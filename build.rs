//! Gives the Windows executable its own icon.
//!
//! The mark itself lives in `src/icon.rs` and is drawn in code, so this script
//! includes that file rather than reading an image: one definition serves both
//! the window icon at run time and the `.ico` resource compiled in here.

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    windows_icon::embed();
}

#[cfg(windows)]
mod windows_icon {
    // The drawing, shared verbatim with the binary.
    include!("src/icon.rs");

    /// The sizes Explorer, the taskbar and Alt-Tab pick between.
    const SIZES: [u32; 4] = [16, 32, 48, 64];

    pub fn embed() {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
        let ico = out.join("os-switcher.ico");
        std::fs::write(&ico, encode_ico(&SIZES)).expect("writing the generated icon");

        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(ico.to_str().expect("OUT_DIR is valid UTF-8"));

        // Windows shows `FileDescription` — not the file name — as the
        // application's name in the UAC consent dialog, Task Manager and the
        // file's properties. That is a display name, so give it one.
        resource.set("FileDescription", "OS Switcher");

        // `winresource` fills in neither of these, though Cargo knows both.
        // A signed build shows them next to the certificate, so derive them
        // from the manifest rather than repeating anything by hand.
        let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
        let authors = authors.replace(';', ", ");
        if !authors.is_empty() {
            resource.set("CompanyName", &authors);
            let license = std::env::var("CARGO_PKG_LICENSE").unwrap_or_default();
            resource.set("LegalCopyright", &format!("© {authors} — {license}"));
        }

        if let Err(e) = resource.compile() {
            // A missing resource compiler must not stop the build: the app then
            // ships without a file icon, which is cosmetic.
            println!("cargo:warning=could not embed the icon resource: {e}");
        }
    }

    /// Packs the drawn sizes into an `.ico`.
    ///
    /// Each image is a bottom-up 32-bit DIB — the classic layout every Windows
    /// version reads — preceded by the directory that indexes them.
    fn encode_ico(sizes: &[u32]) -> Vec<u8> {
        let images: Vec<Vec<u8>> = sizes.iter().map(|&s| encode_dib(s)).collect();

        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
        ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());

        // Images follow the directory, so the first offset is past all entries.
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (&size, image) in sizes.iter().zip(&images) {
            ico.push(if size >= 256 { 0 } else { size as u8 });
            ico.push(if size >= 256 { 0 } else { size as u8 });
            ico.push(0); // palette size: none
            ico.push(0); // reserved
            ico.extend_from_slice(&1u16.to_le_bytes()); // colour planes
            ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
            ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
            ico.extend_from_slice(&offset.to_le_bytes());
            offset += image.len() as u32;
        }
        for image in images {
            ico.extend_from_slice(&image);
        }
        ico
    }

    /// One image: a `BITMAPINFOHEADER`, the BGRA pixels bottom-up, and the
    /// legacy 1-bit AND mask that the format still requires.
    fn encode_dib(size: u32) -> Vec<u8> {
        let rgba = rgba(size);
        let mask_stride = size.div_ceil(32) * 4;
        let xor_len = size * size * 4;
        let and_len = mask_stride * size;

        let mut dib = Vec::with_capacity((40 + xor_len + and_len) as usize);
        dib.extend_from_slice(&40u32.to_le_bytes()); // header size
        dib.extend_from_slice(&(size as i32).to_le_bytes());
        dib.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // colour + mask
        dib.extend_from_slice(&1u16.to_le_bytes()); // planes
        dib.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB, uncompressed
        dib.extend_from_slice(&(xor_len + and_len).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]); // resolution and palette counts

        for y in (0..size).rev() {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // Transparency comes from the alpha channel; leave the mask opaque.
        dib.extend(std::iter::repeat_n(0u8, and_len as usize));
        dib
    }
}
