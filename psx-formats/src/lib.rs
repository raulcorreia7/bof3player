#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod adpcm;
pub mod bytes;
mod error;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
#[cfg(feature = "std")]
pub mod gfx;
pub mod sector;
pub mod sep;
#[cfg(feature = "std")]
pub mod sf2;
pub mod vab;
pub mod wav;
pub mod xa;

pub use error::{Error, Result};
#[cfg(feature = "std")]
pub use gfx::{clut, vram};

/// Sample rate of VAB samples and sequenced music, in Hz.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::VAB_SAMPLE_RATE, 44_100);
/// ```
pub const VAB_SAMPLE_RATE: u32 = 44_100;
