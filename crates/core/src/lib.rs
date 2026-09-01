//! Unified next-boot / default-OS switcher over two mechanisms:
//!
//! - **UEFI** (`BootOrder` / `BootNext`) for OSes that have their own firmware
//!   boot entry — a Linux, or a Windows on its own ESP.
//! - **Windows BCD** (`DefaultObject` / `BootSequence`) to distinguish several
//!   Windows installations that share one ESP, and therefore a single firmware
//!   "Windows Boot Manager" entry.
//!
//! Both Windows layouts are handled:
//!
//! - **Separate disks** — each Windows has its own `Boot####` entry; they show
//!   up as independent UEFI entries.
//! - **Shared ESP** — the single "Windows Boot Manager" UEFI entry is expanded
//!   into one entry per BCD object (Windows 10, Windows 11…). Selecting one
//!   points the firmware at the Windows Boot Manager entry *and* selects the
//!   right object in the BCD, in a single [`Switcher::set`] call.

use std::path::PathBuf;

use os_switcher_bcd::Bcd;
use os_switcher_efi::Efi;

pub use os_switcher_efi::{Nvram, OsKind};

mod power;
pub use power::{reboot, shutdown};

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod detect;

/// Whether a choice is permanent (the new default) or for the next boot only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Permanent default (`BootOrder` / BCD `DefaultObject`).
    Default,
    /// One-shot, consumed at the next boot (`BootNext` / BCD `BootSequence`).
    Once,
}

/// Where an entry actually boots — the mechanism to drive.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BootTarget {
    /// A standalone UEFI entry.
    Efi { id: u16 },
    /// A Windows object inside a shared BCD, reached through the UEFI Windows
    /// Boot Manager entry `efi_id`.
    Bcd { efi_id: u16, guid: String },
}

/// A bootable choice presented to the UI / CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable selector key (e.g. `efi:0001` or `bcd:{guid}`).
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Guessed operating system.
    pub kind: OsKind,
    /// Whether this is the current permanent default.
    pub is_default: bool,
    /// Whether this is armed for the next boot.
    pub is_next: bool,
    target: BootTarget,
}

/// A loaded BCD paired with the UEFI entry that reaches it.
struct BcdSlot {
    efi_id: u16,
    bcd: Bcd,
    /// Where to write the BCD back, if it should be persisted.
    path: Option<PathBuf>,
}

/// Errors from a switch operation.
#[derive(Debug)]
pub enum Error {
    Efi(os_switcher_efi::Error),
    Bcd(os_switcher_bcd::Error),
    Io(std::io::Error),
    /// No entry matched the given selector.
    NotFound(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Efi(e) => write!(f, "UEFI error: {e}"),
            Error::Bcd(e) => write!(f, "BCD error: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::NotFound(s) => write!(f, "no entry matches '{s}'"),
        }
    }
}
impl std::error::Error for Error {}
impl From<os_switcher_efi::Error> for Error {
    fn from(e: os_switcher_efi::Error) -> Self {
        Error::Efi(e)
    }
}
impl From<os_switcher_bcd::Error> for Error {
    fn from(e: os_switcher_bcd::Error) -> Self {
        Error::Bcd(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Crate-specific result type.
pub type Result<T> = core::result::Result<T, Error>;

/// The switcher, over some UEFI NVRAM backend and an optional BCD.
pub struct Switcher<N: Nvram> {
    efi: Efi<N>,
    bcd: Option<BcdSlot>,
}

impl<N: Nvram> Switcher<N> {
    /// Builds a switcher from a UEFI backend and an optional BCD slot.
    fn assemble(efi: Efi<N>, bcd: Option<BcdSlot>) -> Self {
        Switcher { efi, bcd }
    }

    /// The bootable entries. A shared-ESP Windows Boot Manager entry is
    /// expanded into one entry per BCD object; every other UEFI entry maps to
    /// a single entry.
    pub fn entries(&self) -> Vec<Entry> {
        let order = self.efi.boot_order();
        let default_efi = order.first().copied();
        let next_efi = self.efi.boot_next();
        let mut out = Vec::new();

        for e in self.efi.entries() {
            match &self.bcd {
                Some(slot) if slot.efi_id == e.id => {
                    let bcd_default = slot.bcd.default();
                    let bcd_next = slot.bcd.boot_sequence();
                    for guid in slot.bcd.display_order() {
                        let label = slot
                            .bcd
                            .description_of(&guid)
                            .unwrap_or_else(|| guid.clone());
                        let is_default =
                            default_efi == Some(e.id) && bcd_default.as_deref() == Some(&guid);
                        let is_next = next_efi == Some(e.id) && bcd_next.iter().any(|g| g == &guid);
                        out.push(Entry {
                            key: format!("bcd:{guid}"),
                            label,
                            kind: OsKind::Windows,
                            is_default,
                            is_next,
                            target: BootTarget::Bcd { efi_id: e.id, guid },
                        });
                    }
                }
                _ => {
                    out.push(Entry {
                        key: format!("efi:{:04X}", e.id),
                        label: e.description.clone(),
                        kind: e.kind,
                        is_default: default_efi == Some(e.id),
                        is_next: next_efi == Some(e.id),
                        target: BootTarget::Efi { id: e.id },
                    });
                }
            }
        }
        out
    }

    /// Resolves a selector — a 0-based index, an entry key, or a case-insensitive
    /// label substring — to an entry.
    pub fn find(&self, selector: &str) -> Option<Entry> {
        let entries = self.entries();
        if let Ok(i) = selector.parse::<usize>() {
            if let Some(e) = entries.get(i) {
                return Some(e.clone());
            }
        }
        if let Some(e) = entries
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(selector))
        {
            return Some(e.clone());
        }
        let needle = selector.to_ascii_lowercase();
        entries
            .into_iter()
            .find(|e| e.label.to_ascii_lowercase().contains(&needle))
    }

    /// Selects an entry for the given scope, performing the combined UEFI+BCD
    /// action when needed. Returns the chosen entry.
    pub fn set(&mut self, selector: &str, scope: Scope) -> Result<Entry> {
        let entry = self
            .find(selector)
            .ok_or_else(|| Error::NotFound(selector.to_string()))?;
        self.apply(&entry.target, scope)?;
        // Return the entry as it now stands.
        Ok(self
            .find(&entry.key)
            .expect("entry still present after apply"))
    }

    fn apply(&mut self, target: &BootTarget, scope: Scope) -> Result<()> {
        match target {
            BootTarget::Efi { id } => match scope {
                Scope::Default => self.efi.set_default(*id)?,
                Scope::Once => self.efi.set_boot_next(*id)?,
            },
            BootTarget::Bcd { efi_id, guid } => {
                let slot = self.bcd.as_mut().expect("BCD target without BCD slot");
                match scope {
                    Scope::Default => {
                        self.efi.set_default(*efi_id)?;
                        slot.bcd.set_default(guid)?;
                    }
                    Scope::Once => {
                        self.efi.set_boot_next(*efi_id)?;
                        slot.bcd.set_boot_sequence(guid)?;
                    }
                }
                persist(slot)?;
            }
        }
        Ok(())
    }

    /// Clears any one-shot override on both mechanisms (idempotent).
    pub fn clear_next(&mut self) -> Result<()> {
        self.efi.clear_boot_next()?;
        if let Some(slot) = self.bcd.as_mut() {
            slot.bcd.clear_boot_sequence()?;
            persist(slot)?;
        }
        Ok(())
    }
}

fn persist(slot: &mut BcdSlot) -> Result<()> {
    if let Some(path) = &slot.path {
        slot.bcd.save(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_switcher_efi::Efi;
    use regf_rs::{Hive, RegValue};
    use std::collections::HashMap;

    struct MockNvram {
        vars: HashMap<String, Vec<u8>>,
    }
    impl Nvram for MockNvram {
        fn read(&self, name: &str) -> Option<Vec<u8>> {
            self.vars.get(name).cloned()
        }
        fn write(&mut self, name: &str, data: &[u8]) -> os_switcher_efi::Result<()> {
            self.vars.insert(name.to_string(), data.to_vec());
            Ok(())
        }
        fn delete(&mut self, name: &str) -> os_switcher_efi::Result<()> {
            self.vars.remove(name);
            Ok(())
        }
    }

    fn load_option(desc: &str, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_le_bytes());
        let mut dp = Vec::new();
        let name16: Vec<u8> = path
            .encode_utf16()
            .chain(core::iter::once(0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let node_len = (4 + name16.len()) as u16;
        dp.push(0x04);
        dp.push(0x04);
        dp.extend_from_slice(&node_len.to_le_bytes());
        dp.extend_from_slice(&name16);
        dp.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]);
        out.extend_from_slice(&(dp.len() as u16).to_le_bytes());
        for u in desc.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&dp);
        out
    }

    const W10: &str = "{aaaaaaaa-0000-0000-0000-000000000010}";
    const W11: &str = "{aaaaaaaa-0000-0000-0000-000000000011}";
    const BOOTMGR: &str = os_switcher_bcd::BOOTMGR;

    fn synthetic_bcd() -> Bcd {
        let mut h = Hive::new_empty("BCD");
        let mk = |h: &mut Hive, p: &str, v: RegValue| {
            h.create_key(p).unwrap();
            h.set_value(p, "Element", v).unwrap();
        };
        let bm = format!("Objects\\{BOOTMGR}\\Elements");
        mk(&mut h, &format!("{bm}\\23000003"), RegValue::Sz(W10.into()));
        mk(
            &mut h,
            &format!("{bm}\\24000001"),
            RegValue::MultiSz(vec![W10.into(), W11.into()]),
        );
        for (g, d) in [(W10, "Windows 10"), (W11, "Windows 11")] {
            mk(
                &mut h,
                &format!("Objects\\{g}\\Elements\\12000004"),
                RegValue::Sz(d.into()),
            );
        }
        Bcd::from_bytes(h.to_bytes()).unwrap()
    }

    /// UEFI: Boot0000 = Windows Boot Manager, Boot0001 = ubuntu.
    fn switcher() -> Switcher<MockNvram> {
        let mut vars = HashMap::new();
        vars.insert(
            "BootOrder".to_string(),
            [0x0000u16, 0x0001]
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect(),
        );
        vars.insert(
            "Boot0000".to_string(),
            load_option(
                "Windows Boot Manager",
                "\\EFI\\Microsoft\\Boot\\bootmgfw.efi",
            ),
        );
        vars.insert(
            "Boot0001".to_string(),
            load_option("ubuntu", "\\EFI\\ubuntu\\shimx64.efi"),
        );
        let efi = Efi::new(MockNvram { vars });
        let slot = BcdSlot {
            efi_id: 0x0000,
            bcd: synthetic_bcd(),
            path: None,
        };
        Switcher::assemble(efi, Some(slot))
    }

    #[test]
    fn expands_shared_esp_windows_into_bcd_children() {
        let s = switcher();
        let labels: Vec<_> = s.entries().iter().map(|e| e.label.clone()).collect();
        // Windows Boot Manager expanded into W10/W11, plus ubuntu.
        assert_eq!(labels, ["Windows 10", "Windows 11", "ubuntu"]);
    }

    #[test]
    fn default_flags_reflect_efi_and_bcd() {
        let s = switcher();
        let e = s.entries();
        // BootOrder[0] = 0x0000 (Windows) and BCD default = W10 → W10 is default.
        assert!(e[0].is_default); // Windows 10
        assert!(!e[1].is_default); // Windows 11
        assert!(!e[2].is_default); // ubuntu
    }

    #[test]
    fn set_windows11_default_drives_both_mechanisms() {
        let mut s = switcher();
        let chosen = s.set("Windows 11", Scope::Default).unwrap();
        assert_eq!(chosen.label, "Windows 11");
        assert!(chosen.is_default);
        // EFI default is the Windows Boot Manager entry, BCD default is W11.
        assert_eq!(s.efi.boot_order().first().copied(), Some(0x0000));
        assert_eq!(s.bcd.as_ref().unwrap().bcd.default().as_deref(), Some(W11));
    }

    #[test]
    fn set_ubuntu_once_uses_boot_next_only() {
        let mut s = switcher();
        let chosen = s.set("ubuntu", Scope::Once).unwrap();
        assert!(chosen.is_next);
        assert_eq!(s.efi.boot_next(), Some(0x0001));
    }

    #[test]
    fn set_windows11_once_arms_both() {
        let mut s = switcher();
        s.set("Windows 11", Scope::Once).unwrap();
        assert_eq!(s.efi.boot_next(), Some(0x0000));
        assert_eq!(
            s.bcd.as_ref().unwrap().bcd.boot_sequence(),
            vec![W11.to_string()]
        );
        // Clearing removes both.
        s.clear_next().unwrap();
        assert_eq!(s.efi.boot_next(), None);
        assert!(s.bcd.as_ref().unwrap().bcd.boot_sequence().is_empty());
    }

    #[test]
    fn selector_by_index_and_key() {
        let s = switcher();
        assert_eq!(s.find("0").unwrap().label, "Windows 10");
        assert_eq!(s.find("2").unwrap().label, "ubuntu");
        assert_eq!(s.find(&format!("bcd:{W11}")).unwrap().label, "Windows 11");
        assert!(s.find("nope").is_none());
    }
}
