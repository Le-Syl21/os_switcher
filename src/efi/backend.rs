//! Real NVRAM backend, backed by `efivar` (Linux and Windows only).

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod real {
    use crate::efi::{Error, Nvram, Result};
    use efivar::efi::{Variable, VariableFlags};
    use efivar::VarManager;

    /// The running firmware's EFI variables. Writes require elevated
    /// privileges (root / an admin token).
    pub struct SystemNvram {
        mgr: Box<dyn VarManager>,
    }

    impl SystemNvram {
        /// Opens the system EFI variable store.
        pub fn open() -> Self {
            SystemNvram {
                mgr: efivar::system(),
            }
        }

        fn var(name: &str) -> Variable {
            Variable::new(name)
        }
    }

    impl Nvram for SystemNvram {
        fn read(&self, name: &str) -> Option<Vec<u8>> {
            self.mgr
                .read(&Self::var(name))
                .ok()
                .map(|(data, _flags)| data)
        }
        fn write(&mut self, name: &str, data: &[u8]) -> Result<()> {
            self.mgr
                .write(&Self::var(name), VariableFlags::default(), data)
                .map_err(|e| Error::Backend(e.to_string()))
        }
        fn delete(&mut self, name: &str) -> Result<()> {
            self.mgr
                .delete(&Self::var(name))
                .map_err(|e| Error::Backend(e.to_string()))
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use real::SystemNvram;

/// On platforms without an `efivar` backend, `SystemNvram` is unavailable;
/// the [`super::Nvram`] trait and [`super::Efi`] core still build and can run
/// over a custom backend.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub struct SystemNvram;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl SystemNvram {
    /// Unsupported on this platform.
    pub fn open() -> Self {
        SystemNvram
    }
}
