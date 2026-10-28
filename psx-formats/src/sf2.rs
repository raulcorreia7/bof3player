//! SoundFont 2 (`.sf2`): the writer for a VAB bank, and the reader that takes a file back to its parts.
//!
//! A bank becomes one SoundFont file. Program `p` of the bank is preset `p` of bank 0, with an
//! instrument of the same program, so a Standard MIDI File that selects program `p` plays the same
//! sound through the SoundFont. Each usable tone is one zone of its program's instrument. The zone
//! keeps the tone's key range, its root key (the centre note), its tuning (`shift`), its level
//! (`vol`), its pan and its loop. Each VAG is decoded to 16-bit PCM at 44,100 Hz, the SPU's rate at
//! pitch `0x1000`.
//!
//! [`write()`] keeps no envelope: each zone takes the SoundFont default. [`write_with`] maps the ADSR registers
//! through a caller's envelope function. Not mapped by either: the reverb send and the program's own volume
//! and pan.
//!
//! [`read`] takes a file back to its presets, instruments, zones and samples. [`vol_of_attenuation`],
//! [`shift_of_tuning`] and [`pan_of_tenths`] invert the zone mappings of the writer.
//!
//! # Examples
//!
//! ```
//! use psx_formats::sf2;
//! use psx_formats::vab::{Bank, encode_bank};
//!
//! let (header, body) = encode_bank(0, &[], &[]).unwrap();
//! let bank = Bank::parse(&header, &body).unwrap();
//! let font = sf2::read(&sf2::write(&bank, &body, "EMPTY").unwrap()).unwrap();
//! assert!(font.instruments.is_empty());
//! ```

use crate::bytes::{u16_le, u32_le};
use crate::error::{Error, Result};
use crate::vab::{Bank, Tone};

/// The rate a VAG plays at when its pitch is `0x1000`.
const SAMPLE_RATE: u32 = 44_100;
/// The sample points SoundFont 2 requires after the last sample.
const SAMPLE_PADDING: usize = 46;
/// The root key of every sample header. Each zone overrides it with its centre note.
const DEFAULT_ROOT_KEY: u8 = 60;
/// `shift` counts this many units per semitone.
const SHIFT_UNITS_PER_SEMITONE: i32 = 128;
const CENTS_PER_SEMITONE: i32 = 100;
/// The full level of a tone (`vol` 127) is zero attenuation.
const FULL_VOLUME: f64 = 127.0;
/// The largest attenuation SoundFont 2 allows, in centibels.
const MAX_ATTENUATION: i32 = 1440;
/// The pan of a tone runs from 0 (left) to 127 (right), with 64 at centre. The SoundFont pan runs from
/// -500 to 500 tenths of a percent.
const PAN_CENTRE: i32 = 64;
const PAN_RANGE: i32 = 500;

/// SoundFont 2 generator operators. Each zone lists its generators in this order, with `sampleID` (or
/// `instrument`) last.
const GEN_PAN: u16 = 17;
const GEN_ATTACK_VOL_ENV: u16 = 34;
const GEN_DECAY_VOL_ENV: u16 = 36;
const GEN_SUSTAIN_VOL_ENV: u16 = 37;
const GEN_RELEASE_VOL_ENV: u16 = 38;
const GEN_INSTRUMENT: u16 = 41;
const GEN_KEY_RANGE: u16 = 43;
const GEN_INITIAL_ATTENUATION: u16 = 48;
const GEN_COARSE_TUNE: u16 = 51;
const GEN_FINE_TUNE: u16 = 52;
const GEN_SAMPLE_ID: u16 = 53;
const GEN_SAMPLE_MODES: u16 = 54;
const GEN_OVERRIDING_ROOT_KEY: u16 = 58;
/// `sfSampleType`: a mono sample. `sampleModes`: loop continuously.
const MONO: u16 = 1;
const LOOP: u16 = 1;

/// One sample header: a VAG decoded into the shared sample data.
struct SampleData {
    name: String,
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    looped: bool,
}

/// One zone of an instrument: one tone.
struct ZoneData {
    keys: (u8, u8),
    root: u8,
    /// Tuning in semitones and cents.
    tuning: (i16, i16),
    attenuation: i16,
    pan: i16,
    envelope: Envelope,
    looped: bool,
    sample: u16,
}

/// One instrument, which is the program of the same number.
struct InstrumentData {
    program: u16,
    zones: Vec<ZoneData>,
}

/// Writes the bank as a SoundFont 2 file named `name`, with the SoundFont default volume envelope on every
/// zone. `body` is the VAB body the header describes.
///
/// # Examples
///
/// ```
/// use psx_formats::sf2;
/// use psx_formats::vab::{Bank, encode_bank};
///
/// let (header, body) = encode_bank(0, &[], &[]).unwrap();
/// let bank = Bank::parse(&header, &body).unwrap();
/// let file = sf2::write(&bank, &body, "EMPTY").unwrap();
/// assert_eq!(&file[..4], b"RIFF");
/// ```
pub fn write(vab: &Bank, body: &[u8], name: &str) -> Result<Vec<u8>> {
    write_with(vab, body, name, &|_, _| None)
}

/// Writes the bank as a SoundFont 2 file named `name`. Each tone's volume envelope is `envelope` of its ADSR
/// registers, or the SoundFont default when `envelope` returns `None`. `body` is the VAB body the header
/// describes.
///
/// # Examples
///
/// ```
/// use psx_formats::sf2;
/// use psx_formats::vab::{Bank, encode_bank};
///
/// let (header, body) = encode_bank(0, &[], &[]).unwrap();
/// let bank = Bank::parse(&header, &body).unwrap();
/// let envelope = |_, _| Some(sf2::Envelope::DEFAULT);
/// assert_eq!(&sf2::write_with(&bank, &body, "EMPTY", &envelope).unwrap()[..4], b"RIFF");
/// ```
pub fn write_with(
    vab: &Bank,
    body: &[u8],
    name: &str,
    envelope: &dyn Fn(u16, u16) -> Option<Envelope>,
) -> Result<Vec<u8>> {
    let mut samples: Vec<SampleData> = Vec::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut sample_of_vag: Vec<Option<u16>> = vec![None; vab.vags.len()];
    let mut instruments = Vec::new();
    for ((program, _), block) in vab.playable_programs().zip(vab.tone_blocks()) {
        // Every slot of the block is a candidate. The voice manager walks the program's up-to-16 tones and
        // plays each usable tone whose note range holds, so a slot past the program's tone count still sounds.
        let mut zones = Vec::new();
        for tone in block {
            if !tone.is_usable(vab.vag_count) || tone.vol == 0 || tone.min_note > tone.max_note {
                continue;
            }
            let vag = tone.vag as u32;
            let reference = *vab.vag(vag)?;
            if reference.is_empty() {
                continue;
            }
            let sample = match sample_of_vag[vag as usize] {
                Some(sample) => sample,
                None => {
                    let frames = vab.decode_vag(body, vag)?;
                    let start = pcm.len() as u32;
                    let end = start + frames.len() as u32;
                    let looped = reference.has_loop && reference.loop_start_frame < reference.loop_end_frame;
                    let (loop_start, loop_end) = if looped {
                        (start + reference.loop_start_frame, start + reference.loop_end_frame.min(frames.len() as u32))
                    } else {
                        (start, start)
                    };
                    pcm.extend_from_slice(&frames);
                    let number = samples.len() as u16;
                    samples.push(SampleData { name: format!("VAG{vag:03}"), start, end, loop_start, loop_end, looped });
                    sample_of_vag[vag as usize] = Some(number);
                    number
                }
            };
            let volume = envelope(tone.adsr1, tone.adsr2).unwrap_or(Envelope::DEFAULT);
            zones.push(zone_of(tone, sample, samples[usize::from(sample)].looped, volume));
        }
        if !zones.is_empty() {
            instruments.push(InstrumentData { program: program as u16, zones });
        }
    }
    Ok(file(name, &instruments, &samples, &pcm))
}

/// The zone of one tone that plays sample `sample`, with its volume envelope.
fn zone_of(tone: &Tone, sample: u16, looped: bool, envelope: Envelope) -> ZoneData {
    ZoneData {
        keys: (tone.min_note, tone.max_note),
        root: tone.center_note,
        tuning: tuning(tone.shift),
        attenuation: attenuation(tone.vol),
        pan: pan(tone.pan),
        envelope,
        looped,
        sample,
    }
}

/// The tone volume a zone's attenuation names: the smallest volume whose attenuation is nearest, so a
/// value [`write()`] produced returns the volume it came from.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sf2::vol_of_attenuation(0), 127);
/// ```
pub fn vol_of_attenuation(centibels: i16) -> u8 {
    (1..=127_u8).min_by_key(|&vol| (i32::from(attenuation(vol)) - i32::from(centibels)).abs()).unwrap_or(127)
}

/// The tone `shift` a zone's tuning names: the smallest shift whose tuning is nearest, so a tuning
/// [`write()`] produced returns the shift it came from.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sf2::shift_of_tuning(0, 0), 0);
/// ```
pub fn shift_of_tuning(coarse: i16, fine: i16) -> u8 {
    let target = i32::from(coarse) * CENTS_PER_SEMITONE + i32::from(fine);
    (0..=255_u8)
        .min_by_key(|&shift| {
            let (semitones, cents) = tuning(shift);
            (i32::from(semitones) * CENTS_PER_SEMITONE + i32::from(cents) - target).abs()
        })
        .unwrap_or(0)
}

/// The tone pan a zone's pan names: the pan whose SoundFont pan is nearest, so a value [`write()`] produced
/// returns the pan it came from.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sf2::pan_of_tenths(0), 64);
/// ```
pub fn pan_of_tenths(tenths: i16) -> u8 {
    (0..=127_u8).min_by_key(|&pan_value| (i32::from(pan(pan_value)) - i32::from(tenths)).abs()).unwrap_or(64)
}

/// The SoundFont volume envelope of a zone. The phase times are timecents, `1200 * log2(seconds)`, kept
/// between `-12000` and `8000`. The sustain is the level below full scale, in centibels. SoundFont 2 gives the
/// attack a linear rise and the decay and release a linear fall in decibels.
///
/// # Examples
///
/// ```
/// let envelope = psx_formats::sf2::Envelope::new(1.0, 0.5, 0x4000, 2.0);
/// assert_eq!((envelope.attack, envelope.decay, envelope.sustain), (0, -1200, 60));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Envelope {
    /// Attack time, timecents.
    pub attack: i16,
    /// Decay time, timecents: from full level to the sustain level.
    pub decay: i16,
    /// Sustain level, centibels below full.
    pub sustain: i16,
    /// Release time, timecents: from the sustain level to silence.
    pub release: i16,
}

impl Envelope {
    /// The SoundFont defaults: every phase at its shortest time, and the sustain at full level.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::sf2::Envelope::DEFAULT.sustain, 0);
    /// ```
    pub const DEFAULT: Self = Self { attack: TIMECENTS_MIN, decay: TIMECENTS_MIN, sustain: 0, release: TIMECENTS_MIN };

    /// An envelope from phase times in seconds and a sustain level on the PS1 scale, where `0x8000` is full
    /// scale.
    ///
    /// # Examples
    ///
    /// ```
    /// let envelope = psx_formats::sf2::Envelope::new(0.5, 1.0, 0x8000, 0.25);
    /// assert_eq!(envelope.sustain, 0, "full scale is no attenuation");
    /// ```
    pub fn new(attack_seconds: f64, decay_seconds: f64, sustain_level: i32, release_seconds: f64) -> Self {
        Self {
            attack: timecents(attack_seconds),
            decay: timecents(decay_seconds),
            sustain: centibels(sustain_level),
            release: timecents(release_seconds),
        }
    }
}

/// The shortest and longest envelope time SoundFont 2 allows, in timecents.
const TIMECENTS_MIN: i16 = -12_000;
const TIMECENTS_MAX: i16 = 8_000;

/// Timecents for a time in seconds. A time of zero or less is the shortest.
fn timecents(seconds: f64) -> i16 {
    (1200.0 * seconds.log2()).round().clamp(f64::from(TIMECENTS_MIN), f64::from(TIMECENTS_MAX)) as i16
}

/// Centibels below full scale for a PS1 level: `200 log10(0x8000 / level)`, from zero to the SoundFont limit.
fn centibels(level: i32) -> i16 {
    if level <= 0 {
        return MAX_ATTENUATION as i16;
    }
    let below = 200.0 * (f64::from(0x8000) / f64::from(level)).log10();
    (below.round() as i32).clamp(0, MAX_ATTENUATION) as i16
}

/// A tone's `shift` as semitones and cents, rounded to the nearest cent.
fn tuning(shift: u8) -> (i16, i16) {
    let cents = (i32::from(shift) * CENTS_PER_SEMITONE + SHIFT_UNITS_PER_SEMITONE / 2) / SHIFT_UNITS_PER_SEMITONE;
    ((cents / CENTS_PER_SEMITONE) as i16, (cents % CENTS_PER_SEMITONE) as i16)
}

/// A tone's level below full, in centibels: `200 * log10(127 / vol)`, capped at the SoundFont limit.
fn attenuation(vol: u8) -> i16 {
    let centibels = 200.0 * (FULL_VOLUME / f64::from(vol)).log10();
    (centibels.round() as i32).clamp(0, MAX_ATTENUATION) as i16
}

/// A tone's pan in tenths of a percent, from -500 (left) to 500 (right).
fn pan(pan: u8) -> i16 {
    ((i32::from(pan) - PAN_CENTRE) * PAN_RANGE / PAN_CENTRE) as i16
}

/// The generators of a zone, in the order the file lists them.
fn generators(zone: &ZoneData) -> Vec<(u16, u16)> {
    let (coarse, fine) = zone.tuning;
    let mut generators = vec![
        (GEN_KEY_RANGE, u16::from(zone.keys.0) | (u16::from(zone.keys.1) << 8)),
        (GEN_OVERRIDING_ROOT_KEY, u16::from(zone.root)),
    ];
    if coarse != 0 {
        generators.push((GEN_COARSE_TUNE, coarse as u16));
    }
    if fine != 0 {
        generators.push((GEN_FINE_TUNE, fine as u16));
    }
    if zone.attenuation != 0 {
        generators.push((GEN_INITIAL_ATTENUATION, zone.attenuation as u16));
    }
    if zone.pan != 0 {
        generators.push((GEN_PAN, zone.pan as u16));
    }
    generators.push((GEN_ATTACK_VOL_ENV, zone.envelope.attack as u16));
    generators.push((GEN_DECAY_VOL_ENV, zone.envelope.decay as u16));
    generators.push((GEN_SUSTAIN_VOL_ENV, zone.envelope.sustain as u16));
    generators.push((GEN_RELEASE_VOL_ENV, zone.envelope.release as u16));
    if zone.looped {
        generators.push((GEN_SAMPLE_MODES, LOOP));
    }
    generators.push((GEN_SAMPLE_ID, zone.sample));
    generators
}

/// The whole RIFF file: the INFO list, the sample data and the preset data.
fn file(name: &str, instruments: &[InstrumentData], samples: &[SampleData], pcm: &[i16]) -> Vec<u8> {
    let info = list(
        b"INFO",
        &[chunk(b"ifil", &[2, 0, 1, 0]), chunk(b"isng", b"EMU8000\0"), chunk(b"INAM", &nul_terminated(name))],
    );
    let mut data: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    data.extend(std::iter::repeat_n(0, 2 * SAMPLE_PADDING));
    let sdta = list(b"sdta", &[chunk(b"smpl", &data)]);
    let pdta = list(b"pdta", &preset_data(instruments, samples));
    let mut body = b"sfbk".to_vec();
    body.extend(info);
    body.extend(sdta);
    body.extend(pdta);
    chunk(b"RIFF", &body)
}

/// The `pdta` chunks: presets, instruments and samples, each with its index tables.
fn preset_data(instruments: &[InstrumentData], samples: &[SampleData]) -> Vec<Vec<u8>> {
    let (mut phdr, mut pbag, mut pgen) = (Vec::new(), Vec::new(), Vec::new());
    let (mut inst, mut ibag, mut igen, mut shdr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut zone_count: u16 = 0;
    let mut generator_count: u16 = 0;
    for (index, instrument) in instruments.iter().enumerate() {
        let index = index as u16;
        let program = format!("PROG{:03}", instrument.program);
        phdr.extend(preset_record(&program, instrument.program, index));
        pbag.extend(bag_record(index, 0));
        pgen.extend(gen_record(GEN_INSTRUMENT, index));
        inst.extend(inst_record(&program, zone_count));
        for zone in &instrument.zones {
            ibag.extend(bag_record(generator_count, 0));
            for (oper, amount) in generators(zone) {
                igen.extend(gen_record(oper, amount));
                generator_count += 1;
            }
            zone_count += 1;
        }
    }
    let preset_count = instruments.len() as u16;
    phdr.extend(preset_record("EOP", 0, preset_count));
    pbag.extend(bag_record(preset_count, 0));
    pgen.extend(gen_record(0, 0));
    inst.extend(inst_record("EOI", zone_count));
    ibag.extend(bag_record(generator_count, 0));
    igen.extend(gen_record(0, 0));
    for sample in samples {
        shdr.extend(sample_record(sample));
    }
    shdr.extend(sample_record(&SampleData {
        name: "EOS".into(),
        start: 0,
        end: 0,
        loop_start: 0,
        loop_end: 0,
        looped: false,
    }));
    vec![
        chunk(b"phdr", &phdr),
        chunk(b"pbag", &pbag),
        chunk(b"pmod", &[0; 10]),
        chunk(b"pgen", &pgen),
        chunk(b"inst", &inst),
        chunk(b"ibag", &ibag),
        chunk(b"imod", &[0; 10]),
        chunk(b"igen", &igen),
        chunk(b"shdr", &shdr),
    ]
}

fn preset_record(name: &str, number: u16, bag: u16) -> Vec<u8> {
    let mut record = name20(name).to_vec();
    for field in [number, 0, bag] {
        record.extend(field.to_le_bytes());
    }
    record.extend([0; 12]);
    record
}

fn inst_record(name: &str, bag: u16) -> Vec<u8> {
    let mut record = name20(name).to_vec();
    record.extend(bag.to_le_bytes());
    record
}

fn bag_record(generator: u16, modulator: u16) -> Vec<u8> {
    [generator.to_le_bytes(), modulator.to_le_bytes()].concat()
}

fn gen_record(oper: u16, amount: u16) -> Vec<u8> {
    [oper.to_le_bytes(), amount.to_le_bytes()].concat()
}

fn sample_record(sample: &SampleData) -> Vec<u8> {
    let mut record = name20(&sample.name).to_vec();
    for field in [sample.start, sample.end, sample.loop_start, sample.loop_end, SAMPLE_RATE] {
        record.extend(field.to_le_bytes());
    }
    record.push(DEFAULT_ROOT_KEY);
    record.push(0);
    record.extend(0u16.to_le_bytes());
    record.extend(MONO.to_le_bytes());
    record
}

/// A 20-byte name, NUL-padded and cut to 19 bytes.
fn name20(text: &str) -> [u8; 20] {
    let mut name = [0u8; 20];
    for (slot, byte) in name.iter_mut().take(19).zip(text.bytes()) {
        *slot = byte;
    }
    name
}

/// A string for an INFO chunk: NUL-terminated and of even length. SoundFont 2 counts the padding of its
/// strings in the chunk size, and fluidsynth rejects an odd size.
fn nul_terminated(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    if !bytes.len().is_multiple_of(2) {
        bytes.push(0);
    }
    bytes
}

/// A RIFF chunk. Odd bodies get one padding byte, which the size does not count.
fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 9);
    out.extend_from_slice(id);
    out.extend((body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if !body.len().is_multiple_of(2) {
        out.push(0);
    }
    out
}

/// A RIFF list chunk: the list type and its chunks.
fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body = kind.to_vec();
    for item in chunks {
        body.extend_from_slice(item);
    }
    chunk(b"LIST", &body)
}

/// A SoundFont 2 file read back to the parts a VAB bank is made of. The reader takes what the writer
/// writes, and any file whose index tables hold together.
///
/// # Examples
///
/// ```
/// let font = psx_formats::sf2::SoundFont { presets: vec![], instruments: vec![], samples: vec![] };
/// assert!(font.samples.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoundFont {
    /// The presets, in file order.
    pub presets: Vec<Preset>,
    /// The instruments, in file order.
    pub instruments: Vec<Instrument>,
    /// The samples, in file order.
    pub samples: Vec<Sample>,
}

/// A preset: the program it plays, in its bank, and the instrument it uses.
///
/// # Examples
///
/// ```
/// let preset = psx_formats::sf2::Preset { number: 3, bank: 0, instrument: 3 };
/// assert_eq!(preset.number, preset.instrument);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preset {
    /// The program number.
    pub number: u16,
    /// The bank the preset is in; bank 0 is the melodic bank.
    pub bank: u16,
    /// The index of the instrument it uses.
    pub instrument: u16,
}

/// An instrument: its zones in file order. A global zone, which names no sample, is not kept.
///
/// # Examples
///
/// ```
/// let instrument = psx_formats::sf2::Instrument { zones: vec![] };
/// assert!(instrument.zones.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instrument {
    /// The zones.
    pub zones: Vec<Zone>,
}

/// One zone: the keys it answers, its tuning, level, pan and volume envelope, and the sample it plays.
///
/// # Examples
///
/// ```
/// use psx_formats::sf2::{Envelope, Zone};
///
/// let zone = Zone {
///     keys: (0, 127),
///     root: Some(60),
///     coarse_tune: 0,
///     fine_tune: 0,
///     attenuation: 0,
///     pan: 0,
///     envelope: Envelope::DEFAULT,
///     looped: false,
///     sample: 0,
/// };
/// assert_eq!(zone.root, Some(60));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Zone {
    /// Lowest and highest key.
    pub keys: (u8, u8),
    /// The root key the zone sets, when it sets one.
    pub root: Option<u8>,
    /// Coarse tuning, in semitones.
    pub coarse_tune: i16,
    /// Fine tuning, in cents.
    pub fine_tune: i16,
    /// Attenuation below full level, in centibels.
    pub attenuation: i16,
    /// Pan, in tenths of a percent from -500 (left) to 500 (right).
    pub pan: i16,
    /// The volume envelope.
    pub envelope: Envelope,
    /// Whether the zone's sample loops.
    pub looped: bool,
    /// The index of the sample it plays.
    pub sample: u16,
}

/// A sample: its PCM and its loop points, in frames from the first frame.
///
/// # Examples
///
/// ```
/// let sample = psx_formats::sf2::Sample {
///     name: "VAG001".into(),
///     pcm: vec![0; 28],
///     loop_start: 0,
///     loop_end: 28,
///     root: 60,
/// };
/// assert_eq!(sample.pcm.len(), 28);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    /// The name, without its terminator.
    pub name: String,
    /// The frames, from the sample's start to its end.
    pub pcm: Vec<i16>,
    /// First frame of the loop, from the start.
    pub loop_start: u32,
    /// One past the last frame of the loop, from the start.
    pub loop_end: u32,
    /// The root key the sample was recorded at.
    pub root: u8,
}

const PHDR_SIZE: usize = 38;
const BAG_SIZE: usize = 4;
const GEN_SIZE: usize = 4;
const INST_SIZE: usize = 22;
const SHDR_SIZE: usize = 46;
const NAME_SIZE: usize = 20;
/// Where each header's index into its bag table sits, and where a bag's index into the generator table sits.
const PHDR_BAG_AT: usize = 24;
const INST_BAG_AT: usize = 20;
const BAG_GEN_AT: usize = 0;
const GEN_DEFAULT_TIME: i16 = TIMECENTS_MIN;

/// Reads a SoundFont 2 file. Fails when it is not an `sfbk` RIFF file, when a chunk it needs is missing, or when an
/// index points past its table.
///
/// # Examples
///
/// ```
/// use psx_formats::sf2;
/// use psx_formats::vab::{Bank, encode_bank};
///
/// let (header, body) = encode_bank(0, &[], &[]).unwrap();
/// let bank = Bank::parse(&header, &body).unwrap();
/// let font = sf2::read(&sf2::write(&bank, &body, "EMPTY").unwrap()).unwrap();
/// assert!(font.presets.is_empty());
/// ```
pub fn read(bytes: &[u8]) -> Result<SoundFont> {
    const RIFF_ID: [u8; 4] = *b"RIFF";
    let top = chunks(bytes)?;
    let body = match top.as_slice() {
        [(RIFF_ID, body)] => *body,
        _ => return Err(Error::SoundFont("the file is not one RIFF chunk")),
    };
    if body.get(..4) != Some(b"sfbk") {
        return Err(Error::SoundFont("the RIFF form is not sfbk"));
    }
    let parts = chunks(&body[4..])?;
    let pdta = list_of(&parts, b"pdta")?;
    let sdta = list_of(&parts, b"sdta")?;
    let pcm: Vec<i16> = find(&sdta, b"smpl")?.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect();

    let presets = read_presets(find(&pdta, b"phdr")?, find(&pdta, b"pbag")?, find(&pdta, b"pgen")?)?;
    let instruments = read_instruments(find(&pdta, b"inst")?, find(&pdta, b"ibag")?, find(&pdta, b"igen")?)?;
    let samples = read_samples(find(&pdta, b"shdr")?, &pcm)?;
    Ok(SoundFont { presets, instruments, samples })
}

/// The RIFF chunks in `bytes`: each identifier and body. A chunk is padded to an even length.
fn chunks(bytes: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let id = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        let size = u32_le(bytes, at + 4) as usize;
        let end = (at + 8).checked_add(size).filter(|&end| end <= bytes.len());
        let end = end.ok_or(Error::SoundFont("a chunk runs past the end of the file"))?;
        found.push((id, &bytes[at + 8..end]));
        at = end + (size & 1);
    }
    Ok(found)
}

/// The chunks of the `LIST` of type `kind`.
fn list_of<'a>(parts: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Result<Vec<([u8; 4], &'a [u8])>> {
    let body = parts
        .iter()
        .find(|(id, body)| id == b"LIST" && body.get(..4) == Some(&kind[..]))
        .map(|(_, body)| *body)
        .ok_or(Error::SoundFont("a LIST chunk is missing"))?;
    chunks(&body[4..])
}

/// The body of the chunk `id`.
fn find<'a>(parts: &[([u8; 4], &'a [u8])], id: &[u8; 4]) -> Result<&'a [u8]> {
    parts.iter().find(|(found, _)| found == id).map(|(_, body)| *body).ok_or(Error::SoundFont("a chunk is missing"))
}

/// The records of a table, `size` bytes each. The last record is the sentinel that ends the table.
fn records(bytes: &[u8], size: usize) -> Result<Vec<&[u8]>> {
    if bytes.len() < size || !bytes.len().is_multiple_of(size) {
        return Err(Error::SoundFont("an index table is not a whole number of records"));
    }
    Ok(bytes.chunks_exact(size).collect())
}

/// The number of records before a table's sentinel.
fn count(table: &[&[u8]]) -> usize {
    table.len().saturating_sub(1)
}

/// The `u16` at byte `at` of record `index`. An index past its table fails.
fn index_at(table: &[&[u8]], index: usize, at: usize) -> Result<usize> {
    let record = table.get(index).ok_or(Error::SoundFont("an index is past its table"))?;
    Ok(usize::from(u16_le(record, at)))
}

/// The entries `[start, end)` that record `index` owns: its index field at byte `at` up to the next record's.
fn span(table: &[&[u8]], at: usize, index: usize) -> Result<std::ops::Range<usize>> {
    let start = index_at(table, index, at)?;
    let end = index_at(table, index + 1, at)?;
    if end < start {
        return Err(Error::SoundFont("an index table is out of order"));
    }
    Ok(start..end)
}

fn read_presets(phdr: &[u8], pbag: &[u8], pgen: &[u8]) -> Result<Vec<Preset>> {
    let headers = records(phdr, PHDR_SIZE)?;
    let bags = records(pbag, BAG_SIZE)?;
    let generators = records(pgen, GEN_SIZE)?;
    let mut presets = Vec::new();
    for (index, header) in headers[..count(&headers)].iter().enumerate() {
        let mut instrument = None;
        for bag in span(&headers, PHDR_BAG_AT, index)? {
            for generator in span(&bags, BAG_GEN_AT, bag)? {
                let record =
                    generators.get(generator).ok_or(Error::SoundFont("a generator index is past its table"))?;
                if u16_le(record, 0) == GEN_INSTRUMENT {
                    instrument = Some(u16_le(record, 2));
                }
            }
        }
        if let Some(instrument) = instrument {
            presets.push(Preset { number: u16_le(header, 20), bank: u16_le(header, 22), instrument });
        }
    }
    Ok(presets)
}

fn read_instruments(inst: &[u8], ibag: &[u8], igen: &[u8]) -> Result<Vec<Instrument>> {
    let headers = records(inst, INST_SIZE)?;
    let bags = records(ibag, BAG_SIZE)?;
    let generators = records(igen, GEN_SIZE)?;
    let mut instruments = Vec::new();
    for index in 0..count(&headers) {
        let mut zones = Vec::new();
        for bag in span(&headers, INST_BAG_AT, index)? {
            if let Some(zone) = read_zone(&generators, span(&bags, BAG_GEN_AT, bag)?)? {
                zones.push(zone);
            }
        }
        instruments.push(Instrument { zones });
    }
    Ok(instruments)
}

/// The zone of one generator range, or `None` for a global zone, which names no sample.
fn read_zone(generators: &[&[u8]], range: std::ops::Range<usize>) -> Result<Option<Zone>> {
    let mut zone = Zone {
        keys: (0, 127),
        root: None,
        coarse_tune: 0,
        fine_tune: 0,
        attenuation: 0,
        pan: 0,
        envelope: Envelope { attack: GEN_DEFAULT_TIME, decay: GEN_DEFAULT_TIME, sustain: 0, release: GEN_DEFAULT_TIME },
        looped: false,
        sample: 0,
    };
    let mut has_sample = false;
    for generator in range {
        let record = generators.get(generator).ok_or(Error::SoundFont("a generator index is past its table"))?;
        let (oper, amount) = (u16_le(record, 0), u16_le(record, 2));
        match oper {
            GEN_KEY_RANGE => zone.keys = (amount as u8, (amount >> 8) as u8),
            GEN_OVERRIDING_ROOT_KEY => zone.root = Some(amount as u8),
            GEN_COARSE_TUNE => zone.coarse_tune = amount as i16,
            GEN_FINE_TUNE => zone.fine_tune = amount as i16,
            GEN_INITIAL_ATTENUATION => zone.attenuation = amount as i16,
            GEN_PAN => zone.pan = amount as i16,
            GEN_ATTACK_VOL_ENV => zone.envelope.attack = amount as i16,
            GEN_DECAY_VOL_ENV => zone.envelope.decay = amount as i16,
            GEN_SUSTAIN_VOL_ENV => zone.envelope.sustain = amount as i16,
            GEN_RELEASE_VOL_ENV => zone.envelope.release = amount as i16,
            GEN_SAMPLE_MODES => zone.looped = amount & LOOP != 0,
            GEN_SAMPLE_ID => {
                zone.sample = amount;
                has_sample = true;
            }
            _ => {}
        }
    }
    Ok(has_sample.then_some(zone))
}

fn read_samples(shdr: &[u8], pcm: &[i16]) -> Result<Vec<Sample>> {
    let headers = records(shdr, SHDR_SIZE)?;
    let mut samples = Vec::with_capacity(count(&headers));
    for header in &headers[..count(&headers)] {
        let start = u32_le(header, 20);
        let end = u32_le(header, 24);
        let frames =
            pcm.get(start as usize..end as usize).ok_or(Error::SoundFont("a sample runs past the sample data"))?;
        samples.push(Sample {
            name: name_of(&header[..NAME_SIZE]),
            pcm: frames.to_vec(),
            loop_start: u32_le(header, 28).saturating_sub(start),
            loop_end: u32_le(header, 32).saturating_sub(start),
            root: header[40],
        });
    }
    Ok(samples)
}

/// A fixed-width name: the bytes up to the first NUL.
fn name_of(field: &[u8]) -> String {
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}
