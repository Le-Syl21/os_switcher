//! Small process helpers shared by the platform back-ends.

use std::process::Command;

/// A [`Command`] that never flashes a console window.
///
/// The binary is a GUI-subsystem executable on Windows, so a child process
/// started the ordinary way would pop up (and immediately close) a console.
pub fn quiet_command(program: &str) -> Command {
    // `mut` is only used on Windows, where the block below configures the
    // command; elsewhere the binding is returned untouched.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Decodes process output that may be UTF-8 or (as several Windows tools emit
/// when redirected) UTF-16LE.
#[cfg(windows)]
pub fn decode_output(bytes: &[u8]) -> String {
    // UTF-16LE text is full of NUL high bytes; UTF-8 never contains a NUL.
    let looks_utf16 = bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).any(|&b| b == 0);
    let text = if looks_utf16 {
        let (pairs, _) = bytes.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{feff}').trim().to_string()
}
