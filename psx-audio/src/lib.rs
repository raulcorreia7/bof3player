// SPDX-License-Identifier: MIT
#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod error;
pub mod export;
pub mod player;
pub mod source;
pub mod spu;

pub use error::{Error, Result};
pub use player::{Engine, Loop, Player, XaStream};
pub use source::{Blob, CallbackSource, MemorySource, Source};
#[cfg(feature = "std")]
pub use source::{DiscSource, FolderSource, ReaderBlob};
