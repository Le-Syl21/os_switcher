//! The `os-switcher-gui` binary — a thin wrapper over [`os_switcher::gui::run`].
//!
//! Linked for the *windows* subsystem so double-clicking it opens the GUI with
//! no console window flashing behind it; on a terminal launch it re-attaches to
//! that console only to report an error it cannot show as a window.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    os_switcher::gui::run()
}
