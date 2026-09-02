//! Reboot and shutdown, delegated to the platform's standard tools.

use crate::sys::quiet_command;
use crate::Result;

/// Reboots the machine.
pub fn reboot() -> Result<()> {
    run_power(PowerAction::Reboot)
}

/// Powers the machine off.
pub fn shutdown() -> Result<()> {
    run_power(PowerAction::Shutdown)
}

enum PowerAction {
    Reboot,
    Shutdown,
}

#[cfg(target_os = "linux")]
fn run_power(action: PowerAction) -> Result<()> {
    let arg = match action {
        PowerAction::Reboot => "reboot",
        PowerAction::Shutdown => "poweroff",
    };
    quiet_command("systemctl").arg(arg).status()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_power(action: PowerAction) -> Result<()> {
    let flag = match action {
        PowerAction::Reboot => "/r",
        PowerAction::Shutdown => "/s",
    };
    quiet_command("shutdown").args([flag, "/t", "0"]).status()?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn run_power(_action: PowerAction) -> Result<()> {
    Err(crate::Error::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "power control is not supported on this platform",
    )))
}
