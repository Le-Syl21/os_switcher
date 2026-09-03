//! Windows privileged component — an *opt-in* service broker.
//!
//! The default way os-switcher obtains its privileges on Windows is a transient
//! UAC prompt (see [`crate::switcher::elevate`]): the elevated process reads and
//! writes the firmware variables directly. Installing this broker is optional —
//! it trades one consent at install time for none at use time, by running a
//! small Windows service as `LocalSystem` that answers exactly two requests over
//! a named pipe (read the boot state, arm `BootNext`) and nothing else.
//!
//! This file is the skeleton wired into the CLI; the later stages fill it in:
//!   - stage 1 — the in-process UEFI reads/writes behind `SeSystemEnvironmentPrivilege`,
//!   - stage 2 — the named-pipe server (SYSTEM) and client (the app),
//!   - stage 3 — install / update / uninstall of the service and its files,
//!   - stage 4 — the launch sequence and state detection that drive them.

use crate::switcher::{Error, Result};

/// One bootable entry as the broker reports it — the UEFI load option stripped
/// to what the client needs to render and select.
#[derive(Debug, Clone)]
pub struct BrokerEntry {
    /// UEFI entry number (`Boot####`).
    pub id: u16,
    /// Human-readable description.
    pub label: String,
}

/// The boot state the broker returns for `get_state`.
#[derive(Debug, Clone, Default)]
pub struct BrokerState {
    /// The entry the machine booted from this time (`BootCurrent`), if known.
    pub current: Option<u16>,
    /// The armed one-shot entry (`BootNext`), if any.
    pub boot_next: Option<u16>,
    /// Every bootable entry, in `BootOrder`.
    pub entries: Vec<BrokerEntry>,
}

/// Whether the service is registered and healthy — points at an existing binary
/// under `%ProgramFiles%`. Cheap and unprivileged, so the launch path can branch
/// on it before deciding between the pipe and a UAC prompt.
pub fn is_installed() -> bool {
    // TODO(stage 3): query the SCM (SERVICE_QUERY_CONFIG is granted to users).
    false
}

/// Registers the service and installs the binary under `%ProgramFiles%` — one
/// UAC prompt. Opt-in: never called automatically, only from `install` or the
/// GUI's "install" banner.
pub fn install() -> Result<()> {
    Err(unimplemented("install"))
}

/// Removes the service and its files (with `purge`, the on-disk state too).
pub fn uninstall(purge: bool) -> Result<()> {
    let _ = purge;
    Err(unimplemented("uninstall"))
}

/// Re-points the service's `ImagePath` at the installed binary after a move.
pub fn repair() -> Result<()> {
    Err(unimplemented("repair-service"))
}

/// SCM entry point: hands control to the service dispatcher. Never run by hand —
/// the Service Control Manager launches `<installed exe> run-service`.
pub fn run_service() -> Result<()> {
    Err(unimplemented("run-service"))
}

/// Client: ask the running service for the current boot state (unprivileged,
/// over the pipe).
pub fn get_state() -> Result<BrokerState> {
    Err(unimplemented("get_state"))
}

/// Client: ask the service to arm `entry` for the next boot (unprivileged).
pub fn set_boot_next(entry: u16) -> Result<()> {
    let _ = entry;
    Err(unimplemented("set_boot_next"))
}

/// Uniform placeholder error while the broker is being built stage by stage.
fn unimplemented(what: &str) -> Error {
    Error::Elevation(format!(
        "the service broker ('{what}') is not implemented yet"
    ))
}
