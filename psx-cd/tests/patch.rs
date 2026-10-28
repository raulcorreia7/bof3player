//! The patch writer on a synthetic disc: a shorter file replaces its sectors and
//! its directory record, every sector stays sealed, and a file cannot grow past
//! its last sector. Whole-disc checks live with the extractor.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use psx_cd::Image;
use psx_cd::patch::{Replacement, apply};
use psx_formats::sector::{self, RAW_SIZE};

/// The root directory of the synthetic disc, as the writer's layout has it.
const SYNTHETIC_ROOT_SECTOR: usize = 18;

#[test]
fn a_synthetic_disc_takes_a_shorter_file_and_every_sector_stays_sealed() {
    let dir = support::out_dir("a_synthetic_disc_takes_a_shorter_file_and_every_sector_stays_sealed");
    let source = dir.join("source");
    fs::create_dir(&source).expect("the source directory");
    fs::write(source.join("ALPHA.TXT"), pattern(5000, 1)).expect("ALPHA.TXT");
    fs::write(source.join("BETA.DAT"), pattern(3000, 2)).expect("BETA.DAT");
    let cooked = dir.join("cooked.iso");
    psx_cd::rebuild::iso(&source, &cooked).expect("the cooked image builds");
    let bin = dir.join("synthetic.bin");
    fs::write(&bin, wrap_raw(&fs::read(&cooked).expect("the cooked image"))).expect("the raw image");
    let cue_path = dir.join("synthetic.cue");
    fs::write(&cue_path, "FILE \"synthetic.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n")
        .expect("the cue sheet");

    let output = dir.join("patched");
    let shorter = pattern(1200, 3);
    let summary = apply(&cue_path, &output, &[Replacement { path: "BETA.DAT".into(), bytes: shorter.clone() }])
        .expect("the synthetic disc patches");
    // Two sectors of BETA.DAT and the root directory sector that holds its record.
    assert_eq!((summary.files, summary.sealed), (1, 3));

    let mut image = Image::open(output.join("synthetic.cue")).expect("the patched disc opens");
    let entries = image.entries().expect("the file table reads");
    let beta = entries.iter().find(|entry| entry.path == Path::new("BETA.DAT")).expect("BETA.DAT").clone();
    let alpha = entries.iter().find(|entry| entry.path == Path::new("ALPHA.TXT")).expect("ALPHA.TXT").clone();
    assert_eq!(image.read_file(&beta).expect("BETA.DAT reads"), shorter);
    assert_eq!(image.read_file(&alpha).expect("ALPHA.TXT reads"), pattern(5000, 1));

    let before = fs::read(&bin).expect("the original image");
    let after = fs::read(output.join("synthetic.bin")).expect("the patched image");
    let changed: BTreeSet<usize> = (0..before.len() / RAW_SIZE)
        .filter(|&index| {
            before[index * RAW_SIZE..(index + 1) * RAW_SIZE] != after[index * RAW_SIZE..(index + 1) * RAW_SIZE]
        })
        .collect();
    let first = beta.lba as usize;
    assert_eq!(changed, BTreeSet::from([first, first + 1, SYNTHETIC_ROOT_SECTOR]));
    for raw in after.as_chunks::<RAW_SIZE>().0 {
        assert!(sector::is_sealed(raw));
    }

    let grown =
        apply(&cue_path, &dir.join("grown"), &[Replacement { path: "BETA.DAT".into(), bytes: pattern(4097, 4) }]);
    assert!(grown.is_err(), "a file cannot grow past its last sector");
}

/// Deterministic bytes for a fixture.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

/// A cooked 2048-byte image as Mode 2 Form 1 raw sectors: sync, address, a data sub-header, the user bytes, then
/// the sealed EDC and parity.
fn wrap_raw(cooked: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(cooked.len() / 2048 * RAW_SIZE);
    for (lba, user) in cooked.as_chunks::<2048>().0.iter().enumerate() {
        let mut sector = [0_u8; RAW_SIZE];
        sector[1..11].fill(0xFF);
        // Sector 0 of the disc is at 00:02:00.
        let frame = lba + 150;
        let (minutes, seconds, frames) = (frame / 4500, frame / 75 % 60, frame % 75);
        sector[12..16].copy_from_slice(&[bcd(minutes), bcd(seconds), bcd(frames), 2]);
        sector[16..24].copy_from_slice(&[0, 0, 0x08, 0, 0, 0, 0x08, 0]);
        sector[24..2072].copy_from_slice(user);
        sector::seal(&mut sector);
        raw.extend_from_slice(&sector);
    }
    raw
}

fn bcd(value: usize) -> u8 {
    (((value / 10) << 4) | (value % 10)) as u8
}
