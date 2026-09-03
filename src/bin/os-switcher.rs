//! The `os-switcher` command-line binary — a thin wrapper over
//! [`os_switcher::cli::run`]. Linked for the console subsystem: it is a CLI and
//! its output belongs in the terminal that launched it.

fn main() -> std::process::ExitCode {
    os_switcher::cli::run()
}
