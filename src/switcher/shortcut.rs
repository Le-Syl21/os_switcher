//! Putting the app in the desktop's application menu.
//!
//! A binary you downloaded and dropped somewhere is invisible to the system:
//! it is not in the Start menu, not in the launcher, and not reachable by
//! typing its name. This registers it, for the current user only — no
//! installer, no administrator rights, nothing outside `$HOME`.
//!
//! It stops there deliberately. **Pinning to the taskbar cannot be done from
//! a program on Windows**: the `taskbarpin` verb was removed in Windows 10,
//! and the ways around it poke at undocumented registry state that breaks
//! between releases. Once the entry exists, pinning is a right-click away —
//! on the Start menu entry on Windows, on the dash icon on GNOME.

use std::path::PathBuf;

use crate::switcher::{Error, Result};

/// The name the entry carries in the menu.
const DISPLAY_NAME: &str = "OS Switcher";

/// Whether the application menu already has an entry.
pub fn is_present() -> bool {
    entry_path().is_some_and(|p| p.is_file())
}

/// Adds (or refreshes) the entry, pointing at the running executable.
pub fn add() -> Result<()> {
    let path = entry_path().ok_or_else(|| missing("the user's application menu"))?;
    let exe = std::env::current_exe()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_entry(&path, &exe)
}

/// Removes the entry. Removing one that is not there is not a failure.
pub fn remove() -> Result<()> {
    let path = entry_path().ok_or_else(|| missing("the user's application menu"))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

fn missing(what: &str) -> Error {
    Error::Io(std::io::Error::other(format!("cannot locate {what}")))
}

/// Where this desktop keeps a user's own menu entries.
#[cfg(windows)]
fn entry_path() -> Option<PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(roaming)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{DISPLAY_NAME}.lnk")),
    )
}

#[cfg(not(windows))]
fn entry_path() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("applications/os-switcher.desktop"))
}

/// Writes a Windows shell link.
///
/// A `.lnk` is an OLE compound document that only the shell knows how to
/// assemble, so this drives the shell — the same way the rest of the crate
/// drives `bcdedit` and `schtasks` rather than reimplementing them.
#[cfg(windows)]
fn write_entry(path: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    // Single quotes are PowerShell's literal string; a quote inside one is
    // escaped by doubling it. Nothing else needs escaping in that form.
    let quote = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{lnk}'); \
         $s.TargetPath = '{exe}'; \
         $s.WorkingDirectory = '{dir}'; \
         $s.IconLocation = '{exe},0'; \
         $s.Description = '{DISPLAY_NAME}'; \
         $s.Save()",
        lnk = quote(path),
        exe = quote(exe),
        dir = quote(exe.parent().unwrap_or(exe)),
    );

    let output = crate::switcher::sys::quiet_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::other(format!(
            "could not create the Start menu entry: {}",
            crate::switcher::sys::decode_output(&output.stderr)
        ))))
    }
}

/// Writes a freedesktop.org desktop entry.
#[cfg(not(windows))]
fn write_entry(path: &std::path::Path, exe: &std::path::Path) -> Result<()> {
    // `Exec` is a command line, so a path with spaces has to be quoted; the
    // spec escapes an inner quote with a backslash.
    let command = exe.display().to_string().replace('"', "\\\"");
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={DISPLAY_NAME}\n\
         Comment=Choose the next-boot or default operating system\n\
         Exec=\"{command}\"\n\
         Icon=os-switcher\n\
         Categories=System;Settings;\n\
         Terminal=false\n"
    );
    std::fs::write(path, entry)?;
    Ok(())
}
