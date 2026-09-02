//! Permanent elevation on Windows, through a scheduled task.
//!
//! Reading *and* writing the firmware boot configuration needs an elevated
//! token, so without help every launch costs a UAC consent dialog. Windows'
//! own answer is a scheduled task registered to run "with highest privileges":
//! starting it with `schtasks /run` grants the task its elevated token without
//! prompting, because the consent was given once, when the task was created.
//!
//! So the flow is:
//!
//! 1. First run — UAC prompt, then the app offers to install the task.
//! 2. Every run after that — the unprivileged instance starts the task and
//!    exits; the task's instance owns the window, elevated, prompt-free.
//!
//! The task carries no arguments: it always launches the GUI. A caller cannot
//! use it to run an arbitrary command line elevated.

use std::path::{Path, PathBuf};

use crate::sys::quiet_command;
use crate::{Error, Result};

/// Task name, as it appears in the Task Scheduler library.
pub const TASK_NAME: &str = "OS Switcher";

/// A date far enough away that the task's mandatory trigger never fires on its
/// own; `schtasks /run` is the only thing that ever starts it.
const NEVER: &str = "01/01/2100";

/// The executable the task is registered to launch, if it is registered.
///
/// Read from the task's XML definition rather than `schtasks`' human-readable
/// listing, because that listing is translated and this must not be.
pub fn registered_target() -> Option<PathBuf> {
    let output = quiet_command("schtasks")
        .args(["/query", "/tn", TASK_NAME, "/xml", "ONE"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let xml = crate::sys::decode_output(&output.stdout);
    let (_, after) = xml.split_once("<Command>")?;
    let (command, _) = after.split_once("</Command>")?;
    Some(PathBuf::from(command.trim().trim_matches('"')))
}

/// Whether the elevation task is registered at all.
pub fn is_installed() -> bool {
    registered_target().is_some()
}

/// Whether the task exists *and* still points at the running executable.
///
/// A task left behind by a copy of the binary that has since moved would start
/// nothing at all, silently — worse than asking for consent.
pub fn is_current() -> bool {
    let (Some(registered), Ok(exe)) = (registered_target(), std::env::current_exe()) else {
        return false;
    };
    same_file(&registered, &exe)
}

/// Compares two paths as the filesystem would: case-insensitively, and after
/// resolving whatever links or relative segments they carry.
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canonical(a)
        .as_os_str()
        .eq_ignore_ascii_case(canonical(b).as_os_str())
}

/// Registers (or refreshes) the task so it points at the running executable.
///
/// Needs an elevated token — this is the one moment the user is asked.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Elevation(format!("cannot locate the running executable: {e}")))?;
    // schtasks takes the whole command line as one argument, so the path needs
    // its own quotes inside it.
    let action = format!("\"{}\" --gui", exe.display());

    let output = quiet_command("schtasks")
        .args([
            "/create", "/f", // replace an existing registration
            "/tn", TASK_NAME, "/tr", &action, "/sc", "ONCE", "/sd", NEVER, "/st", "00:00", "/rl",
            "HIGHEST", // the point of the whole exercise
            "/it",     // interactive: the window belongs to the logged-on session
        ])
        .output()
        .map_err(|e| Error::Elevation(format!("could not run schtasks: {e}")))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Elevation(format!(
            "could not register the task: {}",
            crate::sys::decode_output(&output.stderr)
        )))
    }
}

/// Removes the task, going back to a UAC prompt per launch.
pub fn uninstall() -> Result<()> {
    let output = quiet_command("schtasks")
        .args(["/delete", "/f", "/tn", TASK_NAME])
        .output()
        .map_err(|e| Error::Elevation(format!("could not run schtasks: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Elevation(format!(
            "could not remove the task: {}",
            crate::sys::decode_output(&output.stderr)
        )))
    }
}

/// Starts the elevated instance through the task. Returns `false` when there is
/// no usable registration, so the caller can fall back to the UAC prompt.
pub fn launch() -> bool {
    if !is_current() {
        return false;
    }
    quiet_command("schtasks")
        .args(["/run", "/tn", TASK_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
