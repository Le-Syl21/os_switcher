//! System constructor: builds a [`Switcher`] from the running firmware and,
//! when found, the machine's BCD store.

use std::path::{Path, PathBuf};

use os_switcher_bcd::Bcd;
use os_switcher_efi::{Efi, OsKind, SystemNvram};

use crate::{BcdSlot, Switcher};

impl Switcher<SystemNvram> {
    /// Builds a switcher from the running system: reads the UEFI variables and,
    /// if a single "Windows Boot Manager" entry and a BCD store are found,
    /// pairs them so shared-ESP Windows installations are distinguished.
    pub fn detect() -> Self {
        let efi = Efi::new(SystemNvram::open());

        // Pair the BCD only when exactly one UEFI Windows entry exists (an
        // unambiguous "Windows Boot Manager").
        let windows: Vec<u16> = efi
            .entries()
            .into_iter()
            .filter(|e| e.kind == OsKind::Windows)
            .map(|e| e.id)
            .collect();

        let bcd = if let [efi_id] = windows[..] {
            find_bcd_path().and_then(|path| {
                Bcd::from_file(&path).ok().map(|bcd| BcdSlot {
                    efi_id,
                    bcd,
                    path: Some(path),
                })
            })
        } else {
            None
        };

        Switcher::assemble(efi, bcd)
    }

    /// Like [`detect`](Self::detect) but with an explicit BCD file path, for
    /// systems where autodetection does not find the ESP.
    pub fn detect_with_bcd(bcd_path: impl AsRef<Path>) -> crate::Result<Self> {
        let efi = Efi::new(SystemNvram::open());
        let efi_id = efi
            .entries()
            .into_iter()
            .find(|e| e.kind == OsKind::Windows)
            .map(|e| e.id);
        let bcd = match efi_id {
            Some(efi_id) => Some(BcdSlot {
                efi_id,
                bcd: Bcd::from_file(bcd_path.as_ref())?,
                path: Some(bcd_path.as_ref().to_path_buf()),
            }),
            None => None,
        };
        Ok(Switcher::assemble(efi, bcd))
    }
}

/// Locates the BCD hive on a mounted ESP.
fn find_bcd_path() -> Option<PathBuf> {
    // Well-known mount points first, then anything in /proc/mounts.
    let mut candidates: Vec<PathBuf> = ["/boot/efi", "/boot", "/efi"]
        .iter()
        .map(PathBuf::from)
        .collect();

    #[cfg(target_os = "linux")]
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let mut cols = line.split_whitespace();
            let (_dev, mnt) = (cols.next(), cols.next());
            if let Some(mnt) = mnt {
                candidates.push(PathBuf::from(mnt.replace("\\040", " ")));
            }
        }
    }

    candidates
        .into_iter()
        .map(|m| m.join("EFI/Microsoft/Boot/BCD"))
        .find(|p| p.is_file())
}
