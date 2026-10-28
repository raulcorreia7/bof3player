//! Graphics: VRAM uploads, CLUT banks, texture-page words, the standard TIM
//! file and the PNG output they share.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::clut;
//!
//! assert_eq!(clut::word_to_rgba(0x001f), [255, 0, 0, 255]);
//! ```

pub mod clut;
pub mod png;
pub mod texture;
pub mod tim;
pub mod vram;
