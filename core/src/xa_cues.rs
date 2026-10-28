// SPDX-License-Identifier: MIT
//! The game's XA cue table: the start/end pairs `fcn.80163744` reads from
//! `SLUS_004.22`, and the battle overlay's voice table that names a cue.
//!
//! Three `.STR` files have a pair array at `[0x8018323C + 4*file]` (file 0
//! `S_XA00`, file 1 `MAGIC00`, file 2 `VOICE`). The scan consumes one u16 per
//! cue and two for an entry with bit 15 set, so a flagged entry starts the next
//! channel's last cue. A cue's first stream sector is `channel + period*start`;
//! its audio sectors are that channel's sectors from `start` to `end`.
//!
//! A magic cue is asked for by `0x1000 | ordinal`, where the ordinal indexes
//! the file and the ability index selects a record in the battle overlay's
//! voice table (`0x800B6178`). The record's second byte is the voice slot the
//! acting character or enemy passes. The overlay is entry 15 of
//! `BIN/BATTLE/BATTLE.EMI`; ability names are the 228 records at `GAME.EMI`
//! archive offset `0x3570C`.

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use emi::Archive;

use crate::catalog::Naming;
use crate::source::Source;

/// The retail executable that holds the pair arrays.
const EXE: &str = "SLUS_004.22";
/// `VA = file offset + EXE_BASE`; pinned by the libsnd pitch table (`wiki: formats/audio#vab-bank`).
const EXE_BASE: u32 = 0x8009_6000;
/// File-number to pair-array pointer.
const ARRAY_TABLE: u32 = 0x8018_323C;
/// S_XA00, MAGIC00 and VOICE.
const FILES: usize = 3;
/// A pair array never holds more than this many entries.
const MAX_ENTRIES: usize = 1024;
/// Interleave period of a file's sectors.
const PERIOD: [u32; FILES] = [8, 16, 16];
/// Game id of the first cue of a file (`0x1000 | ordinal` for MAGIC00, and so on).
const GAME_ID: [u16; FILES] = [0x0000, 0x1000, 0x2000];
/// The battle archive that holds the voice table.
const BATTLE: &str = "BIN/BATTLE/BATTLE.EMI";
/// Load address of the battle overlay (entry 15).
const OVERLAY: u32 = 0x8009_6800;
/// Voice-slot table inside that overlay.
const VOICE_TABLE: usize = 0x1F978;
/// The archive whose ability table names a voice.
const GAME: &str = "BIN/ETC/GAME.EMI";
/// Load address of `GAME.EMI` entry 0.
const ABILITY_RAM: u32 = 0x8019_5800;
/// Ability table inside entry 0's payload (archive offset `0x3570C`).
const ABILITY_TABLE: usize = 0x34F0C;
const ABILITY_COUNT: usize = 228;
const ABILITY_RECORD: usize = 0x14;
const ABILITY_NAME: usize = 12;

/// One cue window of one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cue {
    /// Subheader channel.
    pub channel: u8,
    /// Cue ordinal inside the file; `0x1000 | ordinal` for MAGIC00.
    pub ordinal: u16,
    /// First period of the window.
    pub start: u32,
    /// One past the last period of the window.
    pub end: u32,
    /// Game id of the cue (`0x1000 | ordinal` for MAGIC00).
    pub game_id: u16,
    /// Interleave period of the file.
    pub period: u32,
}

impl Cue {
    /// Stream sector of the cue's first audio sector.
    #[must_use]
    pub fn first_sector(self) -> u32 {
        self.start * self.period + u32::from(self.channel)
    }

    /// Audio sectors the window names.
    #[must_use]
    pub fn wanted_sectors(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// The retail cue table and the magic-voice names, read once per catalog.
#[derive(Debug, Default)]
pub(crate) struct CueTable {
    files: [Vec<Cue>; FILES],
    names: BTreeMap<u16, String>,
}

impl CueTable {
    /// Read the cue arrays from the executable and the voice names from the
    /// battle archive; a source without them gets an empty table, and the
    /// catalog keeps the EOF spans.
    pub fn new(source: &dyn Source, naming: &dyn Naming) -> Self {
        let mut table = Self::default();
        if let Some(bytes) = read(source, EXE) {
            table.files = parse_exe(&bytes);
        }
        table.names = voice_names(source, naming);
        table
    }

    /// The cues of file `index` (0 S_XA00, 1 MAGIC00, 2 VOICE).
    #[must_use]
    pub fn file(&self, index: usize) -> &[Cue] {
        self.files.get(index).map_or(&[], Vec::as_slice)
    }

    /// The name the voice table gives a cue ordinal, when the ability and its
    /// voice slot are known.
    #[must_use]
    pub fn name(&self, ordinal: u16) -> Option<&str> {
        self.names.get(&ordinal).map(String::as_str)
    }
}

/// The file index of a stream stem, or `None` when that stream has no cue array.
#[must_use]
pub(crate) fn file_index(stem: &str) -> Option<usize> {
    if stem.eq_ignore_ascii_case("S_XA00") {
        Some(0)
    } else if stem.eq_ignore_ascii_case("MAGIC00") {
        Some(1)
    } else if stem.eq_ignore_ascii_case("VOICE") {
        Some(2)
    } else {
        None
    }
}

fn read(source: &dyn Source, path: &str) -> Option<Vec<u8>> {
    let blob = source.open(path).ok()?;
    Some(Cow::into_owned(blob.bytes().ok()?))
}

fn payload(bytes: &[u8], wanted: impl Fn(&emi::Entry) -> bool) -> Option<&[u8]> {
    let archive = Archive::parse(bytes).ok()?;
    let index = archive.entries().iter().position(wanted)?;
    archive.payload(bytes, index).ok()
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn va_at(bytes: &[u8], va: u32) -> Option<usize> {
    let at = va.checked_sub(EXE_BASE)? as usize;
    (at < bytes.len()).then_some(at)
}

/// The pair arrays of the three files. A missing or malformed executable gives
/// no cues, which leaves the catalog's EOF spans in place.
fn parse_exe(bytes: &[u8]) -> [Vec<Cue>; FILES] {
    let mut files = [Vec::new(), Vec::new(), Vec::new()];
    if bytes.get(..8) != Some(b"PS-X EXE") {
        return files;
    }
    let mut starts = [0u32; FILES];
    for (index, start) in starts.iter_mut().enumerate() {
        let Some(at) = va_at(bytes, ARRAY_TABLE).and_then(|at| at.checked_add(4 * index)) else { return files };
        let Some(pointer) = u32_at(bytes, at) else { return files };
        *start = pointer;
    }
    for index in 0..FILES {
        let start = starts[index];
        let limit = starts
            .iter()
            .copied()
            .filter(|&other| other > start)
            .min()
            .map_or(MAX_ENTRIES, |next| ((next - start) / 2) as usize);
        let Some(base) = va_at(bytes, start) else { continue };
        let mut values = Vec::with_capacity(limit);
        for at in 0..limit {
            match u16_at(bytes, base + at * 2) {
                Some(value) => values.push(value),
                None => break,
            }
        }
        files[index] = parse_cues(&values, index);
    }
    files
}

/// One u16 per cue, two for a flagged entry; a zero pair ends the list.
fn parse_cues(values: &[u16], file: usize) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut channel = 0u8;
    let mut at = 0;
    while at + 1 < values.len() {
        let (start_entry, end_entry) = (values[at], values[at + 1]);
        if start_entry == 0 && end_entry == 0 {
            break;
        }
        let start = u32::from(start_entry & 0x7FFF);
        let end = u32::from(end_entry & 0x7FFF);
        if end < start {
            break;
        }
        let ordinal = cues.len() as u16;
        cues.push(Cue { channel, ordinal, start, end, game_id: GAME_ID[file] | ordinal, period: PERIOD[file] });
        if start_entry & 0x8000 != 0 {
            channel = channel.saturating_add(1);
            at += 2;
        } else {
            at += 1;
        }
        if cues.len() >= MAX_ENTRIES {
            break;
        }
    }
    cues
}

/// Cue ordinal to `"<ability> voice <slot>"`, from the battle overlay's slot
/// table and the game's ability names. A source without the archives gives no
/// names.
fn voice_names(source: &dyn Source, naming: &dyn Naming) -> BTreeMap<u16, String> {
    let mut names = BTreeMap::new();
    let Some(battle) = read(source, BATTLE) else { return names };
    let Some(overlay) = payload(&battle, |entry| entry.ram_ptr == OVERLAY) else { return names };
    let game = read(source, GAME);
    let abilities = game.as_deref().and_then(|bytes| payload(bytes, |entry| entry.ram_ptr == ABILITY_RAM));
    for slot in 0..=u32::from(u8::MAX) {
        let Some(pointer) = u32_at(overlay, VOICE_TABLE + slot as usize * 4) else { break };
        let Some(mut at) = pointer.checked_sub(OVERLAY).map(|at| at as usize) else { continue };
        let ability = abilities.and_then(|table| ability_name(table, slot as usize));
        let ability = ability.or_else(|| naming.magic_ability(slot as u16).map(String::from));
        let Some(ability) = ability else { continue };
        let ability = ability.as_str();
        while let Some(word) = u32_at(overlay, at) {
            if word == 0xFFFF_FFFF {
                break;
            }
            let cue = (word & 0xFFFF) as u16;
            let voice = ((word >> 16) & 0xFF) as u8;
            names.entry(cue).or_insert_with(|| format!("{ability} voice {voice}"));
            at += 4;
        }
    }
    names
}

/// The name field of one ability record, without its padding. `0xFF` is the
/// name encoding's space; printable ASCII is itself.
fn ability_name(payload: &[u8], index: usize) -> Option<String> {
    if index >= ABILITY_COUNT {
        return None;
    }
    let at = ABILITY_TABLE + index * ABILITY_RECORD;
    let field = payload.get(at..at + ABILITY_NAME)?;
    let end = field.iter().rposition(|&byte| byte != 0).map_or(0, |last| last + 1);
    let mut name = String::with_capacity(end);
    for &byte in &field[..end] {
        match byte {
            0xFF => name.push(' '),
            0x20..=0x7E => name.push(byte as char),
            0x8B => name.push('+'),
            0x8D => name.push(','),
            0x8E => name.push('\''),
            0x8F => name.push(':'),
            _ => {}
        }
    }
    Some(name.trim_end().to_string())
}
