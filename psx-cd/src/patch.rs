//! Patched disc images: files of the data track replaced in place, with every
//! sector they change sealed again.
//!
//! The data track is Mode 2 Form 1, so each sector's EDC and parity come from
//! `psx_formats::sector` (`psx-spx` "CDROM Sector Encoding"). A replacement
//! keeps the file's sectors and can be shorter than the stored file, never
//! longer: the disc has no room to move the files after it. The directory
//! record that gives the file's size is updated, and so is the sector that
//! holds it. Form 2 streams (XA and STR) are not replaced. Every other sector,
//! the audio track and the cue sheet are copied as stored.
//!
//! # Examples
//!
//! ```no_run
//! # fn main() -> Result<(), psx_cd::Error> {
//! use psx_cd::patch::Replacement;
//!
//! let summary = psx_cd::patch::apply(
//!     std::path::Path::new("disc.cue"),
//!     std::path::Path::new("patched"),
//!     &[Replacement { path: "DATA.BIN".into(), bytes: b"new".to_vec() }],
//! )?;
//! assert_eq!(summary.files, 1);
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use psx_formats::bytes::u32_le;
use psx_formats::sector::{self, RAW_SIZE};

use crate::cue;
use crate::error::{Error, Result};
use crate::{Entry, Image};

/// The user data of a Form 1 sector starts after the sync (12), the header (4) and the sub-header (8).
const USER_AT: usize = 24;
const USER_BYTES: usize = 2048;
/// The primary volume descriptor, and the root directory record at its byte 156 (ECMA-119 8.5, 9.1).
const PVD_SECTOR: u32 = 16;
const ROOT_RECORD_AT: usize = 156;
const ROOT_RECORD_BYTES: usize = 34;
/// Offsets inside a directory record. The size is stored twice, little endian then big endian.
const RECORD_EXTENT_AT: usize = 2;
const RECORD_SIZE_AT: usize = 10;
const RECORD_FLAGS_AT: usize = 25;
const RECORD_NAME_LENGTH_AT: usize = 32;
const RECORD_NAME_AT: usize = 33;
const FLAG_DIRECTORY: u8 = 0x02;

/// One file to replace: its path on the disc, such as `DATA.BIN` or
/// `DIR/FILE.BIN`, and its new bytes.
///
/// # Examples
///
/// ```
/// let replacement = psx_cd::patch::Replacement { path: "DATA.BIN".into(), bytes: b"new".to_vec() };
/// assert_eq!(replacement.path, std::path::Path::new("DATA.BIN"));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Replacement {
    /// Path of the file in the image's file table.
    pub path: PathBuf,
    /// Bytes to write; at most the room the file's sectors hold.
    pub bytes: Vec<u8>,
}

/// What a patch wrote.
///
/// # Examples
///
/// ```
/// let summary = psx_cd::patch::Summary { files: 2, sealed: 5 };
/// assert_eq!(summary.files + summary.sealed as usize, 7);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    /// Files replaced.
    pub files: usize,
    /// Sectors sealed: every sector of a replaced file, and each directory record's sector.
    pub sealed: u64,
}

/// Writes a patched copy of the disc named by `cue_path` into the directory
/// `output`. The copy has the cue sheet and every track file under their own
/// names, with the replacements applied to the data track.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> Result<(), psx_cd::Error> {
/// use psx_cd::patch::{Replacement, apply};
///
/// let summary = apply(
///     std::path::Path::new("disc.cue"),
///     std::path::Path::new("patched"),
///     &[Replacement { path: "DATA.BIN".into(), bytes: b"new".to_vec() }],
/// )?;
/// assert_eq!(summary.files, 1);
/// # Ok(())
/// # }
/// ```
pub fn apply(cue_path: &Path, output: &Path, replacements: &[Replacement]) -> Result<Summary> {
    let sheet = cue::read(cue_path)?;
    let data_track = sheet
        .tracks
        .iter()
        .find(|track| track.mode != cue::TrackMode::Audio)
        .ok_or_else(|| Error::Patch(format!("{} has no data track", cue_path.display())))?;
    let mut image = Image::open(cue_path)?;
    let entries = image.entries()?;
    let mut targets = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        targets.push(target(&entries, replacement)?);
    }

    if same_directory(cue_path, output)? {
        return Err(Error::Patch("the patched disc goes to a new directory, not the one the cue sheet is in".into()));
    }
    fs::create_dir_all(output)?;
    let mut copied = BTreeSet::new();
    for track in &sheet.tracks {
        if copied.insert(track.file.clone()) {
            fs::copy(&track.file, output.join(file_name(&track.file)?))?;
        }
    }
    fs::copy(cue_path, output.join(file_name(cue_path)?))?;

    let mut file = OpenOptions::new().read(true).write(true).open(output.join(file_name(&data_track.file)?))?;
    let mut summary = Summary::default();
    for (entry, replacement) in targets.iter().zip(replacements) {
        summary.sealed += replace(&mut file, entry, &replacement.bytes)?;
        summary.files += 1;
    }
    file.flush()?;
    Ok(summary)
}

/// The file a replacement names, checked against the disc: a regular file, not a Form 2 stream, and no larger
/// than the sectors it occupies.
fn target(entries: &[Entry], replacement: &Replacement) -> Result<Entry> {
    let name = replacement.path.display();
    let entry = entries
        .iter()
        .find(|entry| !entry.is_directory && entry.path == replacement.path)
        .ok_or_else(|| Error::Patch(format!("the disc has no file {name}")))?;
    if entry.is_xa {
        return Err(Error::Patch(format!("{name} is a Form 2 stream, which cannot be replaced")));
    }
    let room = entry.sectors() * USER_BYTES as u64;
    if replacement.bytes.len() as u64 > room {
        return Err(Error::Patch(format!(
            "{name} grows from {} to {} bytes; the disc holds {room} for it",
            entry.size,
            replacement.bytes.len()
        )));
    }
    Ok(entry.clone())
}

/// Writes `bytes` over the file's sectors, then its directory records. Returns the sectors sealed.
fn replace(file: &mut File, entry: &Entry, bytes: &[u8]) -> Result<u64> {
    let mut sealed = 0;
    for index in 0..entry.sectors() {
        let lba = sector_of(entry.lba, index)?;
        let mut raw = read_raw(file, lba)?;
        if sector::is_form2(&raw) {
            return Err(Error::Patch(format!("sector {lba} is a Form 2 stream, not a file sector")));
        }
        let start = index as usize * USER_BYTES;
        let user = &mut raw[USER_AT..USER_AT + USER_BYTES];
        if start < bytes.len() {
            let end = bytes.len().min(start + USER_BYTES);
            user[..end - start].copy_from_slice(&bytes[start..end]);
            if bytes.len() < entry.size as usize {
                user[end - start..].fill(0);
            }
        } else {
            user.fill(0);
        }
        sector::seal(&mut raw);
        write_raw(file, lba, &raw)?;
        sealed += 1;
    }
    let size = u32::try_from(bytes.len()).map_err(|_| Error::Patch("a replacement is over 4 GiB".into()))?;
    for (lba, offset) in records_of(file, entry.lba, entry.size)? {
        let mut raw = read_raw(file, lba)?;
        let record = USER_AT + offset + RECORD_SIZE_AT;
        raw[record..record + 4].copy_from_slice(&size.to_le_bytes());
        raw[record + 4..record + 8].copy_from_slice(&size.to_be_bytes());
        sector::seal(&mut raw);
        write_raw(file, lba, &raw)?;
        sealed += 1;
    }
    Ok(sealed)
}

/// Every directory record of a regular file with this extent and size, as the sector that holds it and its
/// byte offset in that sector's user data.
fn records_of(file: &mut File, lba: u32, size: u32) -> Result<Vec<(u32, usize)>> {
    let pvd = read_raw(file, PVD_SECTOR)?;
    let root = &pvd[USER_AT + ROOT_RECORD_AT..USER_AT + ROOT_RECORD_AT + ROOT_RECORD_BYTES];
    let mut pending = vec![(u32_le(root, RECORD_EXTENT_AT), u32_le(root, RECORD_SIZE_AT))];
    let mut found = Vec::new();
    while let Some((extent, length)) = pending.pop() {
        let data = read_extent(file, extent, length)?;
        let mut cursor = 0;
        while cursor < data.len() {
            let record_length = usize::from(data[cursor]);
            if record_length == 0 {
                // A record never straddles a sector: skip to the next one.
                cursor = (cursor / USER_BYTES + 1) * USER_BYTES;
                continue;
            }
            let record = data
                .get(cursor..cursor + record_length)
                .ok_or_else(|| Error::Patch("a directory record is truncated".into()))?;
            let name_length = usize::from(record[RECORD_NAME_LENGTH_AT]);
            let name = record.get(RECORD_NAME_AT..RECORD_NAME_AT + name_length).unwrap_or_default();
            // Identifiers 0 and 1 are the directory itself and its parent.
            if name != [0] && name != [1] {
                let record_extent = u32_le(record, RECORD_EXTENT_AT);
                if record[RECORD_FLAGS_AT] & FLAG_DIRECTORY != 0 {
                    pending.push((record_extent, u32_le(record, RECORD_SIZE_AT)));
                } else if record_extent == lba && u32_le(record, RECORD_SIZE_AT) == size {
                    let sector_of_record = sector_of(extent, (cursor / USER_BYTES) as u64)?;
                    found.push((sector_of_record, cursor % USER_BYTES));
                }
            }
            cursor += record_length;
        }
    }
    Ok(found)
}

/// The user bytes of `length` bytes from the sector `extent` on.
fn read_extent(file: &mut File, extent: u32, length: u32) -> Result<Vec<u8>> {
    let sectors = (length as usize).div_ceil(USER_BYTES);
    let mut bytes = Vec::with_capacity(sectors * USER_BYTES);
    for index in 0..sectors {
        let raw = read_raw(file, sector_of(extent, index as u64)?)?;
        bytes.extend_from_slice(&raw[USER_AT..USER_AT + USER_BYTES]);
    }
    bytes.truncate(length as usize);
    Ok(bytes)
}

/// The sector `index` sectors after `start`.
fn sector_of(start: u32, index: u64) -> Result<u32> {
    u64::from(start)
        .checked_add(index)
        .and_then(|sector| u32::try_from(sector).ok())
        .ok_or_else(|| Error::Patch("a sector number is past the disc".into()))
}

fn read_raw(file: &mut File, sector: u32) -> Result<[u8; RAW_SIZE]> {
    let mut raw = [0; RAW_SIZE];
    file.seek(SeekFrom::Start(u64::from(sector) * RAW_SIZE as u64))?;
    file.read_exact(&mut raw)?;
    Ok(raw)
}

fn write_raw(file: &mut File, sector: u32, raw: &[u8; RAW_SIZE]) -> Result<()> {
    file.seek(SeekFrom::Start(u64::from(sector) * RAW_SIZE as u64))?;
    file.write_all(raw)?;
    Ok(())
}

fn file_name(path: &Path) -> Result<&std::ffi::OsStr> {
    path.file_name().ok_or_else(|| Error::Patch(format!("{} has no file name", path.display())))
}

/// Whether `output` already is the directory the cue sheet is in: copying into it would overwrite the source.
fn same_directory(cue_path: &Path, output: &Path) -> Result<bool> {
    if !output.exists() {
        return Ok(false);
    }
    let source = cue_path.parent().unwrap_or(Path::new("."));
    Ok(fs::canonicalize(source)? == fs::canonicalize(output)?)
}
