//! The EMI archive container: header, table of contents, sector-aligned
//! payloads and the type-8 cue records a VAB bank's sound effects use.
//!
//! [`Archive::parse`] reads the table from bytes and needs no `std`; the file
//! operations ([`Archive::open`], extraction, [`read_entry`]) are behind the
//! `std` feature. The PlayStation formats inside the entries (VAB, SEP, VRAM,
//! CLUT) live in `psx-formats`.

#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cue;
pub mod emi;
mod error;

pub use emi::{Archive, Entry, HEADER_SIZE, MAGIC, SECTOR_SIZE, TOC_ENTRY_SIZE, TYPE_CUE, TYPE_LZ77, align_sector};
#[cfg(feature = "std")]
pub use emi::{TYPE_VRAM, ensure_parent, entry_name, read_entry, type_extension};
pub use error::{Error, Result};
