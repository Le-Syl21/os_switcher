//! Linux permanent authorization — the counterpart of the Windows broker.
//!
//! Reading the UEFI variables is unprivileged on Linux; only writes need root,
//! through `pkexec`. Installing the polkit policy (`allow_active=yes`) and the
//! CLI at its canonical path makes those writes prompt-free for the user
//! physically at the machine, while a remote session still authenticates. This
//! mirrors the Windows service broker: one authorization at install, none after.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::switcher::{Error, Result};

/// Where the polkit action must live to take effect.
const POLICY_PATH: &str = "/usr/share/polkit-1/actions/org.le-syl21.os-switcher.policy";
/// The path the policy names in `exec.path`; pkexec only skips the prompt when
/// it runs *this* binary, so the CLI is installed here.
const INSTALLED_CLI: &str = "/usr/bin/os-switcher";
/// The policy, embedded so the binary can install it without a separate file.
const POLICY_XML: &str = include_str!("../../packaging/org.le-syl21.os-switcher.policy");

/// The canonical CLI path, if it is installed there.
pub fn installed_cli() -> Option<PathBuf> {
    let p = PathBuf::from(INSTALLED_CLI);
    p.exists().then_some(p)
}

/// Whether the prompt-free authorization is in place.
pub fn is_installed() -> bool {
    Path::new(POLICY_PATH).exists() && Path::new(INSTALLED_CLI).exists()
}

/// Installs the CLI at its canonical path and the polkit policy — one prompt.
pub fn install() -> Result<()> {
    let src = source_cli()?;
    // Stage the policy unprivileged, then place both files with a single pkexec.
    let staged = std::env::temp_dir().join("os-switcher.policy");
    std::fs::write(&staged, POLICY_XML)
        .map_err(|e| Error::Elevation(format!("could not stage the policy: {e}")))?;
    let script = format!(
        "set -e; install -Dm755 {} {}; install -Dm644 {} {}",
        quote(&src.to_string_lossy()),
        quote(INSTALLED_CLI),
        quote(&staged.to_string_lossy()),
        quote(POLICY_PATH),
    );
    let status = Command::new("pkexec").args(["sh", "-c", &script]).status();
    let _ = std::fs::remove_file(&staged);
    finish(status)
}

/// Removes the policy and the installed CLI — one prompt.
pub fn uninstall() -> Result<()> {
    let script = format!("rm -f {} {}", quote(POLICY_PATH), quote(INSTALLED_CLI));
    finish(Command::new("pkexec").args(["sh", "-c", &script]).status())
}

fn finish(status: std::io::Result<ExitStatus>) -> Result<()> {
    match status.map(|s| s.code()) {
        Ok(Some(0)) => Ok(()),
        // pkexec: 126 = not authorized, 127 = dismissed.
        Ok(Some(126)) | Ok(Some(127)) => Err(Error::Elevation("authorization refused".into())),
        Ok(Some(c)) => Err(Error::Elevation(format!("install step failed (exit {c})"))),
        _ => Err(Error::Elevation("could not run pkexec".into())),
    }
}

/// The CLI binary to install, taken from beside the running executable (the GUI
/// keeps `os-switcher` next to `os-switcher-gui`).
fn source_cli() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Elevation(format!("cannot locate the running executable: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| Error::Elevation("running executable has no parent directory".into()))?;
    let cli = dir.join("os-switcher");
    if cli.exists() {
        Ok(cli)
    } else {
        Err(Error::Elevation(
            "os-switcher (the CLI) was not found next to this binary; keep them side by side"
                .into(),
        ))
    }
}

/// Wraps a string in single quotes for `sh -c`, escaping embedded quotes.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
