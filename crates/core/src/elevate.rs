//! Privilege detection and privileged delegation.
//!
//! Writing to the UEFI NVRAM (and the BCD) requires elevation. The UI stays
//! unprivileged and delegates the actual write to a small, auditable helper
//! binary (`os-switcher-helper`) run through `pkexec` on Linux. The helper only
//! performs `default` / `next` / `clear`, validating the target against the
//! machine's real entries — it never writes an arbitrary variable.

use std::path::PathBuf;
use std::process::Command;

use crate::{Error, Result};

/// Whether the current process can write firmware variables directly.
///
/// On Unix this is `euid == 0`. On other platforms it assumes the caller
/// arranged elevation (e.g. an elevated shell on Windows) and returns `true`.
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // Safe: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Locates the privileged helper: `$OS_SWITCHER_HELPER`, else next to the
/// current executable (development), else the install path.
pub fn helper_path() -> PathBuf {
    if let Ok(p) = std::env::var("OS_SWITCHER_HELPER") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name(helper_file_name());
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from("/usr/libexec").join(helper_file_name())
}

fn helper_file_name() -> &'static str {
    if cfg!(windows) {
        "os-switcher-helper.exe"
    } else {
        "os-switcher-helper"
    }
}

/// Runs the privileged helper with `args`, elevating as needed.
///
/// On Unix, non-root callers go through `pkexec` (a graphical auth prompt,
/// or none on the active local session if a polkit rule allows it). On other
/// platforms the helper is run directly (assumed already elevated).
pub fn run_helper_elevated(args: &[&str]) -> Result<()> {
    let helper = helper_path();
    let mut command;
    #[cfg(unix)]
    {
        if is_root() {
            command = Command::new(&helper);
            command.args(args);
        } else {
            command = Command::new("pkexec");
            command.arg(&helper).args(args);
        }
    }
    #[cfg(not(unix))]
    {
        command = Command::new(&helper);
        command.args(args);
    }

    let status = command
        .status()
        .map_err(|e| Error::Elevation(format!("could not run {}: {e}", helper.display())))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Elevation(format!(
            "helper exited with {}",
            status.code().unwrap_or(-1)
        )))
    }
}
