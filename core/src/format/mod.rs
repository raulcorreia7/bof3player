// SPDX-License-Identifier: MIT
//! Parsers for the on-disc formats. The PlayStation ones live in `psx-formats`; this module
//! exposes the two an embedder needs (an EMI archive and its VAB banks) and keeps the rest
//! crate-private.

pub mod emi;

pub use psx_formats::vab;

pub(crate) use psx_formats::{VAB_SAMPLE_RATE, sep, wav, xa};
