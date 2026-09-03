//! Privilege detection and self-elevation.
//!
//! Changing the boot configuration needs privileges: root on Linux, an elevated
//! (administrator) token on Windows — where even *reading* the firmware
//! variables requires `SeSystemEnvironmentPrivilege`, which an ordinary token
//! does not hold.
//!
//! When either binary needs privileges it does not have, it re-runs *itself*
//! elevated with the arguments describing the action (the GUI binary, handed a
//! subcommand, runs it and quits instead of opening a window):
//!
//! - **Linux** — through `pkexec`, which shows the polkit prompt (or none at
//!   all on the active local session, see `packaging/`).
//! - **Windows** — through the shell's `runas` verb, which shows the UAC
//!   consent dialog.
//!
//! The elevated run parses the same command line as any other, so the elevated
//! half is neither more nor less capable than the CLI: it resolves the selector
//! against the machine's real entries and refuses anything else.

use std::ffi::OsStr;
use std::path::PathBuf;

use crate::switcher::{Error, Result};

/// Whether the current process can read and write firmware variables.
///
/// Root on Unix; an elevated token on Windows.
pub fn is_elevated() -> bool {
    #[cfg(unix)]
    {
        // Safe: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(windows)]
    {
        windows_impl::is_elevated()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Path of the running executable.
fn current_exe() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|e| Error::Elevation(format!("cannot locate the running executable: {e}")))
}

/// Re-runs this executable with `args`, elevated, and waits for it to finish.
///
/// Returns [`Error::Elevation`] if the user declined the prompt or the elevated
/// run failed.
pub fn run_self_elevated<S: AsRef<OsStr>>(args: &[S]) -> Result<()> {
    let exe = current_exe()?;

    #[cfg(unix)]
    {
        use std::process::Command;
        let mut command = if is_elevated() {
            Command::new(&exe)
        } else {
            let mut c = Command::new("pkexec");
            c.arg(&exe);
            c
        };
        command.args(args);
        let status = command
            .status()
            .map_err(|e| Error::Elevation(format!("could not run {}: {e}", exe.display())))?;
        match status.code() {
            Some(0) => Ok(()),
            // pkexec's own exit codes: 126 = not authorised, 127 = dismissed.
            Some(126) | Some(127) => Err(Error::Elevation("authorization refused".into())),
            Some(c) => Err(Error::Elevation(format!("elevated run failed (exit {c})"))),
            None => Err(Error::Elevation("elevated run was killed".into())),
        }
    }

    #[cfg(windows)]
    {
        if is_elevated() {
            use std::process::Command;
            let status = Command::new(&exe)
                .args(args)
                .status()
                .map_err(|e| Error::Elevation(format!("could not run {}: {e}", exe.display())))?;
            return match status.code() {
                Some(0) => Ok(()),
                Some(c) => Err(Error::Elevation(format!("run failed (exit {c})"))),
                None => Err(Error::Elevation("run was killed".into())),
            };
        }
        windows_impl::run_elevated(&exe, args, true)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = args;
        Err(Error::Elevation(
            "elevation is not supported on this platform".into(),
        ))
    }
}

/// Re-launches this executable elevated **without waiting** for it.
///
/// Used by the GUI on Windows: the whole application needs an elevated token
/// (reads included), so the unprivileged instance hands over and exits rather
/// than showing a UI that can see nothing.
#[cfg(windows)]
pub fn relaunch_self_elevated<S: AsRef<OsStr>>(args: &[S]) -> Result<()> {
    let exe = current_exe()?;
    windows_impl::run_elevated(&exe, args, false)
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use crate::switcher::{Error, Result};

    /// NUL-terminated UTF-16, as the Win32 `W` entry points want it.
    fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(Some(0)).collect()
    }

    /// Whether this process runs with an elevated token.
    pub fn is_elevated() -> bool {
        // Safe: the token handle is closed on every path and the output buffer
        // is sized from the very type it is read into.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut size = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&raw mut elevation).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            );
            CloseHandle(token);
            ok != 0 && elevation.TokenIsElevated != 0
        }
    }

    /// Quotes one argument the way the C runtime's command-line parser expects,
    /// so `std::env::args()` in the child sees exactly what we passed.
    fn quote(arg: &OsStr) -> String {
        let arg = arg.to_string_lossy();
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.into_owned();
        }
        let mut out = String::with_capacity(arg.len() + 2);
        out.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    // Backslashes before a quote must be doubled, then the
                    // quote itself escaped.
                    out.extend(std::iter::repeat_n('\\', backslashes + 1));
                    backslashes = 0;
                    out.push('"');
                    continue;
                }
                _ => backslashes = 0,
            }
            out.push(c);
        }
        // Trailing backslashes would otherwise escape the closing quote.
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
        out
    }

    /// Runs `exe` with `args` through the UAC consent dialog (`runas`).
    ///
    /// When `wait` is set, blocks until the elevated process exits and maps its
    /// exit code; otherwise it only checks that the process started.
    pub fn run_elevated<S: AsRef<OsStr>>(exe: &Path, args: &[S], wait: bool) -> Result<()> {
        let verb = wide("runas");
        let file = wide(exe);
        let params = wide(
            args.iter()
                .map(|a| quote(a.as_ref()))
                .collect::<Vec<_>>()
                .join(" "),
        );

        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL;

        // Safe: every pointer field points at a live NUL-terminated buffer that
        // outlives the call, and cbSize matches the struct passed.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let err = std::io::Error::last_os_error();
            return Err(if err.raw_os_error() == Some(ERROR_CANCELLED as i32) {
                Error::Elevation("administrator approval was refused".into())
            } else {
                Error::Elevation(format!("could not elevate: {err}"))
            });
        }

        let process = info.hProcess;
        if process.is_null() {
            return Err(Error::Elevation(
                "the elevated process did not start".into(),
            ));
        }
        if !wait {
            // Safe: a live handle we own, used nowhere else.
            unsafe { CloseHandle(process) };
            return Ok(());
        }

        // Safe: `process` stays ours until the CloseHandle below.
        let exit = unsafe {
            WaitForSingleObject(process, INFINITE);
            let mut code = 0u32;
            let ok = GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            (ok != 0).then_some(code)
        };
        match exit {
            Some(0) => Ok(()),
            Some(c) => Err(Error::Elevation(format!("elevated run failed (exit {c})"))),
            None => Err(Error::Elevation("elevated run failed".into())),
        }
    }
}
