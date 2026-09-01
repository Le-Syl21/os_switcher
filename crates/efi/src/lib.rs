//! Read and set the UEFI boot configuration through EFI variables.
//!
//! On a UEFI firmware the boot menu is driven by variables in the global
//! namespace: `BootOrder` (permanent order), `BootNext` (one-shot override,
//! consumed and cleared by the firmware at the next boot), `BootCurrent`
//! (what we booted on) and one `Boot####` load option per entry. This crate
//! exposes them as: list the bootable entries, read/set the default
//! (`BootOrder`), arm/clear a one-shot next boot (`BootNext`).
//!
//! The NVRAM access is abstracted behind the [`Nvram`] trait so the boot logic
//! is testable without UEFI hardware. The real backend ([`SystemNvram`], on
//! Linux and Windows) is provided by `efivar`.

use core::fmt;

mod backend;
mod load_option;

pub use backend::SystemNvram;
pub use load_option::OsKind;

/// Errors when accessing or changing the boot configuration.
#[derive(Debug)]
pub enum Error {
    /// The NVRAM backend failed (I/O, permissions, no UEFI firmware…).
    Backend(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Backend(m) => write!(f, "NVRAM backend error: {m}"),
        }
    }
}
impl std::error::Error for Error {}

/// Crate-specific result type.
pub type Result<T> = core::result::Result<T, Error>;

/// Raw access to EFI variables in the global namespace, by name.
///
/// Implemented by [`SystemNvram`] (real firmware) and by test doubles. Only the
/// handful of boot variables this crate needs are ever touched.
pub trait Nvram {
    /// Reads a variable's raw bytes, or `None` if it does not exist.
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    /// Writes (creates or replaces) a variable.
    fn write(&mut self, name: &str, data: &[u8]) -> Result<()>;
    /// Deletes a variable. Errors if it does not exist.
    fn delete(&mut self, name: &str) -> Result<()>;
    /// Whether a variable exists.
    fn exists(&self, name: &str) -> bool {
        self.read(name).is_some()
    }
}

/// A UEFI boot entry (a resolved `Boot####` load option).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EfiEntry {
    /// The `Boot####` id (e.g. `0x0000`).
    pub id: u16,
    /// The load option description ("Windows Boot Manager", "ubuntu"…).
    pub description: String,
    /// The loader file path extracted from the device path, if any.
    pub path: String,
    /// Best-effort guess of which OS this entry boots.
    pub kind: OsKind,
}

/// The UEFI boot configuration, over some [`Nvram`] backend.
pub struct Efi<N: Nvram> {
    nvram: N,
}

impl<N: Nvram> Efi<N> {
    /// Wraps an NVRAM backend.
    pub fn new(nvram: N) -> Self {
        Efi { nvram }
    }

    /// The permanent boot order (`BootOrder`), most preferred first.
    pub fn boot_order(&self) -> Vec<u16> {
        self.nvram
            .read("BootOrder")
            .map(|b| u16_list(&b))
            .unwrap_or_default()
    }

    /// The entry we booted on (`BootCurrent`), if exposed.
    pub fn boot_current(&self) -> Option<u16> {
        self.nvram.read("BootCurrent").and_then(|b| first_u16(&b))
    }

    /// The one-shot override armed for the next boot (`BootNext`), if any.
    pub fn boot_next(&self) -> Option<u16> {
        self.nvram.read("BootNext").and_then(|b| first_u16(&b))
    }

    /// The bootable entries, in `BootOrder`, with description and OS guessed.
    pub fn entries(&self) -> Vec<EfiEntry> {
        self.boot_order()
            .into_iter()
            .filter_map(|id| self.entry(id))
            .collect()
    }

    /// Resolves a single `Boot####` entry.
    pub fn entry(&self, id: u16) -> Option<EfiEntry> {
        let raw = self.nvram.read(&boot_var_name(id))?;
        let parsed = load_option::parse(&raw)?;
        Some(EfiEntry {
            id,
            kind: OsKind::guess(&parsed.description, &parsed.path),
            description: parsed.description,
            path: parsed.path,
        })
    }

    /// Sets the default OS by moving `id` to the front of `BootOrder`, keeping
    /// every other entry in place. Does not touch `Boot####` load options.
    pub fn set_default(&mut self, id: u16) -> Result<()> {
        let mut order = self.boot_order();
        order.retain(|&x| x != id);
        order.insert(0, id);
        self.nvram.write("BootOrder", &u16_bytes(&order))
    }

    /// Arms a one-shot boot to `id` for the next reboot (`BootNext`). The
    /// firmware consumes and clears it, then falls back to `BootOrder`.
    pub fn set_boot_next(&mut self, id: u16) -> Result<()> {
        self.nvram.write("BootNext", &id.to_le_bytes())
    }

    /// Clears any armed one-shot boot (idempotent).
    pub fn clear_boot_next(&mut self) -> Result<()> {
        if self.nvram.exists("BootNext") {
            self.nvram.delete("BootNext")?;
        }
        Ok(())
    }

    /// Borrows the backend (e.g. to inspect other variables in tests).
    pub fn nvram(&self) -> &N {
        &self.nvram
    }
}

/// `Boot####` variable name for an id (uppercase hex, 4 digits).
fn boot_var_name(id: u16) -> String {
    format!("Boot{id:04X}")
}

fn u16_list(bytes: &[u8]) -> Vec<u16> {
    let (chunks, _) = bytes.as_chunks::<2>();
    chunks
        .iter()
        .map(|&[a, b]| u16::from_le_bytes([a, b]))
        .collect()
}

fn first_u16(bytes: &[u8]) -> Option<u16> {
    (bytes.len() >= 2).then(|| u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u16_bytes(ids: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * 2);
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// In-memory NVRAM double.
    #[derive(Default)]
    struct MockNvram {
        vars: HashMap<String, Vec<u8>>,
    }
    impl Nvram for MockNvram {
        fn read(&self, name: &str) -> Option<Vec<u8>> {
            self.vars.get(name).cloned()
        }
        fn write(&mut self, name: &str, data: &[u8]) -> Result<()> {
            self.vars.insert(name.to_string(), data.to_vec());
            Ok(())
        }
        fn delete(&mut self, name: &str) -> Result<()> {
            self.vars
                .remove(name)
                .map(|_| ())
                .ok_or_else(|| Error::Backend(format!("{name}: not found")))
        }
    }

    /// Builds a minimal EFI_LOAD_OPTION with the given description and file path.
    fn load_option(description: &str, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_le_bytes()); // attributes: ACTIVE
                                                    // Device path: a single Media/File-Path node + End node.
        let mut dp = Vec::new();
        let name16: Vec<u8> = path
            .encode_utf16()
            .chain(core::iter::once(0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let node_len = (4 + name16.len()) as u16;
        dp.push(0x04); // type: Media
        dp.push(0x04); // subtype: File Path
        dp.extend_from_slice(&node_len.to_le_bytes());
        dp.extend_from_slice(&name16);
        dp.extend_from_slice(&[0x7F, 0xFF, 0x04, 0x00]); // End Entire
        out.extend_from_slice(&(dp.len() as u16).to_le_bytes()); // file path list length
                                                                 // Description, UTF-16LE, NUL-terminated.
        for u in description.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&dp);
        out
    }

    fn sample() -> Efi<MockNvram> {
        let mut m = MockNvram::default();
        m.vars
            .insert("BootOrder".into(), u16_bytes(&[0x0000, 0x0001]));
        m.vars
            .insert("BootCurrent".into(), 0x0001u16.to_le_bytes().to_vec());
        m.vars.insert(
            "Boot0000".into(),
            load_option(
                "Windows Boot Manager",
                "\\EFI\\Microsoft\\Boot\\bootmgfw.efi",
            ),
        );
        m.vars.insert(
            "Boot0001".into(),
            load_option("ubuntu", "\\EFI\\ubuntu\\shimx64.efi"),
        );
        Efi::new(m)
    }

    #[test]
    fn reads_order_and_current() {
        let efi = sample();
        assert_eq!(efi.boot_order(), vec![0x0000, 0x0001]);
        assert_eq!(efi.boot_current(), Some(0x0001));
        assert_eq!(efi.boot_next(), None);
    }

    #[test]
    fn lists_entries_with_os_kind() {
        let efi = sample();
        let e = efi.entries();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].description, "Windows Boot Manager");
        assert_eq!(e[0].kind, OsKind::Windows);
        assert_eq!(e[1].description, "ubuntu");
        assert_eq!(e[1].kind, OsKind::Linux);
        assert!(e[0].path.ends_with("bootmgfw.efi"));
    }

    #[test]
    fn set_default_moves_to_front_keeping_rest() {
        let mut efi = sample();
        efi.set_default(0x0001).unwrap();
        assert_eq!(efi.boot_order(), vec![0x0001, 0x0000]);
        // Idempotent-ish: setting the same again keeps it first, no duplicate.
        efi.set_default(0x0001).unwrap();
        assert_eq!(efi.boot_order(), vec![0x0001, 0x0000]);
    }

    #[test]
    fn arm_and_clear_boot_next() {
        let mut efi = sample();
        efi.set_boot_next(0x0000).unwrap();
        assert_eq!(efi.boot_next(), Some(0x0000));
        efi.clear_boot_next().unwrap();
        assert_eq!(efi.boot_next(), None);
        // clear is idempotent.
        efi.clear_boot_next().unwrap();
    }
}
