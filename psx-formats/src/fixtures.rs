//! Tiny banks, sequences and sectors for the tests of this crate and the crates built on it.
//!
//! Enabled by the `fixtures` feature; this crate's own tests always have it. Every value is
//! generated here from constants.
//!
//! # Examples
//!
//! ```ignore
//! let (header, body) = psx_formats::fixtures::tiny_bank();
//! let bank = psx_formats::vab::Bank::parse(&header, &body).unwrap();
//! assert!(bank.problems().is_empty());
//! ```

use alloc::vec;
use alloc::vec::Vec;

use crate::adpcm::{FLAG_END, FLAG_LOOP_START, FLAG_REPEAT};
use crate::xa::SECTOR_BYTES;

/// Magic every bank header opens with.
const VAB_MAGIC: &[u8; 4] = b"pBAV";
/// Position of the program table in a bank header.
const PROGRAM_TABLE: usize = 0x20;
/// Bytes of one program record.
const PROGRAM_BYTES: usize = 16;
/// Position of the first tone slot.
const TONE_TABLE: usize = 0x820;
/// Bytes of one tone slot, and the slots one tone block holds.
const TONE_BYTES: usize = 32;
const TONES_PER_BLOCK: usize = 16;
/// Units the size table holds, 2 bytes each.
const SIZE_ENTRIES: usize = 256;

/// A bank with one program (number 1) in tone block 0, one tone answering every note, and one
/// looping two-block sample. Returns the header and the body; the pair breaks no format rule.
///
/// # Examples
///
/// ```ignore
/// let (header, body) = psx_formats::fixtures::tiny_bank();
/// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().vag_count, 1);
/// ```
pub fn tiny_bank() -> (Vec<u8>, Vec<u8>) {
    let size_table = TONE_TABLE + TONES_PER_BLOCK * TONE_BYTES;
    let mut vh = vec![0u8; size_table + SIZE_ENTRIES * 2];
    let mut vb = vec![0u8; 32];
    vh[0..4].copy_from_slice(VAB_MAGIC);
    let file_size = (vh.len() + vb.len()) as u32;
    vh[0x0c..0x10].copy_from_slice(&file_size.to_le_bytes());
    vh[0x12..0x14].copy_from_slice(&1u16.to_le_bytes()); // tone blocks
    vh[0x14..0x16].copy_from_slice(&1u16.to_le_bytes()); // tones
    vh[0x16..0x18].copy_from_slice(&1u16.to_le_bytes()); // samples
    vh[0x18] = 127; // master volume
    vh[0x19] = 64; // master pan
    let program = PROGRAM_TABLE + PROGRAM_BYTES; // program 1
    vh[program] = 1; // tones
    vh[program + 1] = 127; // volume
    vh[program + 4] = 64; // pan
    let tone = TONE_TABLE; // block 0, slot 0
    vh[tone + 2] = 127; // volume
    vh[tone + 3] = 64; // pan
    vh[tone + 4] = 60; // centre note
    vh[tone + 7] = 127; // highest note
    vh[tone + 16..tone + 18].copy_from_slice(&0x000Fu16.to_le_bytes()); // ADSR1
    vh[tone + 20..tone + 22].copy_from_slice(&1u16.to_le_bytes()); // program 1
    vh[tone + 22..tone + 24].copy_from_slice(&1u16.to_le_bytes()); // sample 1
    // Sample 1 is two blocks: 32 bytes are 4 units of 8.
    vh[size_table + 2..size_table + 4].copy_from_slice(&4u16.to_le_bytes());
    vb[0] = 0x0C;
    vb[1] = FLAG_LOOP_START;
    vb[2..16].fill(0x22);
    vb[16] = 0x0C;
    vb[17] = FLAG_END | FLAG_REPEAT;
    vb[18..32].fill(0x22);
    (vh, vb)
}

/// A package with one sequence: program 1 on channel 0, loop markers around one note.
///
/// # Examples
///
/// ```ignore
/// let package = psx_formats::sep::Package::parse(&psx_formats::fixtures::tiny_sep()).unwrap();
/// assert_eq!(package.sequences.len(), 1);
/// ```
pub fn tiny_sep() -> Vec<u8> {
    let events: &[u8] = &[
        0x00, 0xC0, 0x01, // program change 1
        0x00, 0xB0, 99, 20, // loop start
        0x00, 0x90, 60, 100, // note on
        0x30, 0x80, 60, 0, // note off after 48 ticks
        0x00, 0xB0, 99, 30, // loop end
        0x00, 0xFF, 0x2F, 0x00, // end of track
    ];
    let mut out = Vec::new();
    out.extend_from_slice(b"pQES");
    out.extend_from_slice(&[0, 0]); // version
    out.extend_from_slice(&[0, 0]); // id
    out.extend_from_slice(&48u16.to_be_bytes()); // resolution
    out.extend_from_slice(&[0x07, 0xA1, 0x20]); // tempo 500000
    out.extend_from_slice(&[4, 2]); // time signature
    out.extend_from_slice(&(events.len() as u32).to_be_bytes());
    out.extend_from_slice(events);
    out
}

/// A stored XA sector of silence (all-zero nibbles) for `file` and `channel`.
///
/// # Examples
///
/// ```ignore
/// let sector = psx_formats::fixtures::sector(1, 0, 0x04, 0);
/// assert!(psx_formats::xa::is_audio(&sector));
/// ```
pub fn sector(file: u8, channel: u8, submode: u8, coding: u8) -> Vec<u8> {
    let mut sector = vec![0u8; SECTOR_BYTES];
    sector[0] = file;
    sector[1] = channel;
    sector[2] = submode;
    sector[3] = coding;
    sector
}
