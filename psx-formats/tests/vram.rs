//! The VRAM upload codec, on a synthetic upload.
//!
//! The upload is built here from constants.

use std::fs;

use psx_formats::gfx::vram::{self, Descriptor, Plane, RECTANGLE_BYTES, RECTANGLE_SIDE};

mod support;

/// A payload of `count` rectangles, each one word of the rectangle's index.
fn payload(count: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count * RECTANGLE_BYTES);
    for rectangle in 0..count {
        for word in 0..RECTANGLE_SIDE * RECTANGLE_SIDE {
            bytes.extend_from_slice(&((rectangle * 1000 + word) as u16).to_le_bytes());
        }
    }
    bytes
}

/// A descriptor for one row of `chunks_per_row` rectangles.
fn descriptor(column: u8, row: u8, chunks_per_row: u8) -> Descriptor {
    Descriptor { column, row, chunks_per_row }
}

#[test]
fn an_upload_decodes_into_its_rectangles() {
    let bytes = payload(4);
    let rectangles = vram::decode(&bytes, descriptor(2, 3, 2)).expect("four rectangles tile two rows");
    assert_eq!(rectangles.len(), 4);
    assert_eq!((rectangles[0].x, rectangles[0].y), (64, 96));
    assert_eq!((rectangles[1].x, rectangles[1].y), (96, 96));
    assert_eq!((rectangles[2].x, rectangles[2].y), (64, 128));
    assert_eq!(rectangles[0].words[0], 0);
    assert_eq!(rectangles[1].words[0], 1000);
}

#[test]
fn an_upload_encodes_back_to_its_payload() {
    let bytes = payload(4);
    let rectangles = vram::decode(&bytes, descriptor(2, 3, 2)).unwrap();
    assert_eq!(vram::encode(&rectangles), bytes);
    let out = support::out_dir("an_upload_encodes_back_to_its_payload");
    let mut plane = Plane::new();
    plane.blit(&rectangles);
    fs::write(out.join("upload.png"), plane.png()).unwrap();
    fs::write(out.join("upload.raw"), &bytes).unwrap();
}

#[test]
fn a_payload_from_a_grid_is_the_inverse_of_the_grid() {
    let bytes = payload(2);
    let grid = vram::word_grid(&bytes, descriptor(0, 0, 2)).unwrap();
    assert_eq!(vram::payload_from_grid(grid.width, grid.height, &grid.words).unwrap(), bytes);
}
