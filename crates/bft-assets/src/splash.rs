//! The game's splash: `Recoil.exe`'s one bitmap resource, `GAMEBMP`
//! (docs/data/menus.md).
//!
//! A 640×480 8-bit picture — the red "RECOIL" wallpaper the original showed
//! while it loaded and behind its front-end menus (docs/data/menus.md). It is
//! in no archive, only in the executable, so it is read from the user's
//! install at startup like everything else and never copied.
//!
//! Of the file, the reader reads only the standard resource table, in the
//! PE/COFF layout Microsoft documents for Windows program files: the
//! headers and section table that locate it, the table down to the first
//! `RT_BITMAP` resource, and that one bitmap, a DIB it decodes (8-bit
//! paletted or 24/32-bit, bottom-up or top-down, uncompressed). It reads no
//! code and no strings, and takes the bitmap by type, not by name.

use std::path::{Path, PathBuf};

/// A decoded picture: RGBA8, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The executable in an install, matched without regard to case.
pub fn executable(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("recoil.exe"))
        })
}

/// The splash from an install, or `None` when there is no executable or no
/// bitmap in it that decodes.
pub fn read_splash(root: &Path) -> Option<Picture> {
    let bytes = std::fs::read(executable(root)?).ok()?;
    decode_dib(first_bitmap(&bytes)?)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// `RT_BITMAP` in a resource directory.
const RT_BITMAP: u32 = 2;

/// The bytes of the first bitmap resource in a PE file: a DIB, header and
/// all, without the `BITMAPFILEHEADER` a `.bmp` file would have.
pub fn first_bitmap(pe: &[u8]) -> Option<&[u8]> {
    if pe.get(0..2)? != b"MZ" {
        return None;
    }
    let header = u32_at(pe, 0x3c)? as usize;
    if pe.get(header..header + 4)? != b"PE\0\0" {
        return None;
    }
    let file_header = header + 4;
    let sections = u16_at(pe, file_header + 2)? as usize;
    let optional_size = u16_at(pe, file_header + 16)? as usize;
    let optional = file_header + 20;
    // PE32 or PE32+: the data directories start at a different place.
    let directories = match u16_at(pe, optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return None,
    };
    // Directory 2 is the resource table.
    let resource_rva = u32_at(pe, directories + 2 * 8)?;
    if resource_rva == 0 {
        return None;
    }

    // RVA → file offset through the section table.
    let table = optional + optional_size;
    let to_offset = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|index| {
            let section = table + index * 40;
            let virtual_size = u32_at(pe, section + 8)?;
            let virtual_address = u32_at(pe, section + 12)?;
            let raw_size = u32_at(pe, section + 16)?;
            let raw_pointer = u32_at(pe, section + 20)?;
            let span = virtual_size.max(raw_size);
            (rva >= virtual_address && rva < virtual_address + span)
                .then(|| (rva - virtual_address + raw_pointer) as usize)
        })
    };
    let base = to_offset(resource_rva)?;

    // A directory's entries: (id or name, is a subdirectory, offset).
    let entries = |at: usize| -> Option<Vec<(u32, bool, usize)>> {
        let named = u16_at(pe, at + 12)? as usize;
        let ids = u16_at(pe, at + 14)? as usize;
        (0..named + ids)
            .map(|index| {
                let entry = at + 16 + index * 8;
                let name = u32_at(pe, entry)?;
                let target = u32_at(pe, entry + 4)?;
                Some((
                    name,
                    target & 0x8000_0000 != 0,
                    base + (target & 0x7fff_ffff) as usize,
                ))
            })
            .collect()
    };

    // Type → first name → first language → data entry.
    let (_, _, names) = entries(base)?
        .into_iter()
        .find(|(id, is_dir, _)| *id == RT_BITMAP && *is_dir)?;
    let (_, _, languages) = entries(names)?.into_iter().find(|(_, dir, _)| *dir)?;
    let (_, _, data) = entries(languages)?.into_iter().find(|(_, dir, _)| !*dir)?;
    let data_rva = u32_at(pe, data)?;
    let size = u32_at(pe, data + 4)? as usize;
    let start = to_offset(data_rva)?;
    pe.get(start..start + size)
}

/// Decode an uncompressed DIB (`BITMAPINFOHEADER` onward).
pub fn decode_dib(dib: &[u8]) -> Option<Picture> {
    let header_size = u32_at(dib, 0)? as usize;
    let width = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let raw_height = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bits = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    if width <= 0 || raw_height == 0 || compression != 0 {
        return None;
    }
    let (width, height) = (width as usize, raw_height.unsigned_abs() as usize);
    let bottom_up = raw_height > 0;

    let palette_len = match bits {
        8 => match u32_at(dib, 32)? {
            0 => 256,
            used => used as usize,
        },
        24 | 32 => 0,
        _ => return None,
    };
    let palette_at = header_size;
    let pixels_at = palette_at + palette_len * 4;
    let stride = (width * bits as usize / 8).div_ceil(4) * 4;

    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = if bottom_up { height - 1 - y } else { y };
        let line = dib.get(pixels_at + row * stride..pixels_at + row * stride + stride)?;
        for x in 0..width {
            let (b, g, r) = match bits {
                8 => {
                    let index = *line.get(x)? as usize;
                    if index >= palette_len {
                        return None;
                    }
                    let entry = dib.get(palette_at + index * 4..palette_at + index * 4 + 3)?;
                    (entry[0], entry[1], entry[2])
                }
                _ => {
                    let size = bits as usize / 8;
                    let pixel = line.get(x * size..x * size + 3)?;
                    (pixel[0], pixel[1], pixel[2])
                }
            };
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Some(Picture {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2, 8-bit, bottom-up DIB with a two-colour palette.
    fn dib() -> Vec<u8> {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&8u16.to_le_bytes());
        dib.extend_from_slice(&[0; 16]); // compression 0, sizes, resolution
        dib.extend_from_slice(&2u32.to_le_bytes()); // colours used
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // red, blue (BGRx)
        // Bottom row first, each padded to four bytes.
        dib.extend_from_slice(&[1, 1, 0, 0]);
        dib.extend_from_slice(&[0, 1, 0, 0]);
        dib
    }

    #[test]
    fn a_paletted_dib_decodes_top_row_first() {
        let picture = decode_dib(&dib()).expect("decodes");
        assert_eq!((picture.width, picture.height), (2, 2));
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        assert_eq!(picture.rgba, [red, blue, blue, blue].concat());
    }

    #[test]
    fn what_is_not_a_bitmap_or_a_pe_file_is_refused() {
        assert!(first_bitmap(b"not an executable").is_none());
        let mut compressed = dib();
        compressed[16] = 1; // RLE8
        assert!(decode_dib(&compressed).is_none());
    }
}
