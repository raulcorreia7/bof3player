//! The CLUT palette codec, on a synthetic bank.
//!
//! The bank is built here from constants.

use std::fs;

use psx_formats::gfx::clut::{Palette, ROW_WORDS_4BPP};

mod support;

/// One CLUT row: 16 color words, `0x20` bytes.
fn row() -> Vec<u8> {
    (0..ROW_WORDS_4BPP).flat_map(|index| (0x8000 | index as u16).to_le_bytes()).collect()
}

/// A bank of two rows.
fn bank() -> Vec<u8> {
    let mut bytes = row();
    bytes.extend(row());
    bytes
}

#[test]
fn a_palette_decodes_its_words_and_encodes_back() {
    let bytes = bank();
    let palette = Palette::decode(&bytes).expect("two CLUT rows are a bank");
    assert_eq!(palette.words().len(), 32);
    assert_eq!(palette.words()[0], 0x8000);
    assert_eq!(palette.rows_4bpp(), 2);
    assert_eq!(palette.rows_8bpp(), None, "32 words are not a whole 8bpp row");
    assert_eq!(palette.encode(), bytes);
    let out = support::out_dir("a_palette_decodes_its_words_and_encodes_back");
    fs::write(out.join("palette.png"), palette.png_strip()).unwrap();
    fs::write(out.join("palette-colors.json"), palette.json()).unwrap();
    fs::write(out.join("palette.raw"), &bytes).unwrap();
}
