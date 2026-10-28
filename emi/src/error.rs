//! The crate's one error type.

use core::fmt;

use crate::cue;

/// Everything an archive, entry or cue operation in this crate can report.
#[derive(Debug)]
pub enum Error {
    /// A file could not be read or written.
    #[cfg(feature = "std")]
    Io(std::io::Error),
    /// A PlayStation format inside an entry is malformed.
    Format(psx_formats::Error),
    /// The container table is malformed.
    InvalidArchive(&'static str),
    /// The entry index is past the table's last entry.
    EntryOutOfRange {
        /// The requested entry index.
        index: usize,
        /// The number of entries in the table.
        count: usize,
    },
    /// A cue table's size is not a whole number of cue records.
    CueTableSize(usize),
    /// A cue table holds more records than one bank chunk.
    CueTableRecords(usize),
    /// A cue record sets bits the format reserves.
    CueRecord {
        /// The index of the offending record.
        record: usize,
        /// What the record gets wrong.
        reason: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "std")]
            Self::Io(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
            Self::InvalidArchive(message) => write!(f, "invalid EMI archive: {message}"),
            Self::EntryOutOfRange { index, count } => {
                write!(f, "entry index {index} is out of range (archive has {count} entries)")
            }
            Self::CueTableSize(bytes) => {
                write!(f, "cue table is {bytes} bytes; a cue record is {} bytes", cue::RECORD_BYTES)
            }
            Self::CueTableRecords(records) => {
                write!(f, "cue table holds {records} records; one bank chunk holds {}", cue::CHUNK_RECORDS)
            }
            Self::CueRecord { record, reason } => write!(f, "cue record {record}: {reason}"),
        }
    }
}

#[cfg(feature = "std")]
impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Format(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(not(feature = "std"))]
impl core::error::Error for Error {}

#[cfg(feature = "std")]
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<psx_formats::Error> for Error {
    fn from(error: psx_formats::Error) -> Self {
        Self::Format(error)
    }
}

/// The crate's result type.
pub type Result<T, E = Error> = core::result::Result<T, E>;
