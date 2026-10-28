//! VAB sample banks: the header tables, their validator and their encoder.
//!
//! A bank is a header (`.vh`) with fixed tables, and a body (`.vb`) of SPU
//! ADPCM samples. [`Bank::parse`] keeps every reserved byte, so
//! [`Bank::encode_header`] of the result is the input again;
//! [`Bank::problems`] reports what the format rules out, and [`encode_bank`]
//! builds a fresh header and body pair. The sample data follows psx-spx
//! "SPU ADPCM Samples"; the header layout follows Sony's PlayStation sound
//! library VAB format.
//!
//! # Examples
//!
//! ```
//! use psx_formats::vab::{Bank, encode_bank};
//!
//! let (header, body) = encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
//! let bank = Bank::parse(&header, &body).unwrap();
//! assert_eq!(bank.vag_count, 1);
//! assert_eq!(bank.decode_vag(&body, 1).unwrap(), vec![0i16; 28]);
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::adpcm::{self, BLOCK_BYTES, BLOCK_SAMPLES, FLAG_END, FLAG_LOOP_START, FLAG_REPEAT, State};
use crate::bytes::{i16_le, push_i16_le, push_u16_le, push_u32_le, u16_le, u32_le};
use crate::error::{Error, Result};

/// Bank magic: `"VABp"` read as a little-endian `u32`, so the bytes are the
/// other way round.
const VAB_MAGIC: &[u8; 4] = b"pBAV";
/// Container entry type of a VAB header.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::vab::HEADER_TYPE, 6);
/// ```
pub const HEADER_TYPE: u16 = 6;
/// Container entry type of a VAB body.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::vab::BODY_TYPE, 7);
/// ```
pub const BODY_TYPE: u16 = 7;
/// Position of the program table.
const PROGRAM_TABLE: usize = 0x20;
/// Program records the table holds, one [`PROGRAM_SIZE`]-byte record each.
const PROGRAM_SLOTS: usize = 128;
const PROGRAM_SIZE: usize = 16;
/// Position of the first tone slot.
const TONE_TABLE: usize = 0x820;
/// Tone slots one program block holds.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::vab::TONES_PER_PROGRAM, 16);
/// ```
pub const TONES_PER_PROGRAM: usize = 16;
/// Bytes of one tone slot.
const TONE_SIZE: usize = 32;
/// Units the size-table region holds, whatever `vs` declares.
const SIZE_ENTRIES: usize = 256;
/// Bytes one size-table unit covers.
const VAG_SIZE_UNIT: u32 = 8;
/// Programs a tone may name, the driver's own limit.
const PROGRAM_LIMIT: i16 = 128;

/// One tone slot: the [`TONES_PER_PROGRAM`] slots of every program block.
///
/// # Examples
///
/// ```
/// use psx_formats::vab::Tone;
///
/// let tone = Tone { vag: 1, program: 1, ..Tone::default() };
/// assert!(tone.is_live() && tone.is_usable(1));
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Tone {
    /// Program block holding the slot.
    pub storage_block: u8,
    /// Slot within that block.
    pub tone_slot: u8,
    /// Program the tone plays for.
    pub program: i16,
    /// Voice-stealing priority.
    pub priority: u8,
    /// Mode flags; bit 2 ([`Tone::MODE_REVERB`]) marks the reverb send.
    pub mode: u8,
    /// Tone volume.
    pub vol: u8,
    /// Tone pan, 64 is centre.
    pub pan: u8,
    /// Centre note of the sample.
    pub center_note: u8,
    /// Fine tune ("shift") of the centre note.
    pub shift: u8,
    /// Lowest note the tone answers.
    pub min_note: u8,
    /// Highest note the tone answers.
    pub max_note: u8,
    /// Vibrato width.
    pub vibrato_width: u8,
    /// Vibrato time.
    pub vibrato_time: u8,
    /// Portamento width.
    pub portamento_width: u8,
    /// Portamento time.
    pub portamento_time: u8,
    /// Pitch-bend range down, in semitones.
    pub pitch_bend_min: u8,
    /// Pitch-bend range up, in semitones.
    pub pitch_bend_max: u8,
    /// Volume of the program the tone names; 0 when it names none.
    pub program_vol: u8,
    /// Pan of the program the tone names; 0 when it names none.
    pub program_pan: u8,
    /// Priority of the program the tone names; 0 when it names none.
    pub program_priority: u8,
    /// `ADSR1` register.
    pub adsr1: u16,
    /// `ADSR2` register.
    pub adsr2: u16,
    /// 1-based VAG index; 0 marks a slot that names no sample.
    pub vag: i16,
    /// Reserved byte of the slot at offset 14, kept for the encoder.
    pub reserved1: u8,
    /// Reserved byte of the slot at offset 15, kept for the encoder.
    pub reserved2: u8,
    /// Reserved words at the end of the slot, kept for the encoder.
    pub reserved: [i16; 4],
}

impl Tone {
    /// Mode bit 2: the keyed voice also goes to the reverb unit.
    ///
    /// # Examples
    ///
    /// ```
    /// let tone = psx_formats::vab::Tone { mode: psx_formats::vab::Tone::MODE_REVERB, ..Default::default() };
    /// assert!(tone.reverb_send());
    /// ```
    pub const MODE_REVERB: u8 = 0x04;

    /// Whether the voice is sent to the reverb.
    ///
    /// # Examples
    ///
    /// ```
    /// let tone = psx_formats::vab::Tone { mode: 0x04, ..Default::default() };
    /// assert!(tone.reverb_send());
    /// ```
    pub fn reverb_send(&self) -> bool {
        self.mode & Self::MODE_REVERB != 0
    }

    /// A slot that names a sample.
    ///
    /// # Examples
    ///
    /// ```
    /// let tone = psx_formats::vab::Tone { vag: 2, ..Default::default() };
    /// assert!(tone.is_live());
    /// ```
    pub fn is_live(&self) -> bool {
        self.vag != 0
    }

    /// A tone the library's parser keeps: live, inside the size table's range,
    /// and naming a program within the driver's limit.
    ///
    /// # Examples
    ///
    /// ```
    /// let tone = psx_formats::vab::Tone { vag: 2, program: 1, ..Default::default() };
    /// assert!(tone.is_usable(2));
    /// assert!(!tone.is_usable(1));
    /// ```
    pub fn is_usable(&self, vag_count: u16) -> bool {
        self.is_live()
            && self.vag >= 1
            && self.vag <= vag_count as i16
            && self.program >= 0
            && self.program < PROGRAM_LIMIT
    }
}

/// One program record; slot 0 is the bank's dummy program.
///
/// # Examples
///
/// ```
/// let program = psx_formats::vab::Program { tones: 2, ..Default::default() };
/// assert!(program.is_playable());
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Program {
    /// Tones the program declares; 0 marks a slot the bank does not play.
    pub tones: u8,
    /// Program volume, `0..=127`.
    pub vol: u8,
    /// Program priority.
    pub priority: u8,
    /// Program mode.
    pub mode: u8,
    /// Program pan, `0..=127`, 64 is centre.
    pub pan: u8,
    /// Reserved byte of the record, kept for the encoder.
    pub reserved0: u8,
    /// Program attribute word.
    pub attr: i16,
    /// Reserved word of the record, kept for the encoder.
    pub reserved1: u32,
    /// Reserved word of the record, kept for the encoder.
    pub reserved2: u32,
}

impl Program {
    /// A slot the bank plays: one that declares tones.
    ///
    /// # Examples
    ///
    /// ```
    /// let program = psx_formats::vab::Program::default();
    /// assert!(!program.is_playable());
    /// ```
    pub fn is_playable(&self) -> bool {
        self.tones != 0
    }
}

/// One VAG: its range in the body, and what the body's block flags say of it.
///
/// # Examples
///
/// ```
/// let vag = psx_formats::vab::Vag { size: 16, ..Default::default() };
/// assert!(!vag.is_empty());
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Vag {
    /// 1-based VAG number, as tones reference it.
    pub index: u32,
    /// Byte offset in the body.
    pub offset: u32,
    /// Byte size.
    pub size: u32,
    /// Samples a one-pass decode produces.
    pub frames: u32,
    /// An end flag was found.
    pub has_end: bool,
    /// The sample loops (end flag with the repeat bit).
    pub has_loop: bool,
    /// First looped sample, when `has_loop`.
    pub loop_start_frame: u32,
    /// One past the last looped sample, when `has_loop`.
    pub loop_end_frame: u32,
}

impl Vag {
    /// A range shorter than one ADPCM block: the null sample.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(psx_formats::vab::Vag::default().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        (self.size as usize) < BLOCK_BYTES
    }

    /// Byte address the loop restarts from: the loop-start block, else the start.
    ///
    /// # Examples
    ///
    /// ```
    /// let vag = psx_formats::vab::Vag { offset: 0, size: 32, has_loop: true, loop_start_frame: 28, ..Default::default() };
    /// assert_eq!(vag.repeat_offset(), 16);
    /// ```
    pub fn repeat_offset(&self) -> u32 {
        if self.has_loop {
            self.offset + (self.loop_start_frame / BLOCK_SAMPLES as u32) * BLOCK_BYTES as u32
        } else {
            self.offset
        }
    }
}

/// A parsed bank: the header tables and the body they describe.
///
/// # Examples
///
/// ```
/// use psx_formats::vab::{Bank, encode_bank};
///
/// let (header, body) = encode_bank(7, &[], &[]).unwrap();
/// let bank = Bank::parse(&header, &body).unwrap();
/// assert_eq!((bank.id, bank.problems().len()), (7, 0));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bank {
    /// Header version field.
    pub version: u32,
    /// Bank size the header declares.
    pub file_size: u32,
    /// Bank id.
    pub id: u32,
    /// Reserved word at offset `0x10`, kept for the encoder.
    pub reserved0: u16,
    /// As stored: counts the tone blocks, not the program table's slots.
    pub program_count: u16,
    /// Tone count the header declares.
    pub declared_tones: u16,
    /// Slots that name a usable tone, the library's count.
    pub tone_count: u16,
    /// Sample count (entry 0 of `vags` is the null sample).
    pub vag_count: u16,
    /// Bank master volume.
    pub master_vol: u8,
    /// Bank master pan.
    pub master_pan: u8,
    /// First bank attribute byte.
    pub attr1: u8,
    /// Second bank attribute byte.
    pub attr2: u8,
    /// Reserved word at offset `0x1c`, kept for the encoder.
    pub reserved1: u32,
    /// The whole program table, 128 records.
    pub programs: Vec<Program>,
    /// Every tone slot of every block, in block and slot order.
    pub tones: Vec<Tone>,
    /// Index 0 is the null sample; the rest follow the size table.
    pub vags: Vec<Vag>,
    /// The size-table region as stored, 256 units: stale units
    /// past `vag_count` included, because the encoder writes them back. The
    /// `vags` sizes view this table.
    pub size_table: Vec<u16>,
    body_len: usize,
}

/// What a bank breaks of the format.
///
/// # Examples
///
/// ```
/// use psx_formats::vab::{Bank, Problem, encode_bank};
///
/// let (mut header, body) = encode_bank(0, &[], &[]).unwrap();
/// header[0x0c..0x10].copy_from_slice(&0u32.to_le_bytes());
/// let bank = Bank::parse(&header, &body).unwrap();
/// assert!(matches!(bank.problems()[0], Problem::FileSize { .. }));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Problem {
    /// `file_size` is not the header plus the body.
    FileSize {
        /// Size the header declares.
        declared: u32,
        /// Header plus body, in bytes.
        actual: u64,
    },
    /// `program_count` is not the number of records that declare tones.
    ProgramCount {
        /// Count the header declares.
        declared: u16,
        /// Records that declare tones.
        playable: usize,
    },
    /// `declared_tones` is not the sum of the records' tone counts.
    ToneCount {
        /// Count the header declares.
        declared: u16,
        /// Sum of the records' tone counts.
        playable: u32,
    },
    /// A live slot names a program other than the block's own.
    ToneBlockProgram {
        /// The tone block.
        block: usize,
        /// Program the slot names.
        program: i16,
        /// Program the block belongs to.
        expected: usize,
    },
    /// A block's live slots are not the ones its program declares.
    ToneBlockCount {
        /// The tone block.
        block: usize,
        /// Tones the program declares.
        declared: u8,
        /// Live slots the block holds.
        live: usize,
    },
    /// A live slot names a VAG the size table does not cover.
    ToneVag {
        /// The tone block.
        block: usize,
        /// The slot in the block.
        slot: usize,
        /// VAG the slot names.
        vag: i16,
        /// Samples the bank declares.
        vag_count: u16,
    },
    /// The size table covers a different number of bytes than the body holds.
    VagSizeTotal {
        /// Bytes the size table covers.
        declared: u64,
        /// Bytes the body holds.
        body: u64,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileSize { declared, actual } => {
                write!(f, "file_size is {declared}, the header and body are {actual} bytes")
            }
            Self::ProgramCount { declared, playable } => {
                write!(f, "program_count is {declared}, {playable} records declare tones")
            }
            Self::ToneCount { declared, playable } => {
                write!(f, "declared_tones is {declared}, the records declare {playable} tones")
            }
            Self::ToneBlockProgram { block, program, expected } => {
                write!(f, "block {block} holds a live slot for program {program}, not {expected}")
            }
            Self::ToneBlockCount { block, declared, live } => {
                write!(f, "block {block} declares {declared} tones but holds {live} live slots")
            }
            Self::ToneVag { block, slot, vag, vag_count } => {
                write!(f, "block {block} slot {slot} names VAG {vag}, the bank has {vag_count}")
            }
            Self::VagSizeTotal { declared, body } => {
                write!(f, "the size table covers {declared} bytes, the body holds {body}")
            }
        }
    }
}

impl Bank {
    /// Parses a header and its body.
    ///
    /// The fixed tables give one header size per `program_count`, and the body
    /// is kept as it stands, so [`Bank::encode_header`] of the result is the
    /// input again.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::vab::{Bank, encode_bank};
    ///
    /// let (header, body) = encode_bank(0, &[], &[]).unwrap();
    /// assert!(Bank::parse(&header, &body).is_ok());
    /// assert!(Bank::parse(b"pBAV", &body).is_err());
    /// ```
    pub fn parse(vh: &[u8], vb: &[u8]) -> Result<Self> {
        if vh.len() < PROGRAM_TABLE {
            return Err(Error::VabHeaderSize { length: vh.len(), expected: PROGRAM_TABLE });
        }
        if &vh[..4] != VAB_MAGIC {
            return Err(Error::VabBadMagic);
        }

        let program_count = u16_le(vh, 0x12);
        let expected = TONE_TABLE + tone_bytes(program_count) + SIZE_ENTRIES * 2;
        if vh.len() != expected {
            return Err(Error::VabHeaderSize { length: vh.len(), expected });
        }

        let programs: Vec<Program> = (0..PROGRAM_SLOTS).map(|slot| read_program(vh, slot)).collect();
        let tones = read_tones(vh, program_count, &programs);
        let vag_count = u16_le(vh, 0x16);
        let size_table: Vec<u16> =
            (0..SIZE_ENTRIES).map(|entry| u16_le(vh, TONE_TABLE + tone_bytes(program_count) + entry * 2)).collect();
        let mut vags = read_vag_ranges(&size_table, vag_count);
        scan_vag_blocks(vb, &mut vags);

        let tone_count = tones.iter().filter(|tone| tone.is_usable(vag_count)).count();

        Ok(Self {
            version: u32_le(vh, 4),
            file_size: u32_le(vh, 0x0c),
            id: u32_le(vh, 8),
            reserved0: u16_le(vh, 0x10),
            program_count,
            declared_tones: u16_le(vh, 0x14),
            tone_count: tone_count as u16,
            vag_count,
            master_vol: vh[0x18],
            master_pan: vh[0x19],
            attr1: vh[0x1a],
            attr2: vh[0x1b],
            reserved1: u32_le(vh, 0x1c),
            programs,
            tones,
            vags,
            size_table,
            body_len: vb.len(),
        })
    }

    /// The bytes of this model's header; [`Bank::parse`] of them gives it back.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::vab::{Bank, encode_bank};
    ///
    /// let (header, body) = encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(Bank::parse(&header, &body).unwrap().encode_header(), header);
    /// ```
    pub fn encode_header(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header_size());
        out.extend_from_slice(VAB_MAGIC);
        push_u32_le(&mut out, self.version);
        push_u32_le(&mut out, self.id);
        push_u32_le(&mut out, self.file_size);
        push_u16_le(&mut out, self.reserved0);
        push_u16_le(&mut out, self.program_count);
        push_u16_le(&mut out, self.declared_tones);
        push_u16_le(&mut out, self.vag_count);
        out.extend_from_slice(&[self.master_vol, self.master_pan, self.attr1, self.attr2]);
        push_u32_le(&mut out, self.reserved1);

        for program in &self.programs {
            push_program(&mut out, program);
        }
        for tone in &self.tones {
            push_tone(&mut out, tone);
        }
        for unit in &self.size_table {
            push_u16_le(&mut out, *unit);
        }
        out
    }

    /// The tones the sequencer can play: live, naming a sample the size table covers and a program
    /// below the driver's limit. The parser's tone list, in block and slot order.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().usable_tones().count(), 0);
    /// ```
    pub fn usable_tones(&self) -> impl Iterator<Item = &Tone> {
        self.tones.iter().filter(|tone| tone.is_usable(self.vag_count))
    }

    /// Programs that at least one usable tone answers, one bit each.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().program_mask(), 0);
    /// ```
    pub fn program_mask(&self) -> u128 {
        self.usable_tones().fold(0, |mask, tone| mask | 1u128 << (tone.program & 0x7f))
    }

    /// The sample with 1-based number `index`.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
    /// let bank = psx_formats::vab::Bank::parse(&header, &body).unwrap();
    /// assert_eq!(bank.vag(1).unwrap().size, 16);
    /// assert!(bank.vag(2).is_err());
    /// ```
    pub fn vag(&self, index: u32) -> Result<&Vag> {
        if index == 0 || index as usize >= self.vags.len() {
            return Err(Error::VagOutOfRange { index, count: self.vag_count });
        }
        Ok(&self.vags[index as usize])
    }

    /// The raw ADPCM blocks of sample `index` inside `body`, the body this bank was parsed with.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
    /// let bank = psx_formats::vab::Bank::parse(&header, &body).unwrap();
    /// assert_eq!(bank.vag_bytes(&body, 1).unwrap().len(), 16);
    /// ```
    pub fn vag_bytes<'a>(&self, body: &'a [u8], index: u32) -> Result<&'a [u8]> {
        let vag = self.vag(index)?;
        if vag.is_empty() {
            return Err(Error::VagEmpty(index));
        }
        let (offset, size) = (vag.offset as usize, vag.size as usize);
        if offset > body.len() || size > body.len() - offset {
            return Err(Error::VagOutsideBody(index));
        }
        Ok(&body[offset..offset + size])
    }

    /// Decodes one pass of sample `index` to mono PCM.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
    /// let bank = psx_formats::vab::Bank::parse(&header, &body).unwrap();
    /// assert_eq!(bank.decode_vag(&body, 1).unwrap(), vec![0i16; 28]);
    /// ```
    pub fn decode_vag(&self, body: &[u8], index: u32) -> Result<Vec<i16>> {
        let blocks = self.vag_bytes(body, index)?;
        let frames = self.vags[index as usize].frames as usize;
        let mut pcm = alloc::vec![0i16; frames];
        let mut state = State::default();
        for (block, out) in blocks.as_chunks::<BLOCK_BYTES>().0.iter().zip(pcm.as_chunks_mut::<BLOCK_SAMPLES>().0) {
            adpcm::decode_block(block, out, &mut state);
        }
        Ok(pcm)
    }

    /// Bytes of the body this bank was parsed with.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().body_len(), 16);
    /// ```
    pub fn body_len(&self) -> usize {
        self.body_len
    }

    /// Bytes [`Bank::encode_header`] produces for this model.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().header_size(), header.len());
    /// ```
    pub fn header_size(&self) -> usize {
        TONE_TABLE + self.tones.len() * TONE_SIZE + SIZE_ENTRIES * 2
    }

    /// The records that declare tones, in table order: the `n`th of them owns
    /// tone block `n`.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().playable_programs().count(), 0);
    /// ```
    pub fn playable_programs(&self) -> impl Iterator<Item = (usize, &Program)> {
        self.programs.iter().enumerate().filter(|(_, program)| program.is_playable())
    }

    /// One slice per program block, [`TONES_PER_PROGRAM`] slots each.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert_eq!(psx_formats::vab::Bank::parse(&header, &body).unwrap().tone_blocks().count(), 0);
    /// ```
    pub fn tone_blocks(&self) -> impl Iterator<Item = &[Tone]> {
        self.tones.as_chunks::<TONES_PER_PROGRAM>().0.iter().map(|block| block.as_slice())
    }

    /// What the bank breaks of the format; empty for a well-formed bank.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// assert!(psx_formats::vab::Bank::parse(&header, &body).unwrap().problems().is_empty());
    /// ```
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();

        let actual = self.header_size() as u64 + self.body_len as u64;
        if u64::from(self.file_size) != actual {
            problems.push(Problem::FileSize { declared: self.file_size, actual });
        }

        let playable: Vec<usize> = self.playable_programs().map(|(slot, _program)| slot).collect();
        if playable.len() != usize::from(self.program_count) {
            problems.push(Problem::ProgramCount { declared: self.program_count, playable: playable.len() });
        }
        let declared_tones: u32 = self.playable_programs().map(|(_slot, program)| u32::from(program.tones)).sum();
        if u32::from(self.declared_tones) != declared_tones {
            problems.push(Problem::ToneCount { declared: self.declared_tones, playable: declared_tones });
        }

        for (block, tones) in self.tone_blocks().enumerate() {
            let Some(&program_slot) = playable.get(block) else {
                break;
            };
            let declared = self.programs[program_slot].tones;
            let mut live = 0;
            for (slot, tone) in tones.iter().enumerate() {
                if !tone.is_live() {
                    continue;
                }
                live += 1;
                if tone.program != program_slot as i16 {
                    problems.push(Problem::ToneBlockProgram { block, program: tone.program, expected: program_slot });
                }
                if tone.vag < 1 || tone.vag > self.vag_count as i16 {
                    problems.push(Problem::ToneVag { block, slot, vag: tone.vag, vag_count: self.vag_count });
                }
            }
            if live != usize::from(declared) {
                problems.push(Problem::ToneBlockCount { block, declared, live });
            }
        }

        let total: u64 = self.vags.iter().map(|vag| u64::from(vag.size)).sum();
        if total != self.body_len as u64 {
            problems.push(Problem::VagSizeTotal { declared: total, body: self.body_len as u64 });
        }

        problems
    }

    /// The bank as one line per fact: the header counts, then every program
    /// record, tone slot and VAG. Useful to diff two parses of the same bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// let (header, body) = psx_formats::vab::encode_bank(0, &[], &[]).unwrap();
    /// let dump = psx_formats::vab::Bank::parse(&header, &body).unwrap().dump();
    /// assert!(dump.starts_with("vab version="));
    /// ```
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "vab version={} file_size={} id={} reserved0={} program_count={} \
             declared_tones={} tone_count={} vag_count={} master_vol={} master_pan={} \
             attr1={} attr2={} reserved1={}\n",
            self.version,
            self.file_size,
            self.id,
            self.reserved0,
            self.program_count,
            self.declared_tones,
            self.tone_count,
            self.vag_count,
            self.master_vol,
            self.master_pan,
            self.attr1,
            self.attr2,
            self.reserved1,
        ));
        for (slot, program) in self.programs.iter().enumerate() {
            out.push_str(&format!(
                "prog {slot} tones={} vol={} priority={} pan={}\n",
                program.tones, program.vol, program.priority, program.pan,
            ));
        }
        for tone in &self.tones {
            out.push_str(&format!(
                "tone {} {} program={} priority={} mode={} vol={} pan={} center_note={} \
                 shift={} min_note={} max_note={} vibrato_width={} vibrato_time={} \
                 portamento_width={} portamento_time={} pitch_bend_min={} pitch_bend_max={} \
                 program_vol={} program_pan={} program_priority={} adsr1={} adsr2={} vag={}\n",
                tone.storage_block,
                tone.tone_slot,
                tone.program,
                tone.priority,
                tone.mode,
                tone.vol,
                tone.pan,
                tone.center_note,
                tone.shift,
                tone.min_note,
                tone.max_note,
                tone.vibrato_width,
                tone.vibrato_time,
                tone.portamento_width,
                tone.portamento_time,
                tone.pitch_bend_min,
                tone.pitch_bend_max,
                tone.program_vol,
                tone.program_pan,
                tone.program_priority,
                tone.adsr1,
                tone.adsr2,
                tone.vag,
            ));
        }
        for vag in &self.vags {
            out.push_str(&format!(
                "vag {} offset={} size={} frames={} has_end={} has_loop={} \
                 loop_start_frame={} loop_end_frame={}\n",
                vag.index,
                vag.offset,
                vag.size,
                vag.frames,
                u8::from(vag.has_end),
                u8::from(vag.has_loop),
                vag.loop_start_frame,
                vag.loop_end_frame,
            ));
        }
        out
    }
}

/// One program of a bank for [`encode_bank`]: its record and the tone slots its block holds, in order.
///
/// # Examples
///
/// ```
/// use psx_formats::vab::{BankProgram, Program, Tone};
///
/// let program = BankProgram {
///     number: 1,
///     record: Program { tones: 1, ..Program::default() },
///     tones: vec![Tone { vag: 1, program: 1, ..Tone::default() }],
/// };
/// assert_eq!(program.record.tones, program.tones.len() as u8);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankProgram {
    /// The program number. Numbers ascend across the bank's programs.
    pub number: u8,
    /// The program record. Its `tones` count is the number of tone slots the block fills.
    pub record: Program,
    /// The filled tone slots, in order: at most [`TONES_PER_PROGRAM`]. The rest of the block is empty.
    pub tones: Vec<Tone>,
}

/// The version field every bank the writer produces declares.
const VAB_VERSION: u32 = 7;
/// Master volume and pan of a written bank: full volume, centred.
const BANK_VOLUME: u8 = 127;
const BANK_PAN: u8 = 64;

/// Writes a bank: the header (`.vh`) and the body (`.vb`) that the header describes.
///
/// `programs` are the playable programs in ascending program number. Program block `i` holds `programs[i]`,
/// the order in which [`Bank::playable_programs`] pairs with [`Bank::tone_blocks`]. `vags[i]` is the ADPCM of
/// VAG `i + 1`, a whole number of 16-byte blocks, and the body holds the VAGs in order. The header and body
/// parse back with [`Bank::parse`] to the same records, tones and VAG ranges.
///
/// # Examples
///
/// ```
/// use psx_formats::vab::{Bank, encode_bank};
///
/// let (header, body) = encode_bank(0, &[], &[vec![0u8; 16]]).unwrap();
/// let bank = Bank::parse(&header, &body).unwrap();
/// assert_eq!(bank.body_len(), 16);
/// assert!(bank.problems().is_empty());
/// ```
pub fn encode_bank(id: u32, programs: &[BankProgram], vags: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut previous: Option<u8> = None;
    for program in programs {
        if usize::from(program.number) >= PROGRAM_SLOTS || previous.is_some_and(|last| last >= program.number) {
            return Err(Error::VabEncode("program numbers must ascend and stay below 128"));
        }
        if program.tones.len() > TONES_PER_PROGRAM || usize::from(program.record.tones) != program.tones.len() {
            return Err(Error::VabEncode("a program declares a tone count other than its filled tone slots"));
        }
        previous = Some(program.number);
    }
    if vags.len() >= SIZE_ENTRIES {
        return Err(Error::VabEncode("a bank holds at most 255 VAGs"));
    }
    let mut units = Vec::with_capacity(vags.len());
    for vag in vags {
        if vag.is_empty() || vag.len() % BLOCK_BYTES != 0 {
            return Err(Error::VabEncode("a VAG is not a whole number of ADPCM blocks"));
        }
        units.push(
            u16::try_from(vag.len() / VAG_SIZE_UNIT as usize).map_err(|_| Error::VabEncode("a VAG is over 512 KiB"))?,
        );
    }
    let body = vags.concat();

    let mut header = Vec::with_capacity(TONE_TABLE + programs.len() * TONES_PER_PROGRAM * TONE_SIZE + SIZE_ENTRIES * 2);
    header.extend_from_slice(VAB_MAGIC);
    push_u32_le(&mut header, VAB_VERSION);
    push_u32_le(&mut header, id);
    push_u32_le(&mut header, 0);
    push_u16_le(&mut header, 0);
    push_u16_le(&mut header, programs.len() as u16);
    let declared: usize = programs.iter().map(|program| usize::from(program.record.tones)).sum();
    push_u16_le(&mut header, declared as u16);
    push_u16_le(&mut header, vags.len() as u16);
    header.extend_from_slice(&[BANK_VOLUME, BANK_PAN, 0, 0]);
    push_u32_le(&mut header, 0);
    for number in 0..PROGRAM_SLOTS {
        let record = programs
            .iter()
            .find(|program| usize::from(program.number) == number)
            .map(|program| program.record)
            .unwrap_or_default();
        push_program(&mut header, &record);
    }
    for program in programs {
        for slot in 0..TONES_PER_PROGRAM {
            push_tone(&mut header, program.tones.get(slot).unwrap_or(&Tone::default()));
        }
    }
    for entry in 0..SIZE_ENTRIES {
        let unit = if entry == 0 { 0 } else { units.get(entry - 1).copied().unwrap_or(0) };
        push_u16_le(&mut header, unit);
    }
    // The header declares the size of the whole bank: header and body.
    let file_size = u32::try_from(header.len() + body.len()).map_err(|_| Error::VabEncode("the bank is over 4 GiB"))?;
    header[0x0c..0x10].copy_from_slice(&file_size.to_le_bytes());
    Ok((header, body))
}

/// One program record, 16 bytes.
fn push_program(out: &mut Vec<u8>, program: &Program) {
    out.push(program.tones);
    out.push(program.vol);
    out.push(program.priority);
    out.push(program.mode);
    out.push(program.pan);
    out.push(program.reserved0);
    push_i16_le(out, program.attr);
    push_u32_le(out, program.reserved1);
    push_u32_le(out, program.reserved2);
}

/// One tone slot, 32 bytes.
fn push_tone(out: &mut Vec<u8>, tone: &Tone) {
    out.push(tone.priority);
    out.push(tone.mode);
    out.push(tone.vol);
    out.push(tone.pan);
    out.push(tone.center_note);
    out.push(tone.shift);
    out.push(tone.min_note);
    out.push(tone.max_note);
    out.push(tone.vibrato_width);
    out.push(tone.vibrato_time);
    out.push(tone.portamento_width);
    out.push(tone.portamento_time);
    out.push(tone.pitch_bend_min);
    out.push(tone.pitch_bend_max);
    out.push(tone.reserved1);
    out.push(tone.reserved2);
    push_u16_le(out, tone.adsr1);
    push_u16_le(out, tone.adsr2);
    push_i16_le(out, tone.program);
    push_i16_le(out, tone.vag);
    for reserved in tone.reserved {
        push_i16_le(out, reserved);
    }
}

fn tone_bytes(program_count: u16) -> usize {
    usize::from(program_count) * TONES_PER_PROGRAM * TONE_SIZE
}

/// One record of the fixed program table.
fn read_program(vh: &[u8], slot: usize) -> Program {
    let at = PROGRAM_TABLE + slot * PROGRAM_SIZE;
    Program {
        tones: vh[at],
        vol: vh[at + 1],
        priority: vh[at + 2],
        mode: vh[at + 3],
        pan: vh[at + 4],
        reserved0: vh[at + 5],
        attr: i16_le(vh, at + 6),
        reserved1: u32_le(vh, at + 8),
        reserved2: u32_le(vh, at + 12),
    }
}

/// Every tone slot of every block the header declares, with the attributes of
/// the program each slot names folded in.
fn read_tones(vh: &[u8], program_count: u16, programs: &[Program]) -> Vec<Tone> {
    let mut tones = Vec::with_capacity(tone_bytes(program_count) / TONE_SIZE);
    for block in 0..usize::from(program_count) {
        for slot in 0..TONES_PER_PROGRAM {
            let at = TONE_TABLE + (block * TONES_PER_PROGRAM + slot) * TONE_SIZE;
            let program = i16_le(vh, at + 20);
            let attrs = match usize::try_from(program) {
                Ok(index) if index < programs.len() => Some(&programs[index]),
                _ => None,
            };
            let (program_vol, program_pan, program_priority) = match attrs {
                Some(attrs) => (attrs.vol, attrs.pan, attrs.priority),
                None => (0, 0, 0),
            };
            tones.push(Tone {
                storage_block: block as u8,
                tone_slot: slot as u8,
                program,
                priority: vh[at],
                mode: vh[at + 1],
                vol: vh[at + 2],
                pan: vh[at + 3],
                center_note: vh[at + 4],
                shift: vh[at + 5],
                min_note: vh[at + 6],
                max_note: vh[at + 7],
                vibrato_width: vh[at + 8],
                vibrato_time: vh[at + 9],
                portamento_width: vh[at + 10],
                portamento_time: vh[at + 11],
                pitch_bend_min: vh[at + 12],
                pitch_bend_max: vh[at + 13],
                program_vol,
                program_pan,
                program_priority,
                adsr1: u16_le(vh, at + 16),
                adsr2: u16_le(vh, at + 18),
                vag: i16_le(vh, at + 22),
                reserved1: vh[at + 14],
                reserved2: vh[at + 15],
                reserved: [i16_le(vh, at + 24), i16_le(vh, at + 26), i16_le(vh, at + 28), i16_le(vh, at + 30)],
            });
        }
    }
    tones
}

/// VAG ranges: the size table accumulates, and index 0 is the null sample.
fn read_vag_ranges(size_table: &[u16], vag_count: u16) -> Vec<Vag> {
    let mut vags = Vec::with_capacity(usize::from(vag_count) + 1);
    vags.push(Vag::default());
    let mut cursor = 0;
    for index in 1..=u32::from(vag_count) {
        let size = u32::from(size_table[index as usize]) * VAG_SIZE_UNIT;
        vags.push(Vag { index, offset: cursor, size, ..Vag::default() });
        cursor = cursor.saturating_add(size);
    }
    vags
}

/// Block flags give each range its end, its loop points, and the length a
/// one-pass decode produces; a range outside the body keeps zeroes.
fn scan_vag_blocks(vb: &[u8], vags: &mut [Vag]) {
    for vag in vags.iter_mut() {
        let start = vag.offset as usize;
        let size = vag.size as usize;
        if vag.is_empty() || start > vb.len() || size > vb.len() - start {
            continue;
        }

        let region = &vb[start..start + size];
        let blocks = region.len() / BLOCK_BYTES;
        let mut decoded = blocks;
        let mut loop_block = 0;
        let mut end_block = 0;
        let mut has_end = false;
        let mut has_loop = false;

        for block in 0..blocks {
            let flags = region[block * BLOCK_BYTES + 1];
            if flags & FLAG_LOOP_START != 0 {
                loop_block = block;
                has_loop = true;
            }
            if flags & FLAG_END != 0 {
                has_end = true;
                decoded = block + 1;
                end_block = block;
                if flags & FLAG_REPEAT == 0 {
                    has_loop = false;
                }
                break;
            }
        }
        if !has_end {
            has_loop = false;
        }

        vag.has_end = has_end;
        vag.has_loop = has_loop;
        vag.frames = decoded as u32 * BLOCK_SAMPLES as u32;
        if has_loop {
            vag.loop_start_frame = loop_block as u32 * BLOCK_SAMPLES as u32;
            vag.loop_end_frame = (end_block as u32 + 1) * BLOCK_SAMPLES as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tiny_bank;

    #[test]
    fn parses_tone_program_and_loop() {
        let (vh, vb) = tiny_bank();
        let vab = Bank::parse(&vh, &vb).unwrap();
        let tones: Vec<_> = vab.usable_tones().collect();
        assert_eq!(tones.len(), 1);
        assert_eq!(tones[0].program, 1);
        assert_eq!(tones[0].program_vol, 127);
        let vag = vab.vag(1).unwrap();
        assert_eq!((vag.frames, vag.has_loop, vag.loop_start_frame, vag.loop_end_frame), (56, true, 0, 56));
        assert_eq!(vab.program_mask(), 1 << 1);
        assert_eq!(vab.decode_vag(&vb, 1).unwrap()[..3], [2, 2, 2]);
        assert!(vab.vag(0).is_err() && vab.vag(2).is_err());
    }

    #[test]
    fn a_fixture_bank_breaks_no_rule_and_encodes_back() {
        let (vh, vb) = tiny_bank();
        let vab = Bank::parse(&vh, &vb).unwrap();
        assert_eq!(vab.problems(), []);
        assert_eq!(vab.encode_header(), vh);
        assert_eq!(vab.body_len(), vb.len());
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(Bank::parse(&[0u8; 16], &[]).is_err());
        let (mut vh, vb) = tiny_bank();
        vh[0] = 0;
        assert_eq!(Bank::parse(&vh, &vb), Err(Error::VabBadMagic));
        let (vh, vb) = tiny_bank();
        assert!(matches!(Bank::parse(&vh[..TONE_TABLE + 10], &vb), Err(Error::VabHeaderSize { .. })));
    }

    #[test]
    fn an_encoded_bank_parses_back_to_its_playable_programs_tones_and_vags() {
        let (header, body) = tiny_bank();
        let vab = Bank::parse(&header, &body).unwrap();
        let programs: Vec<BankProgram> = vab
            .playable_programs()
            .zip(vab.tone_blocks())
            .map(|((number, record), block)| BankProgram {
                number: number as u8,
                record: *record,
                tones: block.iter().take(usize::from(record.tones)).copied().collect(),
            })
            .collect();
        let vags: Vec<Vec<u8>> =
            (1..=vab.vag_count as u32).map(|index| vab.vag_bytes(&body, index).unwrap().to_vec()).collect();
        let (again_header, again_body) = encode_bank(vab.id, &programs, &vags).unwrap();
        let again = Bank::parse(&again_header, &again_body).unwrap();
        assert!(again.problems().is_empty(), "{:?}", again.problems());
        assert_eq!(again.programs[1], vab.programs[1]);
        assert_eq!(again.tones[0], vab.tones[0]);
        assert_eq!(again.vags[1].frames, vab.vags[1].frames);
        assert_eq!(
            (again.vags[1].has_loop, again.vags[1].loop_start_frame, again.vags[1].loop_end_frame),
            (true, 0, 56)
        );
        assert_eq!(again.decode_vag(&again_body, 1).unwrap(), vab.decode_vag(&body, 1).unwrap());
    }
}
