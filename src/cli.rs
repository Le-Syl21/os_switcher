//! The command-line face of os-switcher.
//!
//! With a subcommand it lists, shows, or changes the boot selection; with none
//! it prints the help. The graphical face lives in the separate `os-switcher-gui`
//! binary.
//!
//! Privileges: changing the boot OS needs root (Linux) or an elevated token
//! (Windows, where even *reading* the firmware variables does). The binary
//! re-runs itself elevated when needed, so there is no separate helper.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::switcher::{is_elevated, reboot, run_self_elevated, shutdown, Scope, Switcher};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "os-switcher",
    about = "Pick the next-boot or default OS on a UEFI multiboot machine",
    version
)]
pub(crate) struct Cli {
    /// Explicit path to the BCD hive (when the ESP is not auto-detected).
    #[arg(long, value_name = "PATH", global = true)]
    pub(crate) bcd: Option<PathBuf>,

    /// Internal: write this run's output here instead of the console, so the
    /// unprivileged parent that elevated us can print it.
    #[arg(long, value_name = "PATH", global = true, hide = true)]
    relay: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand, Clone)]
pub(crate) enum Command {
    /// List the bootable entries.
    List,
    /// Show the current default and one-shot selections.
    Status,
    /// Set the permanent default OS.
    Default {
        /// Index, entry key, or a substring of the label.
        selector: String,
    },
    /// Arm an OS for the next boot only (one-shot).
    Next {
        /// Index, entry key, or a substring of the label.
        selector: String,
    },
    /// Clear any armed one-shot selection.
    Clear,
    /// Reboot now.
    Reboot,
    /// Shut down now.
    Shutdown,
    /// Manage the permanent authorization (Windows scheduled task).
    #[cfg(windows)]
    Elevation {
        #[command(subcommand)]
        action: ElevationAction,
    },
}

/// Install, remove or query the no-prompt elevation task.
#[cfg(windows)]
#[derive(Subcommand, Clone)]
enum ElevationAction {
    /// Register the task, so later launches skip the UAC prompt.
    Install,
    /// Remove the task and go back to prompting.
    Remove,
    /// Report whether the task is registered.
    Status,
}

/// Entry point of the `os-switcher` binary.
pub fn run() -> ExitCode {
    let cli = parse_args();

    // No subcommand: this is the CLI, so print the help rather than doing
    // anything. The graphical face is the separate `os-switcher-gui` binary.
    if cli.command.is_none() {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        println!();
        return ExitCode::SUCCESS;
    }

    execute(&cli)
}

/// Parses the command line. Shared with the GUI binary, which re-runs itself
/// elevated with a subcommand and then dispatches it here.
pub(crate) fn parse_args() -> Cli {
    Cli::parse()
}

/// Runs the (already parsed) subcommand and prints or relays its output.
pub(crate) fn execute(cli: &Cli) -> ExitCode {
    match dispatch(cli) {
        Ok(text) => {
            emit(cli, &text, false);
            ExitCode::SUCCESS
        }
        Err(e) => {
            emit(cli, &format!("error: {e}"), true);
            ExitCode::FAILURE
        }
    }
}

/// Prints `text`, or hands it back to the unprivileged parent through the relay
/// file when this run was the elevated half of a command.
fn emit(cli: &Cli, text: &str, is_error: bool) {
    if text.is_empty() {
        return;
    }
    match &cli.relay {
        Some(path) => {
            let _ = std::fs::write(path, text);
        }
        None if is_error => eprintln!("{text}"),
        None => println!("{text}"),
    }
}

/// Runs one subcommand and returns what should be shown.
fn dispatch(cli: &Cli) -> Result<String, Box<dyn std::error::Error>> {
    let command = cli.command.clone().expect("checked by the caller");

    // Power commands need no boot configuration — and no elevation.
    match command {
        Command::Reboot => {
            reboot()?;
            return Ok(String::new());
        }
        Command::Shutdown => {
            shutdown()?;
            return Ok(String::new());
        }
        _ => {}
    }

    // Anything left touches the firmware. If this process cannot, re-run the
    // whole command elevated and relay its output back.
    if needs_elevation(&command) && !is_elevated() {
        return escalate(cli);
    }

    #[cfg(windows)]
    if let Command::Elevation { action } = &command {
        return elevation(action);
    }

    let mut switcher = match &cli.bcd {
        Some(path) => Switcher::detect_with_bcd(path)?,
        None => Switcher::detect(),
    };

    Ok(match command {
        Command::List => list(&switcher),
        Command::Status => status(&switcher),
        Command::Default { selector } => format!(
            "default OS set to: {}",
            switcher.set(&selector, Scope::Default)?.label
        ),
        Command::Next { selector } => format!(
            "next boot armed for: {} (one-shot)",
            switcher.set(&selector, Scope::Once)?.label
        ),
        Command::Clear => {
            switcher.clear_next()?;
            "one-shot selection cleared".to_string()
        }
        Command::Reboot | Command::Shutdown => unreachable!("handled above"),
        #[cfg(windows)]
        Command::Elevation { .. } => unreachable!("handled above"),
    })
}

/// Whether `command` cannot run with the privileges this process has.
///
/// On Windows that is nearly everything: reading a firmware variable needs
/// `SeSystemEnvironmentPrivilege`, which only an elevated token holds. On Linux
/// the variables are world-readable, so only the writes need root.
fn needs_elevation(command: &Command) -> bool {
    // Reading back the task registration is the one thing that touches neither.
    #[cfg(windows)]
    if matches!(
        command,
        Command::Elevation {
            action: ElevationAction::Status
        }
    ) {
        return false;
    }

    if cfg!(windows) {
        true
    } else {
        matches!(
            command,
            Command::Default { .. } | Command::Next { .. } | Command::Clear
        )
    }
}

/// Re-runs this exact command line elevated, then returns what it printed.
///
/// The elevated half gets its own console-less process, so it writes its output
/// to a temporary file that we read back and print here, in the terminal the
/// user is actually looking at.
fn escalate(cli: &Cli) -> Result<String, Box<dyn std::error::Error>> {
    if cli.relay.is_some() {
        // We *are* the elevated half and still lack privileges: elevating again
        // would loop forever.
        return Err("could not obtain the required privileges".into());
    }

    // Create it here, unprivileged, so the file belongs to this user: the
    // elevated half only rewrites it, and /tmp's sticky bit does not then stop
    // us from cleaning up after a root-owned leftover.
    let relay = std::env::temp_dir().join(format!("os-switcher-out-{}.txt", std::process::id()));
    std::fs::write(&relay, "")?;

    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    args.push("--relay".into());
    args.push(relay.clone().into_os_string());

    let outcome = run_self_elevated(&args);
    let text = std::fs::read_to_string(&relay).unwrap_or_default();
    let _ = std::fs::remove_file(&relay);

    match outcome {
        Ok(()) => Ok(text),
        // The elevated run's own message is the useful one when it has any.
        Err(e) if text.is_empty() => Err(Box::new(e)),
        Err(_) => Err(text.trim_start_matches("error: ").trim().to_string().into()),
    }
}

/// `os-switcher elevation …` — the permanent-authorization task.
#[cfg(windows)]
fn elevation(action: &ElevationAction) -> Result<String, Box<dyn std::error::Error>> {
    use crate::switcher::task;
    Ok(match action {
        ElevationAction::Install => {
            task::install()?;
            format!(
                "registered the '{}' task: launches no longer prompt",
                task::TASK_NAME
            )
        }
        ElevationAction::Remove => {
            task::uninstall()?;
            "removed the task: launches prompt again".to_string()
        }
        ElevationAction::Status => if task::is_installed() {
            "registered: launches do not prompt"
        } else {
            "not registered: every launch prompts"
        }
        .to_string(),
    })
}

fn list<N: crate::switcher::Nvram>(switcher: &Switcher<N>) -> String {
    let entries = switcher.entries();
    if entries.is_empty() {
        return "no boot entries found (is this a UEFI system?)".to_string();
    }
    let mut out = String::new();
    for (i, e) in entries.iter().enumerate() {
        let mark = match (e.is_default, e.is_next) {
            (true, true) => "*>",
            (true, false) => "* ",
            (false, true) => " >",
            (false, false) => "  ",
        };
        out.push_str(&format!(
            "{i:>2} {mark} {:<8} {}\n",
            os_label(e.kind),
            e.label
        ));
    }
    out.push_str("\n  * default    > next boot");
    out
}

fn status<N: crate::switcher::Nvram>(switcher: &Switcher<N>) -> String {
    let entries = switcher.entries();
    let default = entries.iter().find(|e| e.is_default);
    let next = entries.iter().find(|e| e.is_next);
    format!(
        "default:   {}\nnext boot: {}",
        default.map(|e| e.label.as_str()).unwrap_or("(unknown)"),
        next.map(|e| e.label.as_str())
            .unwrap_or("(none — follows default)")
    )
}

fn os_label(kind: crate::switcher::OsKind) -> &'static str {
    use crate::switcher::OsKind::*;
    match kind {
        Windows => "Windows",
        Linux => "Linux",
        MacOs => "macOS",
        Other => "Other",
    }
}
