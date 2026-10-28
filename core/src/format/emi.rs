// SPDX-License-Identifier: MIT
//! EMI archives as the audio library reads them: the container table of `emi` bound to the
//! bytes it was parsed from.

use alloc::format;

use emi::Archive;
use psx_formats::{sep, vab};

use crate::error::{Error, Result};

/// The entry types the library reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    /// VAB header (`.VH`).
    Vh,
    /// VAB body (`.VB`).
    Vb,
    /// Auxiliary audio: the bank's cue list.
    Aux,
    /// SEP sequence file.
    Seq,
}

impl EntryType {
    /// The type code in the archive table.
    #[must_use]
    pub fn code(self) -> u16 {
        match self {
            Self::Vh => vab::HEADER_TYPE,
            Self::Vb => vab::BODY_TYPE,
            Self::Aux => emi::TYPE_CUE,
            Self::Seq => sep::TYPE,
        }
    }
}

/// Convert the container crate's error into the audio crate's error.
pub(crate) fn error(inner: emi::Error) -> Error {
    match inner {
        emi::Error::Format(source) => source.into(),
        other => Error::format(alloc::format!("{other}")),
    }
}

/// A parsed archive that borrows its bytes.
#[derive(Debug)]
pub struct Emi<'a> {
    bytes: &'a [u8],
    archive: Archive,
}

impl<'a> Emi<'a> {
    /// Parse the table of contents; a truncated archive is rejected whole.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        Ok(Self { bytes, archive: Archive::parse(bytes).map_err(error)? })
    }

    /// Payloads of every entry of `kind`, in table order.
    pub fn payloads(&self, kind: EntryType) -> impl Iterator<Item = &'a [u8]> + '_ {
        let bytes = self.bytes;
        self.archive
            .entries()
            .iter()
            .enumerate()
            .filter(move |(_, entry)| entry.file_type == kind.code())
            .filter_map(move |(index, _)| self.archive.payload(bytes, index).ok())
    }

    /// The `occurrence`-th (0-based) payload of `kind`.
    pub fn find(&self, kind: EntryType, occurrence: usize) -> Result<&'a [u8]> {
        self.payloads(kind)
            .nth(occurrence)
            .ok_or_else(|| Error::format(format!("EMI archive holds no entry of type {} #{occurrence}", kind.code())))
    }
}
