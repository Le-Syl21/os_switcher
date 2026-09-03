//! Keeping a console-less binary usable from a console (Windows only).
//!
//! The executable is linked for the *windows* subsystem so that double-clicking
//! it opens the GUI without a black console window flashing behind it. The
//! price is that Windows gives such a process no standard handles at all, so
//! `println!` from the CLI half would go nowhere.
//!
//! The fix is the one every dual-mode Windows tool uses: attach to the console
//! of whatever launched us, if it has one, and point the standard handles at
//! it. Launched from Explorer or a scheduled task there is no parent console,
//! the call fails, and the GUI runs silently — exactly as intended.

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

/// Attaches to the parent process' console, if there is one.
///
/// Returns whether output will actually be seen by a human.
pub fn attach_parent() -> bool {
    // Safe: no arguments to get wrong; failure just means "no parent console".
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return false;
    }
    // The process has no standard handles of its own — open the console's
    // pseudo-files and install them. Handles the launcher *did* give us (a
    // pipe, a file from `> out.txt`) are left alone: redirection must win.
    adopt(STD_OUTPUT_HANDLE, "CONOUT$");
    adopt(STD_ERROR_HANDLE, "CONOUT$");
    adopt(STD_INPUT_HANDLE, "CONIN$");
    true
}

/// Points one standard handle at a console pseudo-file, unless it already
/// points somewhere the caller chose.
fn adopt(std_handle: u32, name: &str) {
    // Safe: GetStdHandle only reads the process' own table.
    let existing = unsafe { GetStdHandle(std_handle) };
    if !existing.is_null() && existing != INVALID_HANDLE_VALUE {
        return;
    }

    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // Safe: `wide` is NUL-terminated and outlives the call; a failed open is
    // reported through the return value and simply skipped.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if handle != INVALID_HANDLE_VALUE {
            SetStdHandle(std_handle, handle);
        }
    }
}

/// Shows a message box.
///
/// The last resort for a launch that fails before there is a window: started
/// from Explorer there is no console either, so without this the process would
/// simply vanish and leave the user with nothing to go on.
pub fn alert(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND,
    };

    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "OS Switcher".encode_utf16().chain(Some(0)).collect();
    // Safe: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        );
    }
}
