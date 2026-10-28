//! The standard PS1 TIM file: a CLUT and an image, ready for the GPU.
//!
//! A TIM file is a header of two words and up to two blocks of frame buffer
//! data. The header's ID word is `0x10` with a version of zero, and the flag
//! word names the pixel mode (4-bit, 8-bit or 15-bit direct colour) and
//! whether a CLUT block follows. Each block is a length, the frame buffer
//! position, the size and the words themselves; the words are the frame
//! buffer's own 16-bit units, so a texture's VRAM words are a TIM's image data
//! unchanged.
//!
//! The layout is the one in Sony's *File Formats* manual, chapter 3 ("TIM:
//! Screen Image Data"): every item is a 32-bit word in little endian, the
//! 4-bit mode holds four pixels per word with the leftmost pixel in the low
//! nibble, and the 8-bit mode holds two.
//!
//! A TIM is what a raw VRAM upload and a raw CLUT bank make when they are
//! read together; [`write`] builds that file.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::tim::{Block, File, Mode};
//!
//! let tim = File {
//!     mode: Mode::Fifteen,
//!     clut: None,
//!     image: Block { x: 0, y: 0, width: 1, height: 1, words: vec![0x001f] },
//! };
//! assert_eq!(File::read(&tim.write()).unwrap(), tim);
//! ```

use crate::error::{Error, Result};

/// The ID word of a TIM file: `0x10` with a version of zero.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::tim::ID, 0x10);
/// ```
pub const ID: u32 = 0x10;

/// The pixel mode of a TIM file.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::tim::Mode;
///
/// assert_eq!(Mode::of_value(1), Some(Mode::Eight));
/// assert_eq!(Mode::Four.pixels_per_word(), 4);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// 4-bit CLUT: four pixels per word, sixteen colours.
    Four,
    /// 8-bit CLUT: two pixels per word, 256 colours.
    Eight,
    /// 15-bit direct colour: one pixel per word.
    Fifteen,
}

impl Mode {
    /// The mode's value in the flag word.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::tim::Mode::Fifteen.value(), 2);
    /// ```
    pub fn value(self) -> u32 {
        match self {
            Mode::Four => 0,
            Mode::Eight => 1,
            Mode::Fifteen => 2,
        }
    }

    /// The mode the flag word names.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::tim::Mode;
    ///
    /// assert_eq!(Mode::of_value(0), Some(Mode::Four));
    /// assert_eq!(Mode::of_value(3), None);
    /// ```
    pub fn of_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(Mode::Four),
            1 => Some(Mode::Eight),
            2 => Some(Mode::Fifteen),
            _ => None,
        }
    }

    /// The pixels one word holds.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::tim::Mode::Eight.pixels_per_word(), 2);
    /// ```
    pub fn pixels_per_word(self) -> usize {
        match self {
            Mode::Four => 4,
            Mode::Eight => 2,
            Mode::Fifteen => 1,
        }
    }
}

/// A block of frame buffer words: a CLUT or an image.
///
/// # Examples
///
/// ```
/// let block = psx_formats::gfx::tim::Block { x: 0, y: 0, width: 2, height: 1, words: vec![1, 2] };
/// assert_eq!(block.words.len(), 2);
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Block {
    /// The frame buffer x coordinate, in pixels.
    pub x: u16,
    /// The frame buffer y coordinate, in rows.
    pub y: u16,
    /// The width in 16-bit units.
    pub width: u16,
    /// The height in rows.
    pub height: u16,
    /// The words, row-major.
    pub words: Vec<u16>,
}

impl Block {
    /// The block's bytes: the length, the position, the size and the words.
    fn write(&self, out: &mut Vec<u8>) {
        let length = (12 + self.words.len() * 2) as u32;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        for word in &self.words {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }

    /// The block at `at` in `bytes`, and the offset after it.
    fn read(bytes: &[u8], at: usize) -> Result<(Self, usize)> {
        let header = bytes.get(at..at + 12).ok_or_else(|| Error::Tim("the block runs past the file".into()))?;
        let length = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
        if length < 12 || !length.is_multiple_of(2) {
            return Err(Error::Tim(format!("a block of {length} bytes is not a whole number of words")));
        }
        let data = bytes.get(at + 12..at + length).ok_or_else(|| Error::Tim("the block runs past the file".into()))?;
        let words = data.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        Ok((
            Self {
                x: u16::from_le_bytes(header[4..6].try_into().unwrap()),
                y: u16::from_le_bytes(header[6..8].try_into().unwrap()),
                width: u16::from_le_bytes(header[8..10].try_into().unwrap()),
                height: u16::from_le_bytes(header[10..12].try_into().unwrap()),
                words,
            },
            at + length,
        ))
    }
}

/// A TIM file: a CLUT and an image.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::tim::{Block, File, Mode};
///
/// let tim = File {
///     mode: Mode::Four,
///     clut: Some(Block { x: 0, y: 0, width: 1, height: 1, words: vec![0x8000] }),
///     image: Block { x: 0, y: 0, width: 1, height: 1, words: vec![0] },
/// };
/// assert_eq!(tim.write()[4], 0x10, "4-bit mode with a CLUT");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File {
    /// The pixel mode.
    pub mode: Mode,
    /// The CLUT block, when the file has one.
    pub clut: Option<Block>,
    /// The image block.
    pub image: Block,
}

impl File {
    /// The TIM file's bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::tim::{Block, File, Mode};
    ///
    /// let tim = File { mode: Mode::Fifteen, clut: None, image: Block::default() };
    /// assert_eq!(&tim.write()[..4], &[0x10, 0x00, 0x00, 0x00]);
    /// ```
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&ID.to_le_bytes());
        let flags = self.mode.value() | u32::from(self.clut.is_some()) << 4;
        out.extend_from_slice(&flags.to_le_bytes());
        if let Some(clut) = &self.clut {
            clut.write(&mut out);
        }
        self.image.write(&mut out);
        out
    }

    /// Reads a TIM file: the magic, the flags, the block sizes and the words.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::tim::{Block, File, Mode};
    ///
    /// let tim = File { mode: Mode::Fifteen, clut: None, image: Block::default() };
    /// assert_eq!(File::read(&tim.write()).unwrap().mode, Mode::Fifteen);
    /// assert!(File::read(b"not a TIM").is_err());
    /// ```
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let header = bytes.get(..8).ok_or_else(|| Error::Tim("the file is shorter than the header".into()))?;
        let id = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if id != ID {
            return Err(Error::Tim(format!("not a TIM file (the ID word is {id:#010x})")));
        }
        let flags = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let mode = Mode::of_value(flags & 0x0f)
            .ok_or_else(|| Error::Tim(format!("a pixel mode of {} is not one this codec reads", flags & 0x0f)))?;
        let mut at = 8;
        let clut = if flags & 0x10 != 0 {
            let (block, next) = Block::read(bytes, at)?;
            at = next;
            Some(block)
        } else {
            None
        };
        let (image, _) = Block::read(bytes, at)?;
        Ok(Self { mode, clut, image })
    }
}
