//! The standard PS1 TIM file, on a synthetic texture and palette.

use std::fs;

use psx_formats::gfx::tim::{Block, File, Mode};

mod support;

/// A texture of one rectangle: 32x32 words, each holding four 4-bit texels.
fn texture() -> Vec<u16> {
    (0..32u16 * 32).map(|word| word % 16).collect()
}

/// A CLUT row of 16 colors.
fn palette() -> Vec<u16> {
    (0..16u16).map(|color| color | 0x8000).collect()
}

#[test]
fn a_tim_reads_back_what_it_writes() {
    let tim = File {
        mode: Mode::Four,
        clut: Some(Block { x: 160, y: 483, width: 16, height: 1, words: palette() }),
        image: Block { x: 704, y: 256, width: 32, height: 32, words: texture() },
    };
    let bytes = tim.write();
    assert_eq!(&bytes[..4], &[0x10, 0x00, 0x00, 0x00], "the ID word is 0x10");
    assert_eq!(bytes[4], 0x10, "the flag word names the 4-bit mode and the CLUT");

    let read = File::read(&bytes).expect("the file reads back");
    assert_eq!(read.mode, Mode::Four);
    let clut = read.clut.expect("the CLUT is there");
    assert_eq!((clut.x, clut.y), (160, 483));
    assert_eq!(clut.words, palette());
    assert_eq!((read.image.x, read.image.y), (704, 256));
    assert_eq!(read.image.words, texture());
    assert_eq!(File::read(&bytes).unwrap().write(), bytes, "a read and write cycle is byte-exact");
    let out = support::out_dir("a_tim_reads_back_what_it_writes");
    fs::write(out.join("texture.tim"), &bytes).unwrap();
    fs::write(
        out.join("blocks.json"),
        format!(
            "{{\"mode\":\"4-bit\",\"clut\":{{\"x\":160,\"y\":483,\"words\":{}}},\"image\":{{\"x\":704,\"y\":256,\"words\":{}}}}}",
            clut.words.len(),
            read.image.words.len()
        ),
    )
    .unwrap();
}
