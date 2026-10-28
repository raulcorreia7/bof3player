//! CUE sheet parsing and CD-DA track extraction.
//!
//! A sheet names its `FILE`s, their `TRACK`s and each track's `INDEX` points.
//! Positions are stored in sectors (75 per second); an audio track is a run of
//! 2352-byte CD-DA frames (`psx-spx` "CDROM Disk Format").
//!
//! # Examples
//!
//! ```
//! use std::path::Path;
//!
//! let sheet = psx_cd::cue::parse(
//!     "FILE \"disc.bin\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00",
//!     Path::new("."),
//! )
//! .unwrap();
//! assert_eq!(sheet.tracks[0].mode, psx_cd::cue::TrackMode::Mode2_2352);
//! ```

use std::fs;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Bytes read from an audio track per system call.
const AUDIO_READ_BUFFER: usize = 1 << 20;

/// A parsed CUE sheet: its tracks in the order the sheet lists them.
///
/// # Examples
///
/// ```
/// let sheet = psx_cd::cue::parse(
///     "FILE \"disc.bin\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00",
///     std::path::Path::new("."),
/// )
/// .unwrap();
/// assert_eq!(sheet.tracks.len(), 1);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sheet {
    /// Tracks in sheet order.
    pub tracks: Vec<Track>,
}

/// One track of a CUE sheet.
///
/// # Examples
///
/// ```
/// let track = psx_cd::cue::Track {
///     number: 2,
///     mode: psx_cd::cue::TrackMode::Audio,
///     file: "track02.bin".into(),
///     index00: Some(0),
///     index01: 150,
///     pregap: None,
/// };
/// assert_eq!(track.index01, 150);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Track {
    /// Track number as the sheet writes it (`01` through `99`).
    pub number: u8,
    /// How the track stores its sectors.
    pub mode: TrackMode,
    /// The `FILE` entry this track's sectors live in.
    pub file: PathBuf,
    /// Sector of `INDEX 00`, the pause before the track, when the sheet has one.
    pub index00: Option<u32>,
    /// Sector of `INDEX 01`, where the track starts.
    pub index01: u32,
    /// Sectors of `PREGAP` the sheet declares but the file does not store.
    pub pregap: Option<u32>,
}

/// How a CUE track stores its sectors.
///
/// # Examples
///
/// ```
/// let mode = psx_cd::cue::TrackMode::Mode2_2352;
/// assert_ne!(mode, psx_cd::cue::TrackMode::Audio);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackMode {
    /// Red Book audio: 2352-byte CD-DA frames.
    Audio,
    /// Mode 1 data with 2048 user bytes per sector.
    Mode1_2048,
    /// Mode 1 data with 2352 bytes per sector, header included.
    Mode1_2352,
    /// Mode 2 data without sync and header: 2336 bytes per sector.
    Mode2_2336,
    /// Mode 2 data with 2352 bytes per sector, sync, header and subheader included.
    Mode2_2352,
}

/// Reads and parses a CUE sheet from `path`; relative `FILE` names resolve
/// against its directory.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> Result<(), psx_cd::Error> {
/// let sheet = psx_cd::cue::read(std::path::Path::new("disc.cue"))?;
/// assert!(!sheet.tracks.is_empty());
/// # Ok(())
/// # }
/// ```
pub fn read(path: &Path) -> Result<Sheet, Error> {
    parse(&fs::read_to_string(path)?, path.parent().unwrap_or(Path::new(".")))
}

/// Bytes in one CD-DA sector: 588 stereo frames of 16-bit samples.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_cd::cue::AUDIO_SECTOR_SIZE, 2352);
/// ```
pub const AUDIO_SECTOR_SIZE: u64 = 2352;

/// The sectors `[start, end)` of the audio track at `position` in its file. A
/// track ends where the next track of the same file starts (its pregap, when it
/// has one), or at the end of the file.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("track.bin"), vec![0_u8; 2 * 2352]).unwrap();
/// let sheet = psx_cd::cue::parse(
///     "FILE \"track.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
///     dir.path(),
/// )
/// .unwrap();
/// assert_eq!(psx_cd::cue::audio_track_range(&sheet, 0).unwrap(), (0, 2));
/// ```
pub fn audio_track_range(sheet: &Sheet, position: usize) -> Result<(u64, u64)> {
    let track = &sheet.tracks[position];
    let file_sectors = fs::metadata(&track.file)?.len() / AUDIO_SECTOR_SIZE;
    let end_sector = sheet.tracks[position + 1..]
        .iter()
        .find(|next| next.file == track.file)
        .map_or(file_sectors, |next| u64::from(next.index00.unwrap_or(next.index01)));
    let start_sector = u64::from(track.index01);
    if start_sector > end_sector || end_sector > file_sectors {
        return Err(Error::InvalidImage("cue audio track range exceeds its file"));
    }
    Ok((start_sector, end_sector))
}

/// Writes every audio track as a 44.1 kHz stereo 16-bit WAV file `trackNN.wav`.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("track.bin"), vec![0_u8; 2352]).unwrap();
/// let sheet = psx_cd::cue::parse(
///     "FILE \"track.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
///     dir.path(),
/// )
/// .unwrap();
/// let files = psx_cd::cue::extract_audio_tracks(&sheet, &dir.path().join("out")).unwrap();
/// assert!(files[0].ends_with("track02.wav"));
/// ```
pub fn extract_audio_tracks(sheet: &Sheet, output: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(output)?;
    let mut written = Vec::new();
    for (position, track) in sheet.tracks.iter().enumerate() {
        if track.mode != TrackMode::Audio {
            continue;
        }
        let (start_sector, end_sector) = audio_track_range(sheet, position)?;
        let mut source = BufReader::with_capacity(AUDIO_READ_BUFFER, File::open(&track.file)?);
        source.seek(SeekFrom::Start(start_sector * AUDIO_SECTOR_SIZE))?;
        let sample_count = (end_sector - start_sector)
            .checked_mul(AUDIO_SECTOR_SIZE / 2)
            .ok_or(Error::InvalidImage("cue audio sample count overflows"))?;
        let path = output.join(format!("track{:02}.wav", track.number));
        let mut samples = Vec::with_capacity(usize::try_from(sample_count).unwrap_or(0));
        let mut sample = [0; 2];
        for _ in 0..sample_count {
            source.read_exact(&mut sample)?;
            samples.push(i16::from_le_bytes(sample));
        }
        let bytes = psx_formats::wav::pcm(&samples, 44_100, 2)
            .map_err(|error| Error::InvalidImageDetail(format!("cannot write WAV: {error}")))?;
        fs::write(&path, bytes)?;
        written.push(path);
    }
    Ok(written)
}

/// Writes every audio track's sectors as the disc stores them, `trackNN.pcm`:
/// little-endian 16-bit stereo samples at 44.1 kHz, 2352 bytes a sector, with
/// no header.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("track.bin"), vec![0_u8; 2352]).unwrap();
/// let sheet = psx_cd::cue::parse(
///     "FILE \"track.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
///     dir.path(),
/// )
/// .unwrap();
/// let files = psx_cd::cue::extract_raw_audio_tracks(&sheet, &dir.path().join("out")).unwrap();
/// assert!(files[0].ends_with("track02.pcm"));
/// assert_eq!(std::fs::metadata(&files[0]).unwrap().len(), 2352);
/// ```
pub fn extract_raw_audio_tracks(sheet: &Sheet, output: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(output)?;
    let mut written = Vec::new();
    for (position, track) in sheet.tracks.iter().enumerate() {
        if track.mode != TrackMode::Audio {
            continue;
        }
        let (start_sector, end_sector) = audio_track_range(sheet, position)?;
        let mut source = BufReader::with_capacity(AUDIO_READ_BUFFER, File::open(&track.file)?);
        source.seek(SeekFrom::Start(start_sector * AUDIO_SECTOR_SIZE))?;
        let path = output.join(format!("track{:02}.pcm", track.number));
        let mut target = File::create(&path)?;
        let wanted = (end_sector - start_sector) * AUDIO_SECTOR_SIZE;
        if io::copy(&mut source.take(wanted), &mut target)? != wanted {
            return Err(Error::InvalidImage("cue audio track is shorter than its range"));
        }
        written.push(path);
    }
    Ok(written)
}

/// Parses CUE text; relative `FILE` names resolve against `root`.
///
/// Unknown commands are ignored, `INDEX 00` and `INDEX 01` are kept, and a
/// track without `INDEX 01` is an error.
///
/// # Examples
///
/// ```
/// let sheet = psx_cd::cue::parse(
///     "FILE \"data.bin\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00",
///     std::path::Path::new("disc"),
/// )
/// .unwrap();
/// assert_eq!(sheet.tracks[0].file, std::path::Path::new("disc/data.bin"));
/// ```
pub fn parse(text: &str, root: &Path) -> Result<Sheet, Error> {
    let mut current_file = None;
    let mut tracks: Vec<Track> = Vec::new();
    for source_line in text.lines() {
        let line = source_line.trim();
        if line.is_empty() || line.starts_with("REM ") {
            continue;
        }
        let (command, rest) = split_word(line);
        match command.to_ascii_uppercase().as_str() {
            "FILE" => {
                let (name, _) = parse_file(rest)?;
                current_file = Some(root.join(name));
            }
            "TRACK" => {
                let file = current_file.clone().ok_or(Error::InvalidImage("cue TRACK appears before FILE"))?;
                let (number, mode) = parse_track(rest)?;
                if tracks.iter().any(|track| track.number == number) {
                    return Err(Error::InvalidImage("cue track number is duplicated"));
                }
                tracks.push(Track { number, mode, file, index00: None, index01: u32::MAX, pregap: None });
            }
            "INDEX" => {
                let track = tracks.last_mut().ok_or(Error::InvalidImage("cue INDEX appears before TRACK"))?;
                let (number, timestamp) = split_word(rest);
                let sector = parse_timecode(timestamp)?;
                match number {
                    "00" => track.index00 = Some(sector),
                    "01" => track.index01 = sector,
                    _ => {}
                }
            }
            "PREGAP" => {
                let track = tracks.last_mut().ok_or(Error::InvalidImage("cue PREGAP appears before TRACK"))?;
                track.pregap = Some(parse_timecode(rest.trim())?);
            }
            _ => {}
        }
    }
    if tracks.is_empty() {
        return Err(Error::InvalidImage("cue sheet has no tracks"));
    }
    if tracks.iter().any(|track| track.index01 == u32::MAX) {
        return Err(Error::InvalidImage("cue track has no INDEX 01"));
    }
    Ok(Sheet { tracks })
}

fn parse_file(value: &str) -> Result<(&str, &str), Error> {
    let value = value.trim_start();
    if let Some(quoted) = value.strip_prefix('"') {
        let end = quoted.find('"').ok_or(Error::InvalidImage("unterminated quoted cue FILE path"))?;
        let name = &quoted[..end];
        if name.is_empty() {
            return Err(Error::InvalidImage("empty cue FILE path"));
        }
        return Ok((name, quoted[end + 1..].trim()));
    }
    let (name, kind) = split_word(value);
    if name.is_empty() {
        return Err(Error::InvalidImage("empty cue FILE path"));
    }
    Ok((name, kind))
}

fn parse_track(value: &str) -> Result<(u8, TrackMode), Error> {
    let (number, mode) = split_word(value.trim());
    let number = number.parse().map_err(|_| Error::InvalidImage("invalid cue track number"))?;
    let mode = match mode.to_ascii_uppercase().as_str() {
        "AUDIO" => TrackMode::Audio,
        "MODE1/2048" => TrackMode::Mode1_2048,
        "MODE1/2352" => TrackMode::Mode1_2352,
        "MODE2/2336" => TrackMode::Mode2_2336,
        "MODE2/2352" => TrackMode::Mode2_2352,
        _ => return Err(Error::InvalidImage("unsupported cue track mode")),
    };
    Ok((number, mode))
}

pub(crate) fn parse_timecode(value: &str) -> Result<u32, Error> {
    let mut parts = value.split(':');
    let minutes: u32 = parse_time_part(parts.next())?;
    let seconds: u32 = parse_time_part(parts.next())?;
    let frames: u32 = parse_time_part(parts.next())?;
    if parts.next().is_some() || seconds >= 60 || frames >= 75 {
        return Err(Error::InvalidImage("invalid cue timecode"));
    }
    minutes
        .checked_mul(60 * 75)
        .and_then(|value| value.checked_add(seconds * 75 + frames))
        .ok_or(Error::InvalidImage("cue timecode overflows"))
}

pub(crate) fn format_timecode(sectors: u32) -> String {
    let minutes = sectors / (60 * 75);
    let remainder = sectors % (60 * 75);
    format!("{minutes:02}:{:02}:{:02}", remainder / 75, remainder % 75)
}

fn parse_time_part(value: Option<&str>) -> Result<u32, Error> {
    value
        .ok_or(Error::InvalidImage("invalid cue timecode"))?
        .parse()
        .map_err(|_| Error::InvalidImage("invalid cue timecode"))
}

fn split_word(value: &str) -> (&str, &str) {
    value.split_once(char::is_whitespace).map_or((value, ""), |(word, rest)| (word, rest.trim_start()))
}
