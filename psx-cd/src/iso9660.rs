//! ISO9660 directory records with the CD-XA system-use field.

use std::path::Component;
use std::path::Path;

use crate::error::{Error, Result};

/// Smallest directory record: the fixed fields up to the identifier length.
const RECORD_MIN_LENGTH: usize = 34;
/// Offsets of the fixed record fields (ECMA-119 9.1).
const EXT_ATTR_LENGTH: usize = 1;
const EXTENT_LE: usize = 2;
const EXTENT_BE: usize = 6;
const SIZE_LE: usize = 10;
const SIZE_BE: usize = 14;
const FLAGS: usize = 25;
const NAME_LENGTH: usize = 32;
const NAME: usize = 33;
/// Directory flag in the record's flags byte.
pub(crate) const FLAG_DIRECTORY: u8 = 0x02;
/// Bytes of the CD-XA attribute block in a record's system-use area.
const XA_BLOCK_LENGTH: usize = 14;
/// The `XA` signature inside the attribute block.
const XA_SIGNATURE: &[u8; 2] = b"XA";
const XA_SIGNATURE_OFFSET: usize = 6;
const XA_FILE_NUMBER_OFFSET: usize = 8;
/// CD-XA attribute bits, in the high byte of the attribute word: Form 1 data
/// and Form 2 data. A Form 2 file is read as raw 2336-byte sectors.
const XA_FORM1: u8 = 0x08;
const XA_FORM2: u8 = 0x10;

/// CD-XA attributes of a directory record.
///
/// The 14-byte system-use block follows the file identifier; its first bytes
/// are the group and user IDs and its sixth and seventh bytes are the `XA`
/// signature (`psx-spx` "CDROM XA Subheader, File, Channel, Interleave").
///
/// # Examples
///
/// ```
/// let attributes = psx_cd::XaAttributes {
///     group_id: 0x1234,
///     user_id: 0x5678,
///     attributes: 0x1055,
///     file_number: 1,
/// };
/// assert_eq!(attributes.file_number, 1);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XaAttributes {
    /// Owner group ID, stored big-endian in the record.
    pub group_id: u16,
    /// Owner user ID, stored big-endian in the record.
    pub user_id: u16,
    /// Attribute word; the high byte holds the Form 1 (`0x08`) and Form 2
    /// (`0x10`) data bits.
    pub attributes: u16,
    /// File number; a nonzero value marks an interleaved XA stream.
    pub file_number: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectoryRecord {
    pub extent: u32,
    pub size: u32,
    pub flags: u8,
    pub name: Vec<u8>,
    pub extended_attributes: u8,
    pub is_xa: bool,
    pub xa: Option<XaAttributes>,
}

impl DirectoryRecord {
    pub fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }
}

/// Parses one record; `None` for an empty record (length byte zero).
pub(crate) fn parse_record(bytes: &[u8]) -> Result<Option<DirectoryRecord>> {
    let Some(&length) = bytes.first() else {
        return Ok(None);
    };
    if length == 0 {
        return Ok(None);
    }
    let length = length as usize;
    if length > bytes.len() || length < RECORD_MIN_LENGTH {
        return Err(Error::InvalidImage("invalid ISO9660 directory record length"));
    }
    let name_len = bytes[NAME_LENGTH] as usize;
    let name_end = NAME.checked_add(name_len).ok_or(Error::InvalidImage("directory name range overflows"))?;
    if name_end > length {
        return Err(Error::InvalidImage("directory name is truncated"));
    }
    let extent = u32::from_le_bytes(bytes[EXTENT_LE..EXTENT_LE + 4].try_into().unwrap());
    let extent_be = u32::from_be_bytes(bytes[EXTENT_BE..EXTENT_BE + 4].try_into().unwrap());
    let size = u32::from_le_bytes(bytes[SIZE_LE..SIZE_LE + 4].try_into().unwrap());
    let size_be = u32::from_be_bytes(bytes[SIZE_BE..SIZE_BE + 4].try_into().unwrap());
    if extent != extent_be || size != size_be {
        return Err(Error::InvalidImage("ISO9660 both-endian fields disagree"));
    }
    // The identifier is padded to an even length before the system-use area.
    let system_use = NAME + name_len + usize::from(name_len.is_multiple_of(2));
    let xa = parse_xa_attributes(&bytes[system_use..length]);
    let is_xa = xa.is_some_and(|attributes| {
        let kind = (attributes.attributes >> 8) as u8;
        attributes.file_number != 0 || (kind & XA_FORM2 != 0 && kind & XA_FORM1 == 0)
    });
    Ok(Some(DirectoryRecord {
        extent,
        size,
        flags: bytes[FLAGS],
        name: bytes[NAME..name_end].to_vec(),
        extended_attributes: bytes[EXT_ATTR_LENGTH],
        is_xa,
        xa,
    }))
}

fn parse_xa_attributes(bytes: &[u8]) -> Option<XaAttributes> {
    if bytes.len() < XA_BLOCK_LENGTH || &bytes[XA_SIGNATURE_OFFSET..XA_SIGNATURE_OFFSET + 2] != XA_SIGNATURE {
        return None;
    }
    Some(XaAttributes {
        group_id: u16::from_be_bytes(bytes[0..2].try_into().ok()?),
        user_id: u16::from_be_bytes(bytes[2..4].try_into().ok()?),
        attributes: u16::from_be_bytes(bytes[4..6].try_into().ok()?),
        file_number: bytes[XA_FILE_NUMBER_OFFSET],
    })
}

/// A record identifier as a file name: no version suffix, no trailing dot.
pub(crate) fn normalized_name(raw: &[u8]) -> Result<String> {
    let decoded = std::str::from_utf8(raw).map_err(|_| Error::InvalidImage("non-UTF-8 ISO9660 identifier"))?;
    let name = decoded.split(';').next().unwrap_or(decoded).trim_end_matches('.');
    if name.is_empty()
        || name == "."
        || name == ".."
        || Path::new(name).components().any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::InvalidImage("unsafe ISO9660 identifier"));
    }
    Ok(name.to_string())
}
