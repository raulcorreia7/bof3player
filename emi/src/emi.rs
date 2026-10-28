//! EMI container: header, table of contents and sector-aligned payloads.
//!
//! [`Archive::parse`] reads the table from bytes and needs no `std`; the file operations
//! ([`Archive::open`], extraction, [`read_entry`]) are behind the `std` feature.

use alloc::vec::Vec;

use psx_formats::bytes::{u16_le, u32_le};

use crate::error::{Error, Result};

#[cfg(feature = "std")]
use std::fs::{self, File};
#[cfg(feature = "std")]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

#[cfg(feature = "std")]
use psx_formats::{sep, vab};

/// Header magic at offset 8.
pub const MAGIC: &[u8; 8] = b"MATH_TBL";
/// Payloads start on this boundary, and the table must fit before the first.
pub const SECTOR_SIZE: u64 = 0x800;
/// Bytes before the first table entry: count, version and magic.
pub const HEADER_SIZE: u64 = 0x10;
/// Bytes per table entry.
pub const TOC_ENTRY_SIZE: u64 = 0x10;
/// Entry type of a raw VRAM upload.
#[cfg(feature = "std")]
pub const TYPE_VRAM: u16 = psx_formats::vram::UPLOAD_TYPE;
/// Entry type of an LZ77-packed payload.
pub const TYPE_LZ77: u16 = 1;
/// Entry type of a type-8 cue table.
pub const TYPE_CUE: u16 = 8;

/// One table entry and the position of its payload in the container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Payload size in bytes, from the table record.
    pub size: u32,
    /// Byte offset of the payload in the container; payloads are sector aligned.
    pub offset: u64,
    /// The `u32` at byte 4 of the table record.
    pub ram_ptr: u32,
    /// The `u32` at byte 8 of the table record.
    pub first4: u32,
    /// The entry type, the `u16` at byte 12 of the table record (see the `TYPE_*` constants).
    pub file_type: u16,
}

/// A parsed container table; the payload bytes stay where they were read from.
#[derive(Debug)]
pub struct Archive {
    #[cfg(feature = "std")]
    source: Option<PathBuf>,
    version: u32,
    entries: Vec<Entry>,
}

impl Archive {
    /// Reads and validates the table of a container held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::read_table(bytes, bytes.len() as u64)
    }

    /// Reads and validates the table from `head`, the first bytes of a container of `file_len`
    /// bytes: at least the header and the table, which end before the first payload.
    fn read_table(head: &[u8], file_len: u64) -> Result<Self> {
        if head.len() < HEADER_SIZE as usize {
            return Err(Error::InvalidArchive("file is shorter than the header"));
        }
        let count = u32_le(head, 0) as usize;
        let version = u32_le(head, 4);
        if &head[8..16] != MAGIC {
            return Err(Error::InvalidArchive("bad magic"));
        }

        let toc_size =
            (count as u64).checked_mul(TOC_ENTRY_SIZE).ok_or(Error::InvalidArchive("entry table overflows"))?;
        let toc_end = HEADER_SIZE.checked_add(toc_size).ok_or(Error::InvalidArchive("entry table overflows"))?;
        if toc_end > SECTOR_SIZE || toc_end > file_len || toc_end > head.len() as u64 {
            return Err(Error::InvalidArchive("entry table does not fit before data"));
        }

        let mut offset = SECTOR_SIZE;
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let at = (HEADER_SIZE + index as u64 * TOC_ENTRY_SIZE) as usize;
            let size = u32_le(head, at);
            let end = offset.checked_add(u64::from(size)).ok_or(Error::InvalidArchive("entry range overflows"))?;
            if end > file_len {
                return Err(Error::InvalidArchive("entry extends beyond end of file"));
            }
            entries.push(Entry {
                size,
                offset,
                ram_ptr: u32_le(head, at + 4),
                first4: u32_le(head, at + 8),
                file_type: u16_le(head, at + 12),
            });
            offset = align_sector(end)?;
        }

        Ok(Self {
            #[cfg(feature = "std")]
            source: None,
            version,
            entries,
        })
    }

    /// The table's version word.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The table entries, in table order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The payload of entry `index` inside `container`, the bytes this table was
    /// parsed from.
    pub fn payload<'a>(&self, container: &'a [u8], index: usize) -> Result<&'a [u8]> {
        let entry = self.entry(index)?;
        let start = entry.offset as usize;
        container
            .get(start..start + entry.size as usize)
            .ok_or(Error::InvalidArchive("entry extends beyond end of file"))
    }

    fn entry(&self, index: usize) -> Result<&Entry> {
        self.entries.get(index).ok_or(Error::EntryOutOfRange { index, count: self.entries.len() })
    }
}

#[cfg(feature = "std")]
impl Archive {
    /// Reads and validates the table of the container at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let source = path.as_ref().to_path_buf();
        let mut archive = Self::from_reader(&mut File::open(&source)?)?;
        archive.source = Some(source);
        Ok(archive)
    }

    /// Reads and validates the table of a container read from `reader`; only the first sector is
    /// read. Payloads are read later with [`Archive::read_payload`].
    pub fn from_reader<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut head = Vec::new();
        reader.take(SECTOR_SIZE).read_to_end(&mut head)?;
        Self::read_table(&head, file_len)
    }

    /// The payload of entry `index`, read from `reader`: the stream this table was read from.
    pub fn read_payload<R: Read + Seek>(&self, reader: &mut R, index: usize) -> Result<Vec<u8>> {
        let entry = self.entry(index)?;
        reader.seek(SeekFrom::Start(entry.offset))?;
        let mut payload = vec![0; entry.size as usize];
        reader.read_exact(&mut payload)?;
        Ok(payload)
    }

    fn source(&self) -> Result<&Path> {
        self.source.as_deref().ok_or(Error::InvalidArchive("archive was parsed from memory"))
    }

    /// Writes entry `index` to `path`, creating parent directories.
    pub fn extract_entry(&self, index: usize, path: impl AsRef<Path>) -> Result<()> {
        let entry = self.entry(index)?;
        let mut source = File::open(self.source()?)?;
        source.seek(SeekFrom::Start(entry.offset))?;
        let mut output = create_output(path.as_ref())?;
        let copied = std::io::copy(&mut source.take(u64::from(entry.size)), &mut output)?;
        if copied != u64::from(entry.size) {
            return Err(Error::InvalidArchive("entry was truncated while extracting"));
        }
        output.flush()?;
        Ok(())
    }

    /// Writes every entry into `directory` as `<index>.<ext>`.
    pub fn extract_all(&self, directory: impl AsRef<Path>, typed: bool) -> Result<()> {
        fs::create_dir_all(directory.as_ref())?;
        for (index, entry) in self.entries.iter().enumerate() {
            self.extract_entry(index, directory.as_ref().join(entry_name(index, entry.file_type, typed)))?;
        }
        Ok(())
    }
}

/// The payload bytes of one entry, read into memory.
#[cfg(feature = "std")]
pub fn read_entry(archive_path: impl AsRef<Path>, index: usize) -> Result<Vec<u8>> {
    let mut file = File::open(archive_path.as_ref())?;
    Archive::from_reader(&mut file)?.read_payload(&mut file, index)
}

/// The file extension an entry type is written with under `-e`.
#[cfg(feature = "std")]
pub fn type_extension(file_type: u16) -> &'static str {
    match file_type {
        TYPE_VRAM => "img",
        vab::HEADER_TYPE => "vh",
        vab::BODY_TYPE => "vb",
        sep::TYPE => "seq",
        _ => "bin",
    }
}

/// The file name entry `index` is written under.
#[cfg(feature = "std")]
pub fn entry_name(index: usize, file_type: u16, typed: bool) -> String {
    format!("{index}.{}", if typed { type_extension(file_type) } else { "bin" })
}

/// Creates the parent directories of `path`, if it has any.
#[cfg(feature = "std")]
pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(feature = "std")]
pub(crate) fn create_output(path: &Path) -> Result<File> {
    ensure_parent(path)?;
    Ok(File::create(path)?)
}

/// `value` rounded up to the next payload boundary.
pub fn align_sector(value: u64) -> Result<u64> {
    value
        .checked_add(SECTOR_SIZE - 1)
        .map(|value| value & !(SECTOR_SIZE - 1))
        .ok_or(Error::InvalidArchive("aligned entry range overflows"))
}
