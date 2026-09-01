//! Accès sémantique au magasin **BCD** (Boot Configuration Data) de Windows,
//! au-dessus de [`regf_rs`].
//!
//! Le BCD est une ruche de registre REGF dont les « objets » (identifiés par
//! GUID) décrivent le gestionnaire de démarrage et les chargeurs d'OS. Ce crate
//! masque la structure brute (codes d'éléments hexadécimaux, listes MultiSz de
//! GUID) derrière une API orientée usage : lister les OS amorçables, lire et
//! fixer l'OS par défaut, armer ou annuler un démarrage unique (« one-shot »).
//!
//! ```no_run
//! use os_switcher_bcd::Bcd;
//! let mut bcd = Bcd::from_file("/boot/efi/EFI/Microsoft/Boot/BCD")?;
//! for e in bcd.entries() {
//!     println!("{} {}", if Some(&e.id) == bcd.default().as_ref() { "*" } else { " " }, e.description);
//! }
//! # Ok::<(), os_switcher_bcd::Error>(())
//! ```

use regf_rs::{Hive, RegValue};

/// GUID du Windows Boot Manager (constante Microsoft, identique sur toute
/// machine — ce n'est pas un identifiant matériel).
pub const BOOTMGR: &str = "{9dea862c-5cdd-4e70-acc1-f32b344d4795}";

/// Codes des éléments BCD manipulés (noms de sous-clés, en hexadécimal).
mod elem {
    /// Boot Manager → objet par défaut (`DefaultObject`).
    pub const DEFAULT: &str = "23000003";
    /// Boot Manager → ordre d'affichage (`DisplayOrder`), liste de GUID.
    pub const DISPLAY_ORDER: &str = "24000001";
    /// Boot Manager → séquence de démarrage unique (`BootSequence`), one-shot.
    pub const BOOT_SEQUENCE: &str = "24000002";
    /// Chargeur d'OS → description lisible (`Description`).
    pub const DESCRIPTION: &str = "12000004";
}

/// Erreurs de manipulation du BCD.
#[derive(Debug)]
pub enum Error {
    /// Erreur du moteur REGF sous-jacent.
    Regf(regf_rs::RegError),
    /// Entrée/sortie fichier (feature `std`).
    Io(std::io::Error),
    /// L'objet Boot Manager attendu est absent : ce n'est pas un BCD valide.
    NotABcd,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Regf(e) => write!(f, "erreur REGF : {e}"),
            Error::Io(e) => write!(f, "erreur d'E/S : {e}"),
            Error::NotABcd => write!(f, "objet Boot Manager absent : BCD invalide"),
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

/// Résultat spécialisé.
pub type Result<T> = core::result::Result<T, Error>;

/// Une entrée de démarrage (un chargeur d'OS du BCD).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    /// GUID de l'objet, entre accolades.
    pub id: String,
    /// Description lisible (« Windows 11 », « Ubuntu »…), ou le GUID à défaut.
    pub description: String,
}

/// Un magasin BCD ouvert en mémoire.
pub struct Bcd {
    hive: Hive,
}

impl Bcd {
    /// Ouvre un BCD depuis des octets bruts, en validant la présence du
    /// Boot Manager.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let hive = Hive::from_bytes(data)?;
        let bcd = Bcd { hive };
        // Sonde : le Boot Manager doit exister.
        bcd.hive
            .open(&bcd.bootmgr_elements())
            .map_err(|_| Error::NotABcd)?;
        Ok(bcd)
    }

    /// Ouvre un BCD depuis un fichier.
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// GUID de l'OS par défaut, s'il est défini.
    pub fn default(&self) -> Option<String> {
        match self
            .hive
            .get_value(&self.bootmgr_element(elem::DEFAULT), "Element")
        {
            Ok(RegValue::Sz(s)) => Some(s),
            _ => None,
        }
    }

    /// GUID armés pour le prochain démarrage (one-shot), s'il y en a.
    pub fn boot_sequence(&self) -> Vec<String> {
        self.multi_sz(&self.bootmgr_element(elem::BOOT_SEQUENCE))
    }

    /// GUID des OS dans l'ordre d'affichage.
    pub fn display_order(&self) -> Vec<String> {
        self.multi_sz(&self.bootmgr_element(elem::DISPLAY_ORDER))
    }

    /// Description lisible d'un objet, si disponible.
    pub fn description_of(&self, guid: &str) -> Option<String> {
        let path = format!("Objects\\{guid}\\Elements\\{}", elem::DESCRIPTION);
        match self.hive.get_value(&path, "Element") {
            Ok(RegValue::Sz(s)) => Some(s),
            _ => None,
        }
    }

    /// Liste les OS amorçables (ordre d'affichage), description résolue.
    pub fn entries(&self) -> Vec<BootEntry> {
        self.display_order()
            .into_iter()
            .map(|id| {
                let description = self.description_of(&id).unwrap_or_else(|| id.clone());
                BootEntry { id, description }
            })
            .collect()
    }

    /// Fixe l'OS par défaut (`DefaultObject`).
    pub fn set_default(&mut self, guid: &str) -> Result<()> {
        let path = self.bootmgr_element(elem::DEFAULT);
        self.hive.create_key(&path)?;
        self.hive
            .set_value(&path, "Element", RegValue::Sz(guid.to_string()))?;
        Ok(())
    }

    /// Arme un démarrage unique vers `guid` (`BootSequence`) : consommé par
    /// Windows au prochain démarrage, comme `BootNext` côté UEFI.
    pub fn set_boot_sequence(&mut self, guid: &str) -> Result<()> {
        let path = self.bootmgr_element(elem::BOOT_SEQUENCE);
        self.hive.create_key(&path)?;
        self.hive
            .set_value(&path, "Element", RegValue::MultiSz(vec![guid.to_string()]))?;
        Ok(())
    }

    /// Annule un démarrage unique éventuellement armé (idempotent).
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

    /// Sérialise le BCD (checksum et séquences finalisés).
    pub fn to_bytes(&mut self) -> Vec<u8> {
        self.hive.to_bytes()
    }

    /// Écrit le BCD dans un fichier.
    pub fn save<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }

    // -- internes --

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
