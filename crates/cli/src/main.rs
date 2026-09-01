//! `os-switcher` command-line interface.
//!
//! Subcommands bypass any UI and perform the action directly. Writing to the
//! UEFI NVRAM (and the BCD) requires elevated privileges: run under `sudo` /
//! `pkexec` on Linux, or an elevated shell on Windows.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use os_switcher_core::{is_root, reboot, run_helper_elevated, shutdown, Scope, Switcher};

#[derive(Parser)]
#[command(
    name = "os-switcher",
    about = "Pick the next-boot or default OS on a UEFI multiboot machine",
    version
)]
struct Cli {
    /// Explicit path to the BCD hive (when the ESP is not auto-detected).
    #[arg(long, value_name = "PATH", global = true)]
    bcd: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List the bootable entries (the default when no subcommand is given).
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Power commands do not need the boot configuration.
    match &cli.command {
        Some(Command::Reboot) => return Ok(reboot()?),
        Some(Command::Shutdown) => return Ok(shutdown()?),
        _ => {}
    }

    let mut switcher = match &cli.bcd {
        Some(path) => Switcher::detect_with_bcd(path)?,
        None => Switcher::detect(),
    };

    match cli.command.unwrap_or(Command::List) {
        Command::List => print_entries(&switcher),
        Command::Status => print_status(&switcher),
        Command::Default { selector } => {
            write_action(&mut switcher, &cli.bcd, "default", Some(&selector))?
        }
        Command::Next { selector } => {
            write_action(&mut switcher, &cli.bcd, "next", Some(&selector))?
        }
        Command::Clear => write_action(&mut switcher, &cli.bcd, "clear", None)?,
        Command::Reboot | Command::Shutdown => unreachable!("handled above"),
    }
    Ok(())
}

/// Performs a write action, elevating through the privileged helper when the
/// current process is not root. When already root, it acts directly.
fn write_action<N: os_switcher_core::Nvram>(
    switcher: &mut Switcher<N>,
    bcd: &Option<PathBuf>,
    verb: &str,
    selector: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if is_root() {
        match (verb, selector) {
            ("default", Some(s)) => {
                println!(
                    "default OS set to: {}",
                    switcher.set(s, Scope::Default)?.label
                )
            }
            ("next", Some(s)) => println!(
                "next boot armed for: {} (one-shot)",
                switcher.set(s, Scope::Once)?.label
            ),
            ("clear", _) => {
                switcher.clear_next()?;
                println!("one-shot selection cleared");
            }
            _ => unreachable!(),
        }
        return Ok(());
    }

    // Not root: delegate to the privileged helper (graphical auth via pkexec).
    let bcd_str = bcd.as_ref().map(|p| p.to_string_lossy().into_owned());
    let mut args: Vec<&str> = Vec::new();
    if let Some(p) = &bcd_str {
        args.push("--bcd");
        args.push(p);
    }
    args.push(verb);
    if let Some(s) = selector {
        args.push(s);
    }
    run_helper_elevated(&args)?;
    Ok(())
}

fn print_entries<N: os_switcher_core::Nvram>(switcher: &Switcher<N>) {
    let entries = switcher.entries();
    if entries.is_empty() {
        println!("no boot entries found (is this a UEFI system?)");
        return;
    }
    for (i, e) in entries.iter().enumerate() {
        let mark = match (e.is_default, e.is_next) {
            (true, true) => "*>",
            (true, false) => "* ",
            (false, true) => " >",
            (false, false) => "  ",
        };
        println!("{i:>2} {mark} {:<8} {}", os_label(e.kind), e.label);
    }
    println!("\n  * default    > next boot");
}

fn print_status<N: os_switcher_core::Nvram>(switcher: &Switcher<N>) {
    let entries = switcher.entries();
    let default = entries.iter().find(|e| e.is_default);
    let next = entries.iter().find(|e| e.is_next);
    println!(
        "default:   {}",
        default.map(|e| e.label.as_str()).unwrap_or("(unknown)")
    );
    println!(
        "next boot: {}",
        next.map(|e| e.label.as_str())
            .unwrap_or("(none — follows default)")
    );
}

fn os_label(kind: os_switcher_core::OsKind) -> &'static str {
    use os_switcher_core::OsKind::*;
    match kind {
        Windows => "Windows",
        Linux => "Linux",
        MacOs => "macOS",
        Other => "Other",
    }
}
