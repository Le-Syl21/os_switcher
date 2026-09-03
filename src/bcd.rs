//! Semantic access to the Windows **BCD** (Boot Configuration Data) store, on
//! top of [`regf_rs`].
//!
//! The BCD is a REGF registry hive whose "objects" (identified by GUID)
//! describe the boot manager and the OS loaders. This crate hides the raw
//! structure (hexadecimal element codes, MultiSz lists of GUIDs) behind a
//! usage-oriented API: list the bootable OSes, read and set the default OS,
//! arm or clear a one-shot next boot.
//!
//! ```no_run
//! use os_switcher::bcd::Bcd;
//! let mut bcd = Bcd::from_file("/boot/efi/EFI/Microsoft/Boot/BCD")?;
//! for e in bcd.entries() {
//!     println!("{} {}", if Some(&e.id) == bcd.default().as_ref() { "*" } else { " " }, e.description);
//! }
//! # Ok::<(), os_switcher::bcd::Error>(())
//! ```

use regf_rs::{Hive, RegValue};

/// GUID of the Windows Boot Manager (a Microsoft constant, identical on every
/// machine — not a hardware identifier).
pub const BOOTMGR: &str = "{9dea862c-5cdd-4e70-acc1-f32b344d4795}";

/// Codes of the BCD elements handled (subkey names, in hexadecimal).
mod elem {
    /// Boot Manager → default object (`DefaultObject`).
    pub const DEFAULT: &str = "23000003";
    /// Boot Manager → display order (`DisplayOrder`), a list of GUIDs.
    pub const DISPLAY_ORDER: &str = "24000001";
    /// Boot Manager → one-shot boot sequence (`BootSequence`).
    pub const BOOT_SEQUENCE: &str = "24000002";
    /// OS loader → human-readable description (`Description`).
    pub const DESCRIPTION: &str = "12000004";
}

/// Errors when manipulating the BCD.
#[derive(Debug)]
pub enum Error {
    /// Error from the underlying REGF engine.
    Regf(regf_rs::RegError),
    /// File I/O error (`std` feature).
    Io(std::io::Error),
    /// The expected Boot Manager object is missing: not a valid BCD.
    NotABcd,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Regf(e) => write!(f, "REGF error: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::NotABcd => write!(f, "missing Boot Manager object: not a valid BCD"),
        }
    }
}
impl std::error::Error for Error {}
impl From<regf_rs::RegError> for Error {
    fn from(e: regf_rs::RegError) -> Self {
        Error::Regf(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Crate-specific result type.
pub type Result<T> = core::result::Result<T, Error>;

/// A boot entry (an OS loader from the BCD).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    /// The object's GUID, in braces.
    pub id: String,
    /// Human-readable description ("Windows 11", "Ubuntu"…), or the GUID as a
    /// fallback.
    pub description: String,
}

/// A BCD store opened in memory.
pub struct Bcd {
    hive: Hive,
}

impl Bcd {
    /// Opens a BCD from raw bytes, validating the presence of the Boot Manager.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let hive = Hive::from_bytes(data)?;
        let bcd = Bcd { hive };
        // Probe: the Boot Manager must exist.
        bcd.hive
            .open(&bcd.bootmgr_elements())
            .map_err(|_| Error::NotABcd)?;
        Ok(bcd)
    }

    /// Opens a BCD from a file.
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// GUID of the default OS, if set.
    pub fn default(&self) -> Option<String> {
        match self
            .hive
            .get_value(&self.bootmgr_element(elem::DEFAULT), "Element")
        {
            Ok(RegValue::Sz(s)) => Some(s),
            _ => None,
        }
    }

    /// GUIDs armed for the next boot (one-shot), if any.
    pub fn boot_sequence(&self) -> Vec<String> {
        self.multi_sz(&self.bootmgr_element(elem::BOOT_SEQUENCE))
    }

    /// GUIDs of the OSes in display order.
    pub fn display_order(&self) -> Vec<String> {
        self.multi_sz(&self.bootmgr_element(elem::DISPLAY_ORDER))
    }

    /// Human-readable description of an object, if available.
    pub fn description_of(&self, guid: &str) -> Option<String> {
        let path = format!("Objects\\{guid}\\Elements\\{}", elem::DESCRIPTION);
        match self.hive.get_value(&path, "Element") {
            Ok(RegValue::Sz(s)) => Some(s),
            _ => None,
        }
    }

    /// Lists the bootable OSes (display order), with resolved descriptions.
    pub fn entries(&self) -> Vec<BootEntry> {
        self.display_order()
            .into_iter()
            .map(|id| {
                let description = self.description_of(&id).unwrap_or_else(|| id.clone());
                BootEntry { id, description }
            })
            .collect()
    }

    /// Sets the default OS (`DefaultObject`).
    pub fn set_default(&mut self, guid: &str) -> Result<()> {
        let path = self.bootmgr_element(elem::DEFAULT);
        self.hive.create_key(&path)?;
        self.hive
            .set_value(&path, "Element", RegValue::Sz(guid.to_string()))?;
        Ok(())
    }

    /// Arms a one-shot boot to `guid` (`BootSequence`): consumed by Windows at
    /// the next boot, like UEFI's `BootNext`.
    pub fn set_boot_sequence(&mut self, guid: &str) -> Result<()> {
        let path = self.bootmgr_element(elem::BOOT_SEQUENCE);
        self.hive.create_key(&path)?;
        self.hive
            .set_value(&path, "Element", RegValue::MultiSz(vec![guid.to_string()]))?;
        Ok(())
    }

    /// Clears any armed one-shot boot (idempotent).
    pub fn clear_boot_sequence(&mut self) -> Result<()> {
        let path = self.bootmgr_element(elem::BOOT_SEQUENCE);
        match self.hive.delete_value(&path, "Element") {
            Ok(()) => Ok(()),
            Err(regf_rs::RegError::ValueNotFound(_)) | Err(regf_rs::RegError::KeyNotFound(_)) => {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Serializes the BCD (checksum and sequences finalized).
    pub fn to_bytes(&mut self) -> Vec<u8> {
        self.hive.to_bytes()
    }

    /// Writes the BCD to a file.
    pub fn save<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }

    // -- internals --

    fn bootmgr_elements(&self) -> String {
        format!("Objects\\{BOOTMGR}\\Elements")
    }
    fn bootmgr_element(&self, code: &str) -> String {
        format!("Objects\\{BOOTMGR}\\Elements\\{code}")
    }
    fn multi_sz(&self, path: &str) -> Vec<String> {
        match self.hive.get_value(path, "Element") {
            Ok(RegValue::MultiSz(v)) => v,
            _ => Vec::new(),
        }
    }
}
