//! The live Windows BCD store, driven through `bcdedit`.
//!
//! On a running Windows the boot store is not just a file on the ESP: the
//! kernel keeps it loaded as a registry hive, so editing the file in place is
//! neither safe nor reliable. `bcdedit` is the supported door, and it is
//! already on every Windows install.
//!
//! Reads go through `bcdedit /export`, which writes an unlocked *copy* of the
//! live store — a plain REGF hive that [`crate::bcd`] parses like any
//! other. Writes use the GUID-taking subcommands. Neither depends on the
//! system language, unlike parsing `bcdedit /enum` output.
//!
//! Every one of these calls needs an elevated token; see [`crate::switcher::elevate`].

use std::path::PathBuf;
use std::process::Output;

use crate::bcd::{Bcd, BOOTMGR};

use crate::switcher::sys::{decode_output, quiet_command};
use crate::switcher::{Error, Result};

/// Reads the live store into memory.
pub fn export() -> Result<Bcd> {
    let path = export_path();
    // bcdedit refuses to overwrite; start from a clean slot.
    let _ = std::fs::remove_file(&path);

    let result = run(&["/export", &path.to_string_lossy()])
        .and_then(|()| std::fs::read(&path).map_err(Error::Io))
        .and_then(|bytes| Bcd::from_bytes(bytes).map_err(Error::Bcd));

    let _ = std::fs::remove_file(&path);
    result
}

/// Makes `guid` the default OS (`bcdedit /default`).
pub fn set_default(guid: &str) -> Result<()> {
    run(&["/default", guid])
}

/// Arms `guid` for the next boot only (`bcdedit /bootsequence`).
pub fn set_boot_sequence(guid: &str) -> Result<()> {
    run(&["/bootsequence", guid])
}

/// Clears the one-shot boot sequence.
///
/// `bcdedit` fails when the element is not set, which is the same outcome the
/// caller asked for, so an already-clear store is reported as success.
pub fn clear_boot_sequence(was_set: bool) -> Result<()> {
    if !was_set {
        return Ok(());
    }
    run(&["/deletevalue", BOOTMGR, "bootsequence"])
}

/// Where the temporary export lands. Per-process, so two runs cannot collide.
fn export_path() -> PathBuf {
    std::env::temp_dir().join(format!("os-switcher-bcd-{}.tmp", std::process::id()))
}

/// Runs `bcdedit` with `args`, mapping a non-zero exit to a readable error.
fn run(args: &[&str]) -> Result<()> {
    let output: Output = quiet_command("bcdedit")
        .args(args)
        .output()
        .map_err(|e| Error::Elevation(format!("could not run bcdedit: {e}")))?;

    if output.status.success() {
        return Ok(());
    }
    // bcdedit reports failures on stdout as often as on stderr.
    let mut message = decode_output(&output.stderr);
    if message.is_empty() {
        message = decode_output(&output.stdout);
    }
    if message.is_empty() {
        message = format!("bcdedit exited with {}", output.status);
    }
    Err(Error::Elevation(format!("bcdedit {}: {message}", args[0])))
}
