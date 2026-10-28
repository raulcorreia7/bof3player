//! A deterministic cooked ISO9660 image built from a folder of files.
//!
//! The image has a primary volume descriptor at sector 16, a terminator at 17,
//! a flat root directory at 18 and the sorted files from sector 19 on. It is a
//! fixture builder: the input is top-level regular files with conservative
//! names, at most 32 MiB in total.
//!
//! # Examples
//!
//! ```
//! let dir = tempfile::tempdir().unwrap();
//! std::fs::write(dir.path().join("HELLO.TXT"), b"hello").unwrap();
//! let output = dir.path().join("disc.iso");
//! psx_cd::rebuild::iso(dir.path(), &output).unwrap();
//! assert_eq!(std::fs::metadata(output).unwrap().len(), 20 * 2048);
//! ```

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

const SECTOR_SIZE: usize = 2_048;
const PVD_SECTOR: usize = 16;
const ROOT_SECTOR: usize = 18;
const FIRST_FILE_SECTOR: usize = 19;
const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;

struct FileEntry {
    name: String,
    bytes: Vec<u8>,
    sector: usize,
}

/// Text a rebuilt image writes into its primary volume descriptor.
///
/// # Examples
///
/// ```
/// let labels = psx_cd::rebuild::VolumeLabels { system: "PLAYSTATION", volume: "FIXTURE" };
/// assert_eq!(labels.volume, "FIXTURE");
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeLabels<'a> {
    /// System identifier, PVD bytes 8..40.
    pub system: &'a str,
    /// Volume identifier, PVD bytes 40..72.
    pub volume: &'a str,
}

impl Default for VolumeLabels<'static> {
    fn default() -> Self {
        Self { system: "PLAYSTATION", volume: "PSX-DISK" }
    }
}

/// Rebuilds a deterministic cooked ISO9660 image from top-level regular files,
/// with neutral PVD labels.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("HELLO.TXT"), b"hello").unwrap();
/// let output = dir.path().join("disc.iso");
/// psx_cd::rebuild::iso(dir.path(), &output).unwrap();
/// assert!(output.is_file());
/// ```
pub fn iso(input: &Path, output: &Path) -> Result<(), Error> {
    iso_with_labels(input, output, VolumeLabels::default())
}

/// Rebuilds a deterministic cooked ISO9660 image from top-level regular files
/// with the given PVD labels.
///
/// Each label is padded with spaces to its 32-byte field. A label longer than
/// 32 bytes, or with non-ASCII text, is an error.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("HELLO.TXT"), b"hello").unwrap();
/// let output = dir.path().join("disc.iso");
/// let labels = psx_cd::rebuild::VolumeLabels { system: "PLAYSTATION", volume: "FIXTURE" };
/// psx_cd::rebuild::iso_with_labels(dir.path(), &output, labels).unwrap();
/// let bytes = std::fs::read(output).unwrap();
/// assert_eq!(&bytes[16 * 2048 + 40..16 * 2048 + 47], b"FIXTURE");
/// ```
pub fn iso_with_labels(input: &Path, output: &Path, labels: VolumeLabels<'_>) -> Result<(), Error> {
    let mut files = read_input(input)?;
    let mut sector = FIRST_FILE_SECTOR;
    for file in &mut files {
        file.sector = sector;
        sector = sector
            .checked_add(file.bytes.len().div_ceil(SECTOR_SIZE))
            .ok_or(Error::InvalidInput("rebuilt image is too large"))?;
    }
    if sector > u32::MAX as usize {
        return Err(Error::InvalidInput("rebuilt image is too large"));
    }

    let mut image = vec![0; sector * SECTOR_SIZE];
    write_pvd(&mut image[PVD_SECTOR * SECTOR_SIZE..(PVD_SECTOR + 1) * SECTOR_SIZE], sector, labels)?;
    {
        let root = &mut image[ROOT_SECTOR * SECTOR_SIZE..(ROOT_SECTOR + 1) * SECTOR_SIZE];
        write_record(root, 0, ROOT_SECTOR as u32, SECTOR_SIZE as u32, 2, &[0])?;
        let mut cursor = record_len(1);
        write_record(root, cursor, ROOT_SECTOR as u32, SECTOR_SIZE as u32, 2, &[1])?;
        cursor += record_len(1);
        for file in &files {
            let identifier = format!("{};1", file.name);
            let length = record_len(identifier.len());
            if cursor + length > root.len() {
                return Err(Error::InvalidInput("too many files for the synthetic root directory"));
            }
            write_record(root, cursor, file.sector as u32, file.bytes.len() as u32, 0, identifier.as_bytes())?;
            cursor += length;
        }
    }
    for file in &files {
        let start = file.sector * SECTOR_SIZE;
        image[start..start + file.bytes.len()].copy_from_slice(&file.bytes);
    }

    let terminator = &mut image[(PVD_SECTOR + 1) * SECTOR_SIZE..(PVD_SECTOR + 2) * SECTOR_SIZE];
    terminator[0] = 255;
    terminator[1..6].copy_from_slice(b"CD001");
    terminator[6] = 1;
    crate::ensure_parent(output)?;
    fs::write(output, image)?;
    Ok(())
}

fn read_input(input: &Path) -> Result<Vec<FileEntry>, Error> {
    if !input.is_dir() {
        return Err(Error::InvalidInput("rebuild input must be a directory"));
    }
    let mut files = Vec::new();
    let mut total = 0_usize;
    for entry in fs::read_dir(input)? {
        let entry = entry?;
        let name =
            entry.file_name().into_string().map_err(|_| Error::InvalidInput("rebuild input name is not UTF-8"))?;
        validate_name(&name)?;
        if !entry.file_type()?.is_file() {
            return Err(Error::InvalidInput("rebuild input supports only top-level regular files"));
        }
        let bytes = fs::read(entry.path())?;
        total = total.checked_add(bytes.len()).ok_or(Error::InvalidInput("rebuild input is too large"))?;
        if total > MAX_INPUT_BYTES {
            return Err(Error::InvalidInput("rebuild input exceeds 32 MiB"));
        }
        files.push(FileEntry { name, bytes, sector: 0 });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    if files.is_empty() {
        return Err(Error::InvalidInput("rebuild input has no files"));
    }
    Ok(files)
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 220
        || !name.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_'))
    {
        return Err(Error::InvalidInput(
            "rebuild filenames must be uppercase ASCII letters, digits, dot, or underscore",
        ));
    }
    Ok(())
}

fn write_pvd(pvd: &mut [u8], sectors: usize, labels: VolumeLabels<'_>) -> Result<(), Error> {
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    write_label(&mut pvd[8..40], labels.system)?;
    write_label(&mut pvd[40..72], labels.volume)?;
    write_both_endian(&mut pvd[80..88], sectors as u32);
    write_both_endian_u16(&mut pvd[120..124], 1);
    write_both_endian_u16(&mut pvd[124..128], 1);
    write_both_endian_u16(&mut pvd[128..132], SECTOR_SIZE as u16);
    write_record(pvd, 156, ROOT_SECTOR as u32, SECTOR_SIZE as u32, 2, &[0])
}

fn write_label(field: &mut [u8], value: &str) -> Result<(), Error> {
    if !value.is_ascii() {
        return Err(Error::InvalidInput("primary volume descriptor label is not ASCII"));
    }
    if value.len() > field.len() {
        return Err(Error::InvalidInput("primary volume descriptor label exceeds 32 bytes"));
    }
    field.fill(b' ');
    field[..value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

fn write_record(
    destination: &mut [u8],
    offset: usize,
    extent: u32,
    size: u32,
    flags: u8,
    name: &[u8],
) -> Result<(), Error> {
    let length = record_len(name.len());
    let record = destination
        .get_mut(offset..offset + length)
        .ok_or(Error::InvalidInput("synthetic ISO directory record overflows"))?;
    record[0] = length as u8;
    write_both_endian(&mut record[2..10], extent);
    write_both_endian(&mut record[10..18], size);
    record[25] = flags;
    write_both_endian_u16(&mut record[28..32], 1);
    record[32] = name.len() as u8;
    record[33..33 + name.len()].copy_from_slice(name);
    Ok(())
}

fn record_len(name_len: usize) -> usize {
    33 + name_len + usize::from(name_len.is_multiple_of(2))
}

fn write_both_endian(destination: &mut [u8], value: u32) {
    destination[..4].copy_from_slice(&value.to_le_bytes());
    destination[4..8].copy_from_slice(&value.to_be_bytes());
}

fn write_both_endian_u16(destination: &mut [u8], value: u16) {
    destination[..2].copy_from_slice(&value.to_le_bytes());
    destination[2..4].copy_from_slice(&value.to_be_bytes());
}
