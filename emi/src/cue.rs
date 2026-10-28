//! EMI type-8 cue records: one VAB bank's sound-effect cue table.
//!
//! A payload is a whole number of 4-byte records; the loader copies it into one
//! 124-byte chunk (`0x801486A0 + 124 * bank`, 31 records) and the cue
//! dispatcher `0x8015DF18` turns record `index` of bank `bank` — cue id
//! `(bank << 8) | index` — into linked `SsUtKeyOnV`/`SsUtSetDetVVol` calls.
//! [`CueTable::decode`] reads the records and rejects the shapes the loader and
//! the dispatcher cannot handle: a partial record, more records than the chunk
//! holds, and the bits the format reserves. [`CueTable::encode`] writes the
//! records back.

use alloc::vec::Vec;
#[cfg(feature = "std")]
use serde::Serialize;

use crate::error::{Error, Result};

/// One cue record, in bytes.
pub const RECORD_BYTES: usize = 4;
/// Records one bank chunk holds (`0x801486A0`-`0x80148A03`, 7 banks of 124 B).
pub const CHUNK_RECORDS: usize = 31;

/// A cue record with the byte's reserved bits cleared.
const RESERVED_ALIAS: u8 = 0x07;
const RESERVED_VOICE: u8 = 0x7f;
const CUE_VOLUME: u8 = 0x80;
const PROGRAM_MASK: u8 = 0x7f;
const LAYERS_MASK: u8 = 0x60;
const VOICE_MASK: u8 = 0x1f;

/// One cue record: the SPU voice, VAB program and tone a sound effect plays.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(Serialize))]
pub struct Cue {
    /// Bank whose record supplies the voice index: `0` = this bank, `1`-`4` =
    /// that bank. Zero in all 8,608 records; the readers handle `1`-`4`.
    pub alias_bank: u8,
    /// Keep the tone's own volume (`false`) or follow each key-on with
    /// `SsUtSetDetVVol` using the cue's volume (`true`).
    pub use_cue_volume: bool,
    /// VAB program (tone block), `SsUtKeyOnV` argument 3.
    pub program: u8,
    /// Tone slot in that program's 16-slot block, `SsUtKeyOnV` argument 4.
    pub tone: u8,
    /// Four bits the dispatcher copies to voice state and compares with the
    /// previous cue's value; the shipped records hold 10, 11 and 27 other values.
    pub group: u8,
    /// Extra voices keyed on this cue, after the base voice.
    pub layers: u8,
    /// First SPU voice; the layered voices follow it.
    pub voice: u8,
}

impl Cue {
    /// Reads one record.
    fn read(record: &[u8], index: usize) -> Result<Self> {
        let [byte0, byte1, byte2, byte3] = [record[0], record[1], record[2], record[3]];
        if byte0 & !RESERVED_ALIAS != 0 {
            return Err(Error::CueRecord {
                record: index,
                reason: "bits 7-3 of byte 0 are reserved and zero in all 8,608 records",
            });
        }
        if byte3 & !RESERVED_VOICE != 0 {
            return Err(Error::CueRecord {
                record: index,
                reason: "bit 7 of byte 3 is reserved and zero in all 8,608 records",
            });
        }
        Ok(Self {
            alias_bank: byte0 & RESERVED_ALIAS,
            use_cue_volume: byte1 & CUE_VOLUME != 0,
            program: byte1 & PROGRAM_MASK,
            tone: byte2 >> 4,
            group: byte2 & 0x0f,
            layers: (byte3 & LAYERS_MASK) >> 5,
            voice: byte3 & VOICE_MASK,
        })
    }

    /// The record's four bytes.
    pub fn encode(self) -> [u8; RECORD_BYTES] {
        let byte1 = self.program | if self.use_cue_volume { CUE_VOLUME } else { 0 };
        [
            self.alias_bank & RESERVED_ALIAS,
            byte1,
            (self.tone << 4) | (self.group & 0x0f),
            (self.layers << 5) | (self.voice & VOICE_MASK),
        ]
    }

    /// The cue id the dispatcher keys this record with, `(bank << 8) | index`.
    pub fn cue_id(bank: u8, index: usize) -> u16 {
        (u16::from(bank) << 8) | (index as u16 & 0xff)
    }
}

/// One bank's cue records, in record-index order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CueTable {
    records: Vec<Cue>,
}

impl CueTable {
    /// Reads a type-8 payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if !bytes.len().is_multiple_of(RECORD_BYTES) {
            return Err(Error::CueTableSize(bytes.len()));
        }
        let count = bytes.len() / RECORD_BYTES;
        if count > CHUNK_RECORDS {
            return Err(Error::CueTableRecords(count));
        }
        let mut records = Vec::with_capacity(count);
        for (index, record) in bytes.as_chunks::<RECORD_BYTES>().0.iter().enumerate() {
            records.push(Cue::read(record, index)?);
        }
        Ok(Self { records })
    }

    /// The records, in cue-id order.
    pub fn records(&self) -> &[Cue] {
        &self.records
    }

    /// The record at `index`, if the table holds one.
    pub fn record(&self, index: usize) -> Option<&Cue> {
        self.records.get(index)
    }

    /// The records as JSON, one object per cue in cue-id order.
    #[cfg(feature = "std")]
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(&self.records).expect("cue records are serializable")
    }

    /// The payload bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.records.len() * RECORD_BYTES);
        for record in &self.records {
            out.extend_from_slice(&record.encode());
        }
        out
    }
}
