//! Windows privileged component — an *opt-in* service broker.
//!
//! The default way os-switcher obtains its privileges on Windows is a transient
//! UAC prompt (see [`crate::switcher::elevate`]): the elevated process reads and
//! writes the firmware variables directly. Installing this broker is optional —
//! it trades one consent at install time for none at use time, by running a
//! small Windows service as `LocalSystem` that answers exactly a handful of
//! whitelisted requests over a named pipe (read the boot state, arm a selection,
//! clear it) and nothing else.
//!
//! Layout:
//!   - [`wire`] — the strict pipe protocol and the privileged operations it maps
//!     to (which reuse the ordinary [`Switcher`](crate::switcher::Switcher)),
//!   - [`pipe`] — the named-pipe server (SYSTEM) and client (the app),
//!   - [`service`] — the SCM dispatcher that hosts the pipe server,
//!   - [`install`] — install / update / uninstall of the service and its files,
//!   - [`provenance`] — version, signature and build provenance, for display.
//!
//! Every request is validated against the machine's real boot entries, re-read
//! at the moment of the call, so a stale or forged selector is refused — the
//! Windows mirror of the narrow Linux polkit rule.

use crate::switcher::OsKind;

mod install;
mod pipe;
mod provenance;
mod service;
mod wire;

pub use provenance::{installed_provenance, Provenance};

/// Service name in the SCM (no spaces: it is an identifier, not a label).
const SERVICE_NAME: &str = "os-switcher-broker";
/// Friendly name shown in services.msc.
const SERVICE_DISPLAY: &str = "OS Switcher boot broker";
/// Named pipe the service listens on and the app connects to.
const PIPE_NAME: &str = r"\\.\pipe\os-switcher-broker";
/// Sub-directory of `%ProgramFiles%` the binaries are installed into.
const INSTALL_DIR: &str = "os-switcher";
/// Event Log source name.
const EVENT_SOURCE: &str = "os-switcher";
/// The CLI binary — this is what the service runs (`run-service`); it carries no
/// GUI stack, so the SYSTEM process stays small.
const CLI_EXE: &str = "os-switcher.exe";
/// The GUI binary, installed alongside so the Start-menu shortcut points at it.
const GUI_EXE: &str = "os-switcher-gui.exe";

/// One bootable entry as the broker reports it — the fields the UI needs to
/// render and select, without the private routing detail of [`crate::switcher::Entry`].
#[derive(Debug, Clone)]
pub struct BrokerEntry {
    /// Stable selector key (`efi:0001` or `bcd:{guid}`).
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Guessed operating system.
    pub kind: OsKind,
    /// Whether this is the current permanent default.
    pub is_default: bool,
    /// Whether this is armed for the next boot.
    pub is_next: bool,
}

/// Whether the service is registered and points at an existing binary under
/// `%ProgramFiles%`. Cheap and unprivileged, so the launch path can branch on it
/// before choosing between the pipe and a UAC prompt.
pub fn is_installed() -> bool {
    install::is_current()
}

/// Registers the service and installs the binaries (one UAC prompt). Opt-in:
/// never called automatically. Re-runs itself elevated when needed.
pub fn install() -> crate::switcher::Result<()> {
    install::run_install()
}

/// Removes the service and its files (with `purge`, the on-disk state too).
pub fn uninstall(purge: bool) -> crate::switcher::Result<()> {
    install::run_uninstall(purge)
}

/// Re-points the service's `ImagePath` at the installed binary after a move.
pub fn repair() -> crate::switcher::Result<()> {
    install::run_repair()
}

/// SCM entry point: hands control to the service dispatcher. Never run by hand.
pub fn run_service() -> crate::switcher::Result<()> {
    service::run()
}

/// Client: the current boot state, fetched from the service over the pipe.
pub fn get_entries() -> crate::switcher::Result<Vec<BrokerEntry>> {
    pipe::client_get_state()
}

/// Client: ask the service to select `key` for `scope`.
pub fn set(key: &str, scope: crate::switcher::Scope) -> crate::switcher::Result<()> {
    pipe::client_set(key, scope)
}

/// Client: ask the service to clear any one-shot selection.
pub fn clear_next() -> crate::switcher::Result<()> {
    pipe::client_clear_next()
}
