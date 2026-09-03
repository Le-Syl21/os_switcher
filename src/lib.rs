//! **os-switcher** — choose the next-boot or default OS on a UEFI multiboot
//! machine (Windows/Linux), aware of both the UEFI boot entries and the Windows
//! BCD.
//!
//! The crate ships as a library plus two binaries built from it: `os-switcher`
//! (the CLI, always) and `os-switcher-gui` (the graphical face, behind the
//! `gui` feature). The heavy egui/eframe stack is optional so a plain
//! `cargo install os-switcher` stays small and pulls no GUI system libraries.
//!
//! - [`efi`] — read/set `BootOrder` and `BootNext` over `efivar`.
//! - [`bcd`] — semantic access to the Windows BCD store, over `regf-rs`.
//! - [`switcher`] — the unified switcher tying both mechanisms together.
//! - [`cli`] — the command-line face (`run` is the `os-switcher` entry point).
//! - [`gui`] — the eframe interface (`gui` feature only).

rust_i18n::i18n!("locales");

pub mod bcd;
pub mod efi;
pub mod switcher;

pub mod cli;

// Drawn in code and shared with `build.rs` (which `include!`s the file for the
// Windows `.ico` resource); the module itself is only needed for the GUI's
// run-time window icon.
#[cfg(feature = "gui")]
mod icon;

// Keeps the window-subsystem GUI binary usable from a console and able to show
// a message box before it has a window. Windows- and GUI-only.
#[cfg(all(windows, feature = "gui"))]
mod console;

#[cfg(feature = "gui")]
pub mod gui;
