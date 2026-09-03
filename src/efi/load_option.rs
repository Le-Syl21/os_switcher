//! Minimal `EFI_LOAD_OPTION` parsing: description and loader file path.
//!
//! Layout (UEFI spec): `u32 attributes`, `u16 file_path_list_length`,
//! NUL-terminated UTF-16LE `description`, then the device path of that length,
//! then optional data. We extract the description and the Media/File-Path
//! nodes of the device path — enough to label an entry and guess its OS.

/// A parsed load option (only the fields we need).
pub(crate) struct LoadOption {
    pub description: String,
    pub path: String,
}

/// Parses a load option, or `None` if the buffer is too short to be one.
pub(crate) fn parse(bytes: &[u8]) -> Option<LoadOption> {
    if bytes.len() < 6 {
        return None;
    }
    let fpl_len = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;

    // Description: UTF-16LE, NUL-terminated, starting after the 6-byte prefix.
    let mut i = 6;
    let mut units = Vec::new();
    while i + 1 < bytes.len() {
        let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        i += 2;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    let description = String::from_utf16_lossy(&units);

    // Device path follows the description, for fpl_len bytes.
    let end = (i + fpl_len).min(bytes.len());
    let path = extract_file_path(&bytes[i..end]);

    Some(LoadOption { description, path })
}

/// Concatenates the Media/File-Path nodes (type 0x04, subtype 0x04) of a
/// device path into a single string.
fn extract_file_path(dp: &[u8]) -> String {
    let mut j = 0;
    let mut out = String::new();
    while j + 4 <= dp.len() {
        let node_type = dp[j];
        let node_subtype = dp[j + 1];
        let len = u16::from_le_bytes([dp[j + 2], dp[j + 3]]) as usize;
        if len < 4 || j + len > dp.len() {
            break;
        }
        if node_type == 0x7F {
            break; // End of device path
        }
        if node_type == 0x04 && node_subtype == 0x04 {
            let data = &dp[j + 4..j + len];
            let (chunks, _) = data.as_chunks::<2>();
            let units: Vec<u16> = chunks
                .iter()
                .map(|&[a, b]| u16::from_le_bytes([a, b]))
                .take_while(|&u| u != 0)
                .collect();
            out.push_str(&String::from_utf16_lossy(&units));
        }
        j += len;
    }
    out
}

/// A best-effort guess of the operating system an entry boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsKind {
    Windows,
    Linux,
    MacOs,
    /// Unrecognized (network boot, firmware app, unknown loader…).
    Other,
}

impl OsKind {
    /// Guesses the OS from a load option's description and loader path.
    pub fn guess(description: &str, path: &str) -> OsKind {
        let hay = format!("{description}\u{0}{path}").to_ascii_lowercase();
        const WIN: &[&str] = &["microsoft", "bootmgfw", "windows"];
        const LIN: &[&str] = &[
            "ubuntu", "debian", "fedora", "grub", "shim", "systemd", "linux", "rhel", "suse",
            "arch",
        ];
        // A Hackintosh boots macOS through OpenCore or Clover, each a UEFI
        // loader on the ESP; the entry usually names the loader, not "macOS".
        const MAC: &[&str] = &[
            "boot.efi",
            "system/library/coreservices",
            "apple",
            "opencore",
            "clover",
        ];
        if WIN.iter().any(|k| hay.contains(k)) {
            OsKind::Windows
        } else if MAC.iter().any(|k| hay.contains(k)) {
            OsKind::MacOs
        } else if LIN.iter().any(|k| hay.contains(k)) {
            OsKind::Linux
        } else {
            OsKind::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OsKind;

    #[test]
    fn guesses_the_os_from_description_or_path() {
        assert_eq!(
            OsKind::guess("Windows Boot Manager", r"\EFI\Microsoft\Boot\bootmgfw.efi"),
            OsKind::Windows
        );
        assert_eq!(
            OsKind::guess("ubuntu", r"\EFI\ubuntu\shimx64.efi"),
            OsKind::Linux
        );
        // A Hackintosh boots macOS through OpenCore or Clover; the entry names
        // the loader, so those keywords have to map to macOS, not "Other".
        assert_eq!(
            OsKind::guess("OpenCore", r"\EFI\OC\OpenCore.efi"),
            OsKind::MacOs
        );
        assert_eq!(
            OsKind::guess("UEFI OS", r"\EFI\CLOVER\CLOVERX64.efi"),
            OsKind::MacOs
        );
        assert_eq!(OsKind::guess("PXE Network Boot", ""), OsKind::Other);
    }
}
