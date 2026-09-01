//! `os-switcher-helper` — the privileged half of os-switcher.
//!
//! Runs elevated (via `pkexec` on Linux, or an elevated context on Windows) and
//! performs only the three write actions. Arguments are parsed by hand and the
//! target is validated against the machine's real boot entries by the core
//! (`Switcher::set` errors on an unknown selector): the helper never writes an
//! arbitrary firmware variable.
//!
//! Usage:
//!   os-switcher-helper [--bcd PATH] default <selector>
//!   os-switcher-helper [--bcd PATH] next <selector>
//!   os-switcher-helper [--bcd PATH] clear

use std::process::ExitCode;

use os_switcher_core::{Scope, Switcher};

fn main() -> ExitCode {
    match run() {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).peekable();

    // Optional `--bcd PATH`.
    let mut bcd_path: Option<String> = None;
    if args.peek().map(String::as_str) == Some("--bcd") {
        args.next();
        bcd_path = Some(args.next().ok_or("--bcd requires a path")?);
    }

    let verb = args.next().ok_or("missing action (default|next|clear)")?;

    let mut switcher = match &bcd_path {
        Some(p) => Switcher::detect_with_bcd(p)?,
        None => Switcher::detect(),
    };

    let msg = match verb.as_str() {
        "default" => {
            let sel = args.next().ok_or("default requires a selector")?;
            let e = switcher.set(&sel, Scope::Default)?;
            format!("default OS set to: {}", e.label)
        }
        "next" => {
            let sel = args.next().ok_or("next requires a selector")?;
            let e = switcher.set(&sel, Scope::Once)?;
            format!("next boot armed for: {} (one-shot)", e.label)
        }
        "clear" => {
            switcher.clear_next()?;
            "one-shot selection cleared".to_string()
        }
        other => return Err(format!("unknown action '{other}'").into()),
    };
    Ok(msg)
}
