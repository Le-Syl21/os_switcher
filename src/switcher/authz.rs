//! One "authorize once" action, whichever platform — the opt-in that turns a
//! per-use prompt into a prompt-free session.
//!
//! - **Windows** — the service broker (see [`crate::switcher::winbroker`]).
//! - **Linux** — the polkit policy (see [`crate::switcher::polkit`]).

use crate::switcher::Result;

/// Whether the prompt-free authorization is installed.
#[cfg(windows)]
pub fn is_installed() -> bool {
    crate::switcher::winbroker::is_installed()
}
#[cfg(all(unix, not(windows)))]
pub fn is_installed() -> bool {
    crate::switcher::polkit::is_installed()
}
#[cfg(not(any(windows, unix)))]
pub fn is_installed() -> bool {
    false
}

/// Installs it (one prompt).
#[cfg(windows)]
pub fn install() -> Result<()> {
    crate::switcher::winbroker::install()
}
#[cfg(all(unix, not(windows)))]
pub fn install() -> Result<()> {
    crate::switcher::polkit::install()
}
#[cfg(not(any(windows, unix)))]
pub fn install() -> Result<()> {
    Err(crate::switcher::Error::Elevation(
        "no authorization mechanism on this platform".into(),
    ))
}

/// Removes it (one prompt).
#[cfg(windows)]
pub fn uninstall() -> Result<()> {
    crate::switcher::winbroker::uninstall(false)
}
#[cfg(all(unix, not(windows)))]
pub fn uninstall() -> Result<()> {
    crate::switcher::polkit::uninstall()
}
#[cfg(not(any(windows, unix)))]
pub fn uninstall() -> Result<()> {
    Err(crate::switcher::Error::Elevation(
        "no authorization mechanism on this platform".into(),
    ))
}
