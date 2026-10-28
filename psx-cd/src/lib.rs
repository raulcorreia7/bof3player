#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod checksum;
pub mod cue;
mod error;
mod image;
mod iso9660;
pub mod metadata;
pub mod patch;
pub mod rebuild;

pub use error::{Error, Result};
pub(crate) use image::ensure_parent;
pub use image::{Entry, ISO_SECTOR_SIZE, Image, RAW_SECTOR_SIZE, ReadSeek, XA_SECTOR_SIZE, discover_image};
pub use iso9660::XaAttributes;
