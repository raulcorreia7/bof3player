//! SEP sequence packages: the event streams, their validator and their encoder.
//!
//! Layout: `"pQES"` magic and a zero big-endian `u16` version, then per sequence a 13-byte header
//! (id, resolution, tempo, time signature, payload size) followed by that many bytes of events.
//! An event is a variable-length delta, an optional status byte (running status when absent) and
//! 0 to 2 data bytes; `0xFF` is a meta event whose payload length is declared in the stream.
//!
//! The parser accepts only what [`Package::encode`] writes back, so a package that parses is a
//! package that round-trips; [`Package::problems`] reports what the format rules out beyond that.
//!
//! # Examples
//!
//! ```
//! use psx_formats::sep::{Event, Package, Sequence};
//!
//! let sequence = Sequence {
//!     resolution: 48,
//!     events: vec![Event { status: 0x90, data1: 60, data2: 100, status_byte: true, ..Event::default() }],
//!     ..Sequence::default()
//! };
//! let package = Package { sequences: vec![sequence] };
//! assert_eq!(Package::parse(&package.encode()).unwrap(), package);
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::bytes::{push_u16_be, push_u24_be, push_u32_be, u16_be, u24_be, u32_be};
use crate::error::{Error, Result};

/// Magic a package opens with, in file order.
const SEP_MAGIC: &[u8; 4] = b"pQES";
/// Container entry type of a sequence package.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sep::TYPE, 10);
/// ```
pub const TYPE: u16 = 10;
/// Bytes of the file header: the magic plus the version field.
const FILE_HEADER: usize = 6;
/// Bytes of one sequence header.
const SEQ_HEADER: usize = 13;
/// Version the format declares.
const VERSION: u16 = 0;
/// Meta type of the end-of-track event.
const META_END: u8 = 0x2f;
/// Bytes a variable-length quantity may take to stay inside a `u32`.
const VLQ_MAX_BYTES: usize = 5;
/// Meta type of the set-tempo event.
const META_TEMPO: u8 = 0x51;
/// Tempo a sequence states none for: 120 BPM, in microseconds per quarter note.
const DEFAULT_TEMPO_US: u32 = 500_000;
/// Time signature numerator a sequence states none for.
const DEFAULT_TIME_NUMERATOR: u8 = 4;
/// Time signature denominator (a power-of-two exponent) a sequence states none for.
const DEFAULT_TIME_DENOMINATOR: u8 = 2;
/// MIDI clocks per metronome click and 32nd notes per quarter in an exported time signature.
const MIDI_CLOCKS_PER_CLICK: u8 = 24;
const MIDI_THIRTY_SECONDS_PER_QUARTER: u8 = 8;
/// Meta type of the time-signature event.
const META_TIME_SIGNATURE: u8 = 0x58;
/// MIDI channels.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sep::CHANNELS, 16);
/// ```
pub const CHANNELS: usize = 16;

/// Message class of an event's status byte.
///
/// # Examples
///
/// ```
/// use psx_formats::sep::{Event, Kind};
///
/// let event = Event { status: 0x92, ..Event::default() };
/// assert_eq!(event.kind(), Kind::NoteOn);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Note off.
    NoteOff = 0x80,
    /// Note on (velocity 0 is a note off).
    NoteOn = 0x90,
    /// Polyphonic aftertouch.
    PolyAftertouch = 0xA0,
    /// Control change.
    Control = 0xB0,
    /// Program change.
    Program = 0xC0,
    /// Channel aftertouch.
    ChannelAftertouch = 0xD0,
    /// Pitch bend.
    PitchBend = 0xE0,
    /// Meta event.
    Meta = 0xFF,
}

/// Data bytes a status byte carries: one for program and channel aftertouch,
/// two for the rest, none for meta and for bytes outside `0x80..=0xEF`.
fn data_bytes(status: u8) -> usize {
    match status & 0xf0 {
        0xc0 | 0xd0 => 1,
        0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => 2,
        _ => 0,
    }
}

/// One event of a sequence: the delta that waits for it, the status it runs
/// under, and the bytes that status declares.
///
/// # Examples
///
/// ```
/// use psx_formats::sep::Event;
///
/// let event = Event { delta: 48, status: 0xb3, data1: 7, ..Event::default() };
/// assert_eq!(event.channel(), 3);
/// assert_eq!(event.delta, 48);
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    /// Ticks since the previous event.
    pub delta: u32,
    /// Status byte, running status resolved; `0xff` marks a meta event.
    pub status: u8,
    /// First data byte.
    pub data1: u8,
    /// Second data byte.
    pub data2: u8,
    /// Meta type, meaningful when [`Event::kind`] is [`Kind::Meta`].
    pub meta_type: u8,
    /// Meta payload, empty for end of track.
    pub meta: Vec<u8>,
    /// Whether the stream repeated the status byte instead of running it on.
    pub status_byte: bool,
}

impl Event {
    /// The event's message class; the parser keeps no other status byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::sep::{Event, Kind};
    ///
    /// assert_eq!(Event { status: 0xff, ..Event::default() }.kind(), Kind::Meta);
    /// ```
    pub fn kind(&self) -> Kind {
        match self.status & 0xf0 {
            0x80 => Kind::NoteOff,
            0x90 => Kind::NoteOn,
            0xa0 => Kind::PolyAftertouch,
            0xb0 => Kind::Control,
            0xc0 => Kind::Program,
            0xd0 => Kind::ChannelAftertouch,
            0xe0 => Kind::PitchBend,
            _ => Kind::Meta,
        }
    }

    /// The channel the status byte addresses.
    ///
    /// # Examples
    ///
    /// ```
    /// let event = psx_formats::sep::Event { status: 0x9f, ..Default::default() };
    /// assert_eq!(event.channel(), 15);
    /// ```
    pub fn channel(&self) -> u8 {
        self.status & 0x0f
    }

    /// The tempo a set-tempo meta event states, in microseconds per quarter note.
    ///
    /// # Examples
    ///
    /// ```
    /// let event = psx_formats::sep::Event {
    ///     status: 0xff,
    ///     meta_type: 0x51,
    ///     meta: vec![0x07, 0xa1, 0x20],
    ///     ..Default::default()
    /// };
    /// assert_eq!(event.tempo(), Some(500_000));
    /// ```
    pub fn tempo(&self) -> Option<u32> {
        (self.kind() == Kind::Meta && self.meta_type == META_TEMPO && self.meta.len() >= 3)
            .then(|| u24_be(&self.meta, 0))
    }
}

/// One sequence: its header fields and its events, in stream order.
///
/// # Examples
///
/// ```
/// use psx_formats::sep::{Event, Sequence};
///
/// let sequence = Sequence {
///     resolution: 48,
///     events: vec![Event { status: 0xc0, data1: 5, status_byte: true, ..Event::default() }],
///     ..Sequence::default()
/// };
/// assert_eq!(sequence.programs_used(), 0, "a program change alone selects no note");
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sequence {
    /// Sequence id the package states.
    pub seq_id: u16,
    /// Ticks per quarter note.
    pub resolution: u16,
    /// Microseconds per quarter note, three big-endian bytes.
    pub tempo_us: u32,
    /// Time signature numerator.
    pub time_num: u8,
    /// Time signature denominator (power-of-two exponent).
    pub time_den: u8,
    /// The events, in stream order.
    pub events: Vec<Event>,
}

impl Sequence {
    /// Reads one format 0 Standard MIDI File, as [`Package::to_midi`] writes it, into a sequence with id `seq_id`.
    ///
    /// `to_midi` puts a tempo and a time signature at time zero, and it appends an end-of-track after the
    /// sequence's own. Those become the header fields and are not events. A status byte equal to the running
    /// status is a run-on event, as in the package. MIDI has no sequence id, so the caller supplies it.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::sep::{Event, Package, Sequence};
    ///
    /// let sequence = Sequence {
    ///     resolution: 48,
    ///     events: vec![Event { status: 0x90, data1: 60, data2: 100, status_byte: true, ..Event::default() }],
    ///     ..Sequence::default()
    /// };
    /// let midi = Package { sequences: vec![sequence] }.to_midi(0).unwrap();
    /// let back = Sequence::from_midi(&midi, 7).unwrap();
    /// assert_eq!(back.seq_id, 7);
    /// assert!(back.has_notes());
    /// ```
    pub fn from_midi(midi: &[u8], seq_id: u16) -> Result<Self> {
        let (resolution, track) = midi_track(midi)?;
        let mut events = Vec::new();
        let (mut at, mut running) = (0, 0u8);
        while at < track.len() {
            let delta = midi_vlq(track, &mut at)?;
            let status = match track.get(at) {
                Some(&byte) if byte & 0x80 != 0 => {
                    at += 1;
                    byte
                }
                Some(_) if running != 0 => running,
                Some(_) => return Err(midi_error("a data byte has no running status")),
                None => return Err(midi_error("the track ends on a delta")),
            };
            running = if (0x80..0xf0).contains(&status) { status } else { 0 };
            let mut event = Event { delta, status, ..Event::default() };
            if status == 0xff {
                event.meta_type = *track.get(at).ok_or_else(|| midi_error("a meta event has no type"))?;
                at += 1;
                let length = midi_vlq(track, &mut at)? as usize;
                let payload =
                    track.get(at..at + length).ok_or_else(|| midi_error("a meta payload runs past the track"))?;
                event.meta = payload.to_vec();
                at += length;
            } else if status >= 0xf0 {
                return Err(midi_error("system exclusive messages have no sequence form"));
            } else {
                let wanted = data_bytes(status);
                let data = track.get(at..at + wanted).ok_or_else(|| midi_error("a message runs past the track"))?;
                if wanted >= 1 {
                    event.data1 = data[0];
                }
                if wanted >= 2 {
                    event.data2 = data[1];
                }
                at += wanted;
            }
            events.push(event);
        }

        let (mut tempo_us, mut time_num, mut time_den) = (0, 0, 0);
        if events.first().is_some_and(|event| is_meta(event, META_TEMPO, 3)) {
            tempo_us = u24_be(&events[0].meta, 0);
            events.remove(0);
        }
        if events.first().is_some_and(|event| is_meta(event, META_TIME_SIGNATURE, 4)) {
            time_num = events[0].meta[0];
            time_den = events[0].meta[1];
            events.remove(0);
        }
        let count = events.len();
        if count >= 2 && is_meta(&events[count - 1], META_END, 0) && is_meta(&events[count - 2], META_END, 0) {
            events.pop();
        }

        let mut running = 0;
        for event in &mut events {
            event.status_byte = event.status != running;
            running = event.status;
        }
        Ok(Self { seq_id, resolution, tempo_us, time_num, time_den, events })
    }

    /// Whether the sequence keys a voice at least once.
    ///
    /// # Examples
    ///
    /// ```
    /// let event = psx_formats::sep::Event { status: 0x90, data2: 100, ..Default::default() };
    /// let sequence = psx_formats::sep::Sequence { events: vec![event], ..Default::default() };
    /// assert!(sequence.has_notes());
    /// ```
    pub fn has_notes(&self) -> bool {
        self.events.iter().any(|event| event.kind() == Kind::NoteOn && event.data2 != 0)
    }

    /// Programs the note-carrying channels select, one bit each.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::sep::{Event, Sequence};
    ///
    /// let sequence = Sequence {
    ///     events: vec![
    ///         Event { status: 0xc0, data1: 5, status_byte: true, ..Event::default() },
    ///         Event { status: 0x90, data1: 60, data2: 100, status_byte: true, ..Event::default() },
    ///     ],
    ///     ..Sequence::default()
    /// };
    /// assert_eq!(sequence.programs_used(), 1 << 5);
    /// ```
    pub fn programs_used(&self) -> u128 {
        let mut program = [0u8; CHANNELS];
        let mut mask = 0u128;
        for event in &self.events {
            match event.kind() {
                Kind::Program => program[usize::from(event.channel())] = event.data1,
                Kind::NoteOn if event.data2 != 0 => {
                    mask |= 1u128 << (program[usize::from(event.channel())] & 0x7f);
                }
                _ => {}
            }
        }
        mask
    }
}

/// The lowest program a [`Sequence::programs_used`] mask selects, 0 for an empty mask.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sep::lowest_program(1 << 5), 5);
/// assert_eq!(psx_formats::sep::lowest_program(0), 0);
/// ```
pub fn lowest_program(mask: u128) -> u8 {
    if mask == 0 { 0 } else { mask.trailing_zeros() as u8 }
}

/// A parsed package: every sequence the payload tiles.
///
/// # Examples
///
/// ```
/// let package = psx_formats::sep::Package { sequences: vec![Default::default()] };
/// assert_eq!(package.sequences.len(), 1);
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Package {
    /// The sequences, in package order.
    pub sequences: Vec<Sequence>,
}

/// What a package breaks of the format.
///
/// # Examples
///
/// ```
/// let package = psx_formats::sep::Package { sequences: vec![Default::default()] };
/// assert!(matches!(package.problems()[0], psx_formats::sep::Problem::NoEndOfTrack { .. }));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Problem {
    /// An event runs on with no status byte before it.
    UnrunnableStatus {
        /// Position of the sequence.
        sequence: usize,
        /// Position of the event.
        event: usize,
    },
    /// An event repeats a status byte it could have run on.
    RedundantStatus {
        /// Position of the sequence.
        sequence: usize,
        /// Position of the event.
        event: usize,
    },
    /// The sequence does not end on an end-of-track event.
    NoEndOfTrack {
        /// Position of the sequence.
        sequence: usize,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnrunnableStatus { sequence, event } => {
                write!(f, "sequence {sequence} event {event} runs on with no status byte")
            }
            Self::RedundantStatus { sequence, event } => {
                write!(f, "sequence {sequence} event {event} repeats the status byte it runs on")
            }
            Self::NoEndOfTrack { sequence } => write!(f, "sequence {sequence} does not end on an end-of-track event"),
        }
    }
}

impl Package {
    /// Parses a package.
    ///
    /// The sequence headers and their payloads must tile the payload exactly,
    /// and every quantity must be minimally encoded, so [`Package::encode`] of
    /// the result is the input again.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::sep::Package;
    ///
    /// let package = Package { sequences: vec![Default::default()] };
    /// assert_eq!(Package::parse(&package.encode()).unwrap(), package);
    /// assert!(Package::parse(b"pQES").is_err());
    /// ```
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < FILE_HEADER + SEQ_HEADER {
            return Err(Error::SepTooShort(bytes.len()));
        }
        if &bytes[..4] != SEP_MAGIC {
            return Err(Error::SepBadMagic);
        }
        let version = u16_be(bytes, 4);
        if version != VERSION {
            return Err(Error::SepVersion(version));
        }

        let mut sequences = Vec::new();
        let mut at = FILE_HEADER;
        while at < bytes.len() {
            let sequence = sequences.len();
            let remaining = bytes.len() - at;
            if remaining < SEQ_HEADER {
                return Err(Error::SepSequenceOverflow { sequence });
            }
            let data_size = u32_be(bytes, at + 9) as usize;
            if data_size > remaining - SEQ_HEADER {
                return Err(Error::SepSequenceOverflow { sequence });
            }

            sequences.push(Sequence {
                seq_id: u16_be(bytes, at),
                resolution: u16_be(bytes, at + 2),
                tempo_us: u24_be(bytes, at + 4),
                time_num: bytes[at + 7],
                time_den: bytes[at + 8],
                events: read_events(bytes, at + SEQ_HEADER, data_size, sequence)?,
            });
            at += SEQ_HEADER + data_size;
        }
        Ok(Self { sequences })
    }

    /// The bytes of this model; [`Package::parse`] of them gives it back.
    ///
    /// # Examples
    ///
    /// ```
    /// let bytes = psx_formats::sep::Package { sequences: vec![Default::default()] }.encode();
    /// assert_eq!(&bytes[..4], b"pQES");
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SEP_MAGIC);
        out.extend_from_slice(&VERSION.to_be_bytes());
        for sequence in &self.sequences {
            let events = encode_events(sequence);
            push_u16_be(&mut out, sequence.seq_id);
            push_u16_be(&mut out, sequence.resolution);
            push_u24_be(&mut out, sequence.tempo_us);
            out.push(sequence.time_num);
            out.push(sequence.time_den);
            push_u32_be(&mut out, events.len() as u32);
            out.extend_from_slice(&events);
        }
        out
    }

    /// Converts sequence `index` to a Standard MIDI File (format 0, one track).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::sep::{Event, Package, Sequence};
    ///
    /// let sequence = Sequence {
    ///     resolution: 48,
    ///     events: vec![Event { status: 0x90, data1: 60, data2: 100, status_byte: true, ..Event::default() }],
    ///     ..Sequence::default()
    /// };
    /// let midi = Package { sequences: vec![sequence] }.to_midi(0).unwrap();
    /// assert_eq!(&midi[..4], b"MThd");
    /// ```
    pub fn to_midi(&self, index: usize) -> Result<Vec<u8>> {
        let sequence =
            self.sequences.get(index).ok_or(Error::SepSequenceOutOfRange { index, count: self.sequences.len() })?;
        if sequence.resolution == 0 {
            return Err(Error::SepNoResolution);
        }
        let mut track = Vec::with_capacity(sequence.events.len() * 4 + 32);
        let tempo = if sequence.tempo_us != 0 { sequence.tempo_us } else { DEFAULT_TEMPO_US };
        track.extend_from_slice(&[0, 0xff, META_TEMPO, 3]);
        push_u24_be(&mut track, tempo);
        track.extend_from_slice(&[
            0,
            0xff,
            META_TIME_SIGNATURE,
            4,
            if sequence.time_num != 0 { sequence.time_num } else { DEFAULT_TIME_NUMERATOR },
            if sequence.time_den != 0 { sequence.time_den } else { DEFAULT_TIME_DENOMINATOR },
            MIDI_CLOCKS_PER_CLICK,
            MIDI_THIRTY_SECONDS_PER_QUARTER,
        ]);
        for event in &sequence.events {
            push_vlq(&mut track, event.delta);
            if event.status == 0xff {
                track.extend_from_slice(&[0xff, event.meta_type]);
                push_vlq(&mut track, event.meta.len() as u32);
                track.extend_from_slice(&event.meta);
                continue;
            }
            track.push(event.status);
            let wanted = data_bytes(event.status);
            if wanted >= 1 {
                track.push(event.data1);
            }
            if wanted >= 2 {
                track.push(event.data2);
            }
        }
        track.extend_from_slice(&[0, 0xff, META_END, 0]);

        let mut out = Vec::with_capacity(track.len() + 22);
        out.extend_from_slice(b"MThd");
        push_u32_be(&mut out, 6);
        push_u16_be(&mut out, 0);
        push_u16_be(&mut out, 1);
        push_u16_be(&mut out, sequence.resolution);
        out.extend_from_slice(b"MTrk");
        push_u32_be(&mut out, track.len() as u32);
        out.extend_from_slice(&track);
        Ok(out)
    }

    /// What the package breaks of the format; empty for a well-formed package.
    ///
    /// # Examples
    ///
    /// ```
    /// let package = psx_formats::sep::Package { sequences: vec![Default::default()] };
    /// assert_eq!(package.problems().len(), 1, "the sequence has no end-of-track event");
    /// ```
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (sequence, item) in self.sequences.iter().enumerate() {
            let mut running = 0;
            for (event, message) in item.events.iter().enumerate() {
                if !message.status_byte && running == 0 {
                    problems.push(Problem::UnrunnableStatus { sequence, event });
                }
                if message.status_byte && message.status == running {
                    problems.push(Problem::RedundantStatus { sequence, event });
                }
                if message.status != 0xff {
                    running = message.status;
                }
            }
            let ends =
                item.events.last().is_some_and(|event| event.kind() == Kind::Meta && event.meta_type == META_END);
            if !ends {
                problems.push(Problem::NoEndOfTrack { sequence });
            }
        }
        problems
    }

    /// The package as one line per fact: the sequence headers, then every
    /// event. Useful to diff two parses of the same bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// let dump = psx_formats::sep::Package { sequences: vec![Default::default()] }.dump();
    /// assert!(dump.starts_with("sep sequences=1"));
    /// ```
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("sep sequences={}\n", self.sequences.len()));
        for (index, sequence) in self.sequences.iter().enumerate() {
            out.push_str(&format!(
                "seq {index} seq_id={} resolution={} tempo_us={} time_num={} time_den={} \
                 events={}\n",
                sequence.seq_id,
                sequence.resolution,
                sequence.tempo_us,
                sequence.time_num,
                sequence.time_den,
                sequence.events.len(),
            ));
            for (event, message) in sequence.events.iter().enumerate() {
                let meta: String = message.meta.iter().map(|byte| format!("{byte:02x}")).collect();
                out.push_str(&format!(
                    "ev {index} {event} delta={} status={} data1={} data2={} meta_type={} \
                     meta={meta} status_byte={}\n",
                    message.delta,
                    message.status,
                    message.data1,
                    message.data2,
                    message.meta_type,
                    u8::from(message.status_byte),
                ));
            }
        }
        out
    }
}

/// The events of one sequence, up to `size` payload bytes.
fn read_events(bytes: &[u8], at: usize, size: usize, sequence: usize) -> Result<Vec<Event>> {
    let end = at + size;
    let mut events = Vec::new();
    let mut running = 0;
    let mut cursor = at;
    while cursor < end {
        let delta = read_vlq(bytes, &mut cursor, end, sequence)?;
        if cursor >= end {
            return Err(Error::SepEvent { sequence, reason: "the payload ends on a delta" });
        }

        let status_byte = bytes[cursor] & 0x80 != 0;
        if status_byte {
            running = bytes[cursor];
            cursor += 1;
        }
        if running == 0 {
            return Err(Error::SepEvent { sequence, reason: "an event runs on with no status byte" });
        }

        let mut event = Event { delta, status: running, status_byte, ..Event::default() };
        if running == 0xff {
            if cursor >= end {
                return Err(Error::SepEvent { sequence, reason: "a meta event has no type" });
            }
            event.meta_type = bytes[cursor];
            cursor += 1;
            let length = read_vlq(bytes, &mut cursor, end, sequence)? as usize;
            if length > end - cursor {
                return Err(Error::SepEvent { sequence, reason: "a meta payload runs past the sequence" });
            }
            event.meta = bytes[cursor..cursor + length].to_vec();
            cursor += length;
        } else {
            let wanted = data_bytes(running);
            if wanted == 0 {
                return Err(Error::SepEvent { sequence, reason: "a status byte no message class carries" });
            }
            if wanted > end - cursor {
                return Err(Error::SepEvent { sequence, reason: "a message runs past the sequence" });
            }
            if wanted >= 1 {
                event.data1 = bytes[cursor];
            }
            if wanted >= 2 {
                event.data2 = bytes[cursor + 1];
            }
            cursor += wanted;
        }
        events.push(event);
    }
    Ok(events)
}

/// The payload bytes of one sequence, header excluded.
fn encode_events(sequence: &Sequence) -> Vec<u8> {
    let mut out = Vec::new();
    for event in &sequence.events {
        push_vlq(&mut out, event.delta);
        if event.status_byte {
            out.push(event.status);
        }
        if event.status == 0xff {
            out.push(event.meta_type);
            push_vlq(&mut out, event.meta.len() as u32);
            out.extend_from_slice(&event.meta);
            continue;
        }
        match data_bytes(event.status) {
            1 => out.push(event.data1),
            2 => {
                out.push(event.data1);
                out.push(event.data2);
            }
            _ => {}
        }
    }
    out
}

/// The resolution and the single track of a format 0 Standard MIDI File.
fn midi_track(midi: &[u8]) -> Result<(u16, &[u8])> {
    if midi.len() < 14 || &midi[..4] != b"MThd" || u32_be(midi, 4) != 6 {
        return Err(midi_error("it has no MThd header of length 6"));
    }
    if u16_be(midi, 8) != 0 || u16_be(midi, 10) != 1 {
        return Err(midi_error("it is not one format 0 track"));
    }
    let division = u16_be(midi, 12);
    if division == 0 || division & 0x8000 != 0 {
        return Err(midi_error("its division is not ticks per quarter note"));
    }
    let mut at = 14;
    while at + 8 <= midi.len() {
        let length = u32_be(midi, at + 4) as usize;
        let body = midi.get(at + 8..at + 8 + length).ok_or_else(|| midi_error("a chunk runs past the file"))?;
        if &midi[at..at + 4] == b"MTrk" {
            return Ok((division, body));
        }
        at += 8 + length;
    }
    Err(midi_error("it has no track"))
}

/// One Standard MIDI File variable-length quantity, at most four bytes.
fn midi_vlq(track: &[u8], at: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for _ in 0..4 {
        let byte = *track.get(*at).ok_or_else(|| midi_error("a quantity runs past the track"))?;
        *at += 1;
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(midi_error("a quantity is longer than four bytes"))
}

fn midi_error(reason: &'static str) -> Error {
    Error::MidiFile { reason }
}

/// Whether an event is the meta event of `meta_type` with a payload of `length` bytes.
fn is_meta(event: &Event, meta_type: u8, length: usize) -> bool {
    event.status == 0xff && event.meta_type == meta_type && event.meta.len() == length
}

/// One variable-length quantity: seven payload bits per byte, high bit set
/// while more follow, and never a byte the value does not need.
fn read_vlq(bytes: &[u8], at: &mut usize, end: usize, sequence: usize) -> Result<u32> {
    let start = *at;
    let mut value: u64 = 0;
    loop {
        if *at >= end {
            return Err(Error::SepEvent { sequence, reason: "a quantity runs past the sequence" });
        }
        let byte = bytes[*at];
        *at += 1;
        value = (value << 7) | u64::from(byte & 0x7f);
        if value > u64::from(u32::MAX) {
            return Err(Error::SepEvent { sequence, reason: "a quantity does not fit four bytes" });
        }
        if byte & 0x80 == 0 {
            break;
        }
        if *at - start >= VLQ_MAX_BYTES {
            return Err(Error::SepEvent { sequence, reason: "a quantity does not fit four bytes" });
        }
    }
    let minimal = *at - start == 1 || value >= (1 << (7 * (*at - start - 1)));
    if !minimal {
        return Err(Error::SepEvent { sequence, reason: "a quantity is not minimally encoded" });
    }
    Ok(value as u32)
}
fn push_vlq(out: &mut Vec<u8>, value: u32) {
    let mut buffer = [0u8; VLQ_MAX_BYTES];
    let mut at = buffer.len();
    let mut value = value;
    loop {
        at -= 1;
        buffer[at] = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    let last = buffer.len() - 1;
    for byte in &mut buffer[at..last] {
        *byte |= 0x80;
    }
    out.extend_from_slice(&buffer[at..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tiny_sep;

    #[test]
    fn parses_events_and_running_status() {
        let sep = Package::parse(&tiny_sep()).unwrap();
        let sequence = &sep.sequences[0];
        assert_eq!(sequence.resolution, 48);
        assert_eq!(sequence.events.len(), 6);
        assert_eq!(sequence.events[2].kind(), Kind::NoteOn);
        assert!(sequence.has_notes());
        assert_eq!(sequence.programs_used(), 1 << 1);
        assert_eq!(lowest_program(sequence.programs_used()), 1);
        assert_eq!(sequence.events[5].kind(), Kind::Meta);
        assert_eq!(sep.problems(), []);
        assert_eq!(sep.encode(), tiny_sep());
    }

    #[test]
    fn midi_export_has_header_and_track() {
        let sep = Package::parse(&tiny_sep()).unwrap();
        let midi = sep.to_midi(0).unwrap();
        assert_eq!(&midi[..4], b"MThd");
        assert!(midi.windows(4).any(|window| window == b"MTrk"));
        assert_eq!(sep.to_midi(5), Err(Error::SepSequenceOutOfRange { index: 5, count: 1 }));
    }

    #[test]
    fn a_sequence_comes_back_from_its_midi_file() {
        let package = Package::parse(&tiny_sep()).unwrap();
        let midi = package.to_midi(0).unwrap();
        let sequence = Sequence::from_midi(&midi, package.sequences[0].seq_id).unwrap();
        assert_eq!(sequence, package.sequences[0]);
        assert_eq!(Package { sequences: vec![sequence] }.encode(), package.encode());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Package::parse(b"not a sep file at all, really").is_err());
        let mut bad = tiny_sep();
        let length = bad.len();
        bad.truncate(length - 3);
        assert!(Package::parse(&bad).is_err());
    }
}
