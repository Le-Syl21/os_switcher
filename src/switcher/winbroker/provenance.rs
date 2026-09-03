//! What is actually installed, for display only (G8).
//!
//! The project is open source: the signature never gates execution. It is shown
//! in "About" so a user (or a bug report) can see whether the binary running as
//! a service is the author's signed build, someone else's, or an unsigned local
//! build. The real protection against a swapped update is the UAC prompt, which
//! names the publisher.

use std::path::Path;

use super::{CLI_EXE, GUI_EXE, INSTALL_DIR};

/// Authenticode status of a file — informational.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// A valid Authenticode signature chaining to a trusted root.
    Signed,
    /// No signature, or one that does not verify.
    Unsigned,
    /// Could not be determined.
    Unknown,
}

/// A snapshot of the installed component, for the "About" panel.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// File version of the installed GUI binary (falls back to this build's).
    pub version: String,
    /// Authenticode status of the installed GUI binary.
    pub signature: Signature,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn program_files() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};
    let mut raw: *mut u16 = std::ptr::null_mut();
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }
    let mut len = 0usize;
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(std::path::PathBuf::from(s))
}

/// The provenance of the installed component. If nothing is installed, reports
/// this build's own version and an unknown signature.
pub fn installed_provenance() -> Provenance {
    let dir = program_files().map(|p| p.join(INSTALL_DIR));
    let gui = dir.as_ref().map(|d| d.join(GUI_EXE));
    let cli = dir.map(|d| d.join(CLI_EXE));

    let version = gui
        .as_deref()
        .and_then(file_version)
        .or_else(|| cli.as_deref().and_then(file_version))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    let signature = gui
        .as_deref()
        .filter(|p| p.exists())
        .map(verify_signature)
        .unwrap_or(Signature::Unknown);

    Provenance { version, signature }
}

/// Reads a PE's `VS_FIXEDFILEINFO` file version as `a.b.c.d`.
fn file_version(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };

    let wpath = wide(&path.to_string_lossy());
    let mut dummy = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(wpath.as_ptr(), &mut dummy) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wpath.as_ptr(), 0, size, buf.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root = wide("\\");
    let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    if unsafe { VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut value, &mut len) } == 0
        || value.is_null()
    {
        return None;
    }
    let info = unsafe { &*(value as *const VS_FIXEDFILEINFO) };
    let ms = info.dwFileVersionMS;
    let ls = info.dwFileVersionLS;
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

/// Verifies a file's Authenticode signature — informational, never blocking.
fn verify_signature(path: &Path) -> Signature {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE,
    };

    let wpath = wide(&path.to_string_lossy());
    let mut file: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file.cbStruct = size_of::<WINTRUST_FILE_INFO>() as u32;
    file.pcwszFilePath = wpath.as_ptr();

    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.Anonymous.pFile = &mut file;

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // INVALID_HANDLE_VALUE as the window handle = no UI, no parent.
    let hwnd = INVALID_HANDLE_VALUE;
    let status = unsafe { WinVerifyTrust(hwnd, &mut action, (&raw mut data).cast()) };

    // Always close the state, whatever the verdict.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(hwnd, &mut action, (&raw mut data).cast()) };

    if status == 0 {
        Signature::Signed
    } else {
        Signature::Unsigned
    }
}
