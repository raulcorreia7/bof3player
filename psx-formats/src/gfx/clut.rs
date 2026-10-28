//! PSX CLUT (palette) codec.
//!
//! A palette payload is raw little-endian PSX color words
//! (`STP | B5 | G5 | R5`; bit 15 is STP). [`Palette::decode`] turns payload
//! bytes into words and [`Palette::encode`] turns words back into the same
//! bytes. The bank rule is [`validate_words`]: a whole number of 16-color
//! (4bpp) CLUT rows; a bank whose word count also divides by 256 is a whole
//! number of 8bpp rows. Texture mode and the texture-to-palette association are
//! draw-state data, so the codec never asks for them.
//!
//! The RGBA view ([`word_to_rgba`], [`Palette::json`], [`Palette::png_strip`])
//! is for inspection: STP is not alpha, so it survives in the words and in the
//! JSON `stp` flag, not in the RGBA tuple.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::clut::Palette;
//!
//! let bytes: Vec<u8> = (0..32).collect();
//! let palette = Palette::decode(&bytes).unwrap();
//! assert_eq!(palette.rows_4bpp(), 1);
//! assert_eq!(palette.encode(), bytes);
//! ```

use serde::Serialize;

use crate::error::{Error, Result};

/// Colors in one 4bpp CLUT row: 16 words, `0x20` bytes.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::clut::ROW_WORDS_4BPP, 16);
/// ```
pub const ROW_WORDS_4BPP: usize = 16;
/// Colors in one 8bpp CLUT row: 256 words, `0x200` bytes.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::clut::ROW_WORDS_8BPP, 256);
/// ```
pub const ROW_WORDS_8BPP: usize = 256;
/// PSX color words that set STP (semi-transparency) in bit 15.
const STP: u16 = 0x8000;
/// Swatch edge of the PNG strip, in pixels.
const SWATCH_PIXELS: u32 = 16;
/// Rejects a color-word count that cannot be a CLUT bank.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::clut::validate_words;
///
/// assert!(validate_words(16).is_ok());
/// assert!(validate_words(15).is_err());
/// ```
pub fn validate_words(words: usize) -> Result<(), Error> {
    if words < ROW_WORDS_4BPP || !words.is_multiple_of(ROW_WORDS_4BPP) {
        return Err(Error::PaletteNotRowAligned(words));
    }
    Ok(())
}

/// Expands one 5-bit channel to 8 bits by replicating its top 3 bits.
fn expand_channel(channel: u16) -> u8 {
    ((channel << 3) | (channel >> 2)) as u8
}

/// Expands a PSX color word to an opaque RGBA color: alpha is always 255. The
/// lossless view is [`word_to_view`].
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::clut::word_to_rgba(0x001f), [255, 0, 0, 255]);
/// ```
pub fn word_to_rgba(word: u16) -> [u8; 4] {
    [expand_channel(word & 0x1f), expand_channel((word >> 5) & 0x1f), expand_channel((word >> 10) & 0x1f), 0xff]
}

/// The view alpha of `0x0000`, which the GPU never draws.
const VIEW_TRANSPARENT: u8 = 0;
/// The view alpha of a word that sets STP (semi-transparency).
const VIEW_STP: u8 = 0x80;
/// The view alpha of every other word.
const VIEW_OPAQUE: u8 = 0xff;

/// Expands a PSX color word to RGBA, with alpha carrying what the channels
/// cannot: `0x0000` is transparent, a word with STP set is half transparent, and
/// every other word is opaque. [`view_to_word`] inverts it exactly for all
/// 65,536 words, so a view PNG holds the whole word.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::clut::{view_to_word, word_to_view};
///
/// assert_eq!(word_to_view(0x8000)[3], 128);
/// assert_eq!(view_to_word(word_to_view(0x7c00)), 0x7c00);
/// ```
pub fn word_to_view(word: u16) -> [u8; 4] {
    let alpha = match word {
        0 => VIEW_TRANSPARENT,
        _ if word & STP != 0 => VIEW_STP,
        _ => VIEW_OPAQUE,
    };
    let [red, green, blue, _] = word_to_rgba(word);
    [red, green, blue, alpha]
}

/// The color word of a view pixel, the inverse of [`word_to_view`]. Alpha below
/// 64 is `0x0000` and its channels do not count; alpha from 64 to 191 sets STP;
/// anything higher is opaque. An edited pixel falls into the same bands.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::clut::view_to_word([255, 0, 0, 255]), 0x001f);
/// assert_eq!(psx_formats::gfx::clut::view_to_word([0, 0, 0, 0]), 0);
/// ```
pub fn view_to_word(rgba: [u8; 4]) -> u16 {
    if rgba[3] < 64 {
        return 0;
    }
    let stp = if rgba[3] < 192 { STP } else { 0 };
    rgba_to_word(rgba) | stp
}

/// Packs RGBA into a PSX color word with the top 5 bits of each channel. The
/// low 3 bits and alpha are not representable and are dropped.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::clut::rgba_to_word([255, 0, 0, 255]), 0x001f);
/// ```
pub fn rgba_to_word(rgba: [u8; 4]) -> u16 {
    let channel = |value: u8| u16::from(value >> 3);
    channel(rgba[0]) | (channel(rgba[1]) << 5) | (channel(rgba[2]) << 10)
}

/// A CLUT bank: raw little-endian PSX color words.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::clut::Palette;
///
/// let palette = Palette::from_words(vec![0; 16]).unwrap();
/// assert_eq!(palette.rows_4bpp(), 1);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Palette {
    words: Vec<u16>,
}

#[derive(Serialize)]
struct PaletteJson {
    format: &'static str,
    words: usize,
    rows_4bpp: usize,
    rows_8bpp: Option<usize>,
    colors: Vec<ColorJson>,
}

#[derive(Serialize)]
struct ColorJson {
    index: usize,
    word: u16,
    stp: bool,
    rgba: [u8; 4],
}

impl Palette {
    /// Decodes payload bytes, rejecting shapes that are not a CLUT bank.
    ///
    /// # Examples
    ///
    /// ```
    /// let bytes: Vec<u8> = (0..32).collect();
    /// assert!(psx_formats::gfx::clut::Palette::decode(&bytes).is_ok());
    /// assert!(psx_formats::gfx::clut::Palette::decode(&[0; 31]).is_err());
    /// ```
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if !bytes.len().is_multiple_of(2) {
            return Err(Error::PaletteOddByteCount(bytes.len()));
        }
        let words = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        Self::from_words(words)
    }

    /// Validates color words with [`validate_words`] and keeps them.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0x8000; 32]).unwrap();
    /// assert_eq!(palette.words().len(), 32);
    /// ```
    pub fn from_words(words: Vec<u16>) -> Result<Self, Error> {
        validate_words(words.len())?;
        Ok(Self { words })
    }

    /// Encodes the bank back to payload bytes; the inverse of [`Self::decode`].
    ///
    /// # Examples
    ///
    /// ```
    /// let bytes: Vec<u8> = (0..32).collect();
    /// let palette = psx_formats::gfx::clut::Palette::decode(&bytes).unwrap();
    /// assert_eq!(palette.encode(), bytes);
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.words.len() * 2);
        for word in &self.words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    /// The color words in payload order.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0x001f; 16]).unwrap();
    /// assert_eq!(palette.words()[0], 0x001f);
    /// ```
    pub fn words(&self) -> &[u16] {
        &self.words
    }

    /// Number of 4bpp CLUT rows; exact, because every constructor validates.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0; 32]).unwrap();
    /// assert_eq!(palette.rows_4bpp(), 2);
    /// ```
    pub fn rows_4bpp(&self) -> usize {
        self.words.len() / ROW_WORDS_4BPP
    }

    /// Number of 8bpp CLUT rows, or `None` when the bank is not 256-word aligned.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0; 256]).unwrap();
    /// assert_eq!(palette.rows_8bpp(), Some(1));
    /// ```
    pub fn rows_8bpp(&self) -> Option<usize> {
        self.words.len().is_multiple_of(ROW_WORDS_8BPP).then_some(self.words.len() / ROW_WORDS_8BPP)
    }

    /// The bank as JSON: geometry plus one record per color word.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0; 16]).unwrap();
    /// assert!(palette.json().contains("\"format\": \"psx-clut\""));
    /// ```
    pub fn json(&self) -> String {
        let document = PaletteJson {
            format: "psx-clut",
            words: self.words.len(),
            rows_4bpp: self.rows_4bpp(),
            rows_8bpp: self.rows_8bpp(),
            colors: self
                .words
                .iter()
                .enumerate()
                .map(|(index, &word)| ColorJson { index, word, stp: word & STP != 0, rgba: word_to_rgba(word) })
                .collect(),
        };
        serde_json::to_string_pretty(&document).expect("palette model is serializable")
    }

    /// Renders the bank as an 8-bit RGBA PNG swatch strip: 16 colors per row,
    /// one 4bpp CLUT row per image row, `SWATCH_PIXELS` pixels per swatch.
    ///
    /// # Examples
    ///
    /// ```
    /// let palette = psx_formats::gfx::clut::Palette::from_words(vec![0; 16]).unwrap();
    /// assert_eq!(&palette.png_strip()[..8], &psx_formats::gfx::png::SIGNATURE);
    /// ```
    pub fn png_strip(&self) -> Vec<u8> {
        let columns = ROW_WORDS_4BPP as u32;
        let width = columns * SWATCH_PIXELS;
        let height = self.rows_4bpp() as u32 * SWATCH_PIXELS;
        let mut pixels = Vec::with_capacity((width * 4 * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let index = (y / SWATCH_PIXELS * columns + x / SWATCH_PIXELS) as usize;
                pixels.extend_from_slice(&word_to_rgba(self.words[index]));
            }
        }
        super::png::rgba_png(width, height, &pixels)
    }
}
