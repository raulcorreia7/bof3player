//! Type-3 VRAM upload codec.
//!
//! A type-3 payload is a sequence of `0x800`-byte chunks; the TOC load
//! argument is the descriptor that places them. Each chunk is one 32x32-word
//! VRAM rectangle, advanced `chunks_per_row` rectangles per row.
//! `decode` turns a payload into those rectangles, `encode` reproduces the payload, and
//! `Plane` is the 1024x512-word frame buffer used to place rectangles and to
//! dump them as a raw 16-bit PNG.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::vram::{self, Descriptor, RECTANGLE_BYTES};
//!
//! let payload = vec![0u8; RECTANGLE_BYTES];
//! let descriptor = Descriptor { column: 0, row: 0, chunks_per_row: 1 };
//! let rectangles = vram::decode(&payload, descriptor).unwrap();
//! assert_eq!(vram::encode(&rectangles), payload);
//! ```

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use super::png;
use crate::error::{Error, Result};

/// Container entry type of a VRAM upload.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::vram::UPLOAD_TYPE, 3);
/// ```
pub const UPLOAD_TYPE: u16 = 3;

/// VRAM words per frame-buffer row.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::vram::VRAM_WIDTH, 1024);
/// ```
pub const VRAM_WIDTH: usize = 1024;
/// Frame-buffer rows.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::vram::VRAM_HEIGHT, 512);
/// ```
pub const VRAM_HEIGHT: usize = 512;
/// Side of one upload rectangle, in VRAM words and rows.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::vram::RECTANGLE_SIDE, 32);
/// ```
pub const RECTANGLE_SIDE: usize = 32;
/// Bytes one upload rectangle occupies in a payload (`32 * 32` words).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::vram::RECTANGLE_BYTES, 2048);
/// ```
pub const RECTANGLE_BYTES: usize = RECTANGLE_SIDE * RECTANGLE_SIDE * 2;
/// Words one upload rectangle holds.
const RECTANGLE_WORDS: usize = RECTANGLE_SIDE * RECTANGLE_SIDE;

/// Packed type-3 load argument: the VRAM destination and row span of an
/// upload. The handler reads the fields as three masked bytes, so the
/// unmasked bits above them carry no meaning.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::Descriptor;
///
/// let descriptor = Descriptor::from_load_argument(0x1a08_0200);
/// assert_eq!((descriptor.column, descriptor.row, descriptor.chunks_per_row), (0x1a, 0x08, 0x02));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Descriptor {
    /// First rectangle's VRAM column, in 32-word units.
    pub column: u8,
    /// First rectangle's VRAM row, in 32-row units.
    pub row: u8,
    /// Rectangles per row before the destination advances one rectangle row.
    pub chunks_per_row: u8,
}

impl Descriptor {
    /// Unpacks a TOC load argument.
    ///
    /// # Examples
    ///
    /// ```
    /// let descriptor = psx_formats::gfx::vram::Descriptor::from_load_argument(0x00_03_01_00);
    /// assert_eq!((descriptor.column, descriptor.row, descriptor.chunks_per_row), (0, 3, 1));
    /// ```
    pub fn from_load_argument(load_argument: u32) -> Self {
        Self {
            column: ((load_argument >> 24) & 0x3f) as u8,
            row: ((load_argument >> 16) & 0x1f) as u8,
            chunks_per_row: ((load_argument >> 8) & 0x3f) as u8,
        }
    }
}

/// One 32x32-word upload rectangle: its VRAM destination and its words.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::Rectangle;
///
/// let rectangle = Rectangle { x: 0, y: 0, words: [0; 1024] };
/// assert_eq!(rectangle.words.len(), 1024);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rectangle {
    /// Destination column in VRAM words.
    pub x: usize,
    /// Destination row in VRAM rows.
    pub y: usize,
    /// Rectangle words, row-major.
    pub words: [u16; RECTANGLE_WORDS],
}

/// Decodes a type-3 payload into its VRAM rectangles, in payload order.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, decode};
///
/// let descriptor = Descriptor { column: 1, row: 2, chunks_per_row: 1 };
/// let rectangles = decode(&vec![0u8; RECTANGLE_BYTES], descriptor).unwrap();
/// assert_eq!((rectangles[0].x, rectangles[0].y), (32, 64));
/// ```
pub fn decode(payload: &[u8], descriptor: Descriptor) -> Result<Vec<Rectangle>> {
    if payload.is_empty() || !payload.len().is_multiple_of(RECTANGLE_BYTES) {
        return Err(Error::VramPayloadLength { bytes: payload.len() });
    }
    let per_row = descriptor.chunks_per_row as usize;
    let count = payload.len() / RECTANGLE_BYTES;
    if per_row == 0 || !count.is_multiple_of(per_row) {
        return Err(Error::VramRowShape { rectangles: count, chunks_per_row: descriptor.chunks_per_row });
    }

    let mut rectangles = Vec::with_capacity(count);
    for (index, chunk) in payload.as_chunks::<RECTANGLE_BYTES>().0.iter().enumerate() {
        let mut words = [0u16; RECTANGLE_WORDS];
        for (word, bytes) in words.iter_mut().zip(chunk.as_chunks::<2>().0.iter()) {
            *word = u16::from_le_bytes([bytes[0], bytes[1]]);
        }
        rectangles.push(Rectangle {
            x: (descriptor.column as usize + index % per_row) * RECTANGLE_SIDE,
            y: (descriptor.row as usize + index / per_row) * RECTANGLE_SIDE,
            words,
        });
    }

    Ok(rectangles)
}

/// An upload laid out as one image: `chunks_per_row` rectangles across, each
/// word read as its RGBA view. This is the 16bpp reading; a 4bpp or 8bpp payload
/// shows as noise because no field names its texture mode or palette.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, upload_image};
///
/// let descriptor = Descriptor { column: 0, row: 0, chunks_per_row: 1 };
/// let image = upload_image(&vec![0u8; RECTANGLE_BYTES], descriptor).unwrap();
/// assert_eq!((image.width, image.height), (32, 32));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// `width * height` RGBA pixels, row-major.
    pub rgba: Vec<u8>,
}

/// An upload's words as one row-major grid, `chunks_per_row` rectangles across.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, word_grid};
///
/// let descriptor = Descriptor { column: 0, row: 0, chunks_per_row: 1 };
/// let grid = word_grid(&vec![0u8; RECTANGLE_BYTES], descriptor).unwrap();
/// assert_eq!(grid.words.len(), grid.width * grid.height);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WordGrid {
    /// Grid width in VRAM words.
    pub width: usize,
    /// Grid height in VRAM rows.
    pub height: usize,
    /// `width * height` words, row-major.
    pub words: Vec<u16>,
}

/// Lays an upload's rectangles out in their own grid, without the VRAM clip.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, word_grid};
///
/// let descriptor = Descriptor { column: 0, row: 0, chunks_per_row: 2 };
/// let grid = word_grid(&vec![0u8; 2 * RECTANGLE_BYTES], descriptor).unwrap();
/// assert_eq!((grid.width, grid.height), (64, 32));
/// ```
pub fn word_grid(payload: &[u8], descriptor: Descriptor) -> Result<WordGrid> {
    let rectangles = decode(payload, descriptor)?;
    let per_row = usize::from(descriptor.chunks_per_row);
    let width = per_row * RECTANGLE_SIDE;
    let height = rectangles.len() / per_row * RECTANGLE_SIDE;
    let mut words = vec![0; width * height];
    for (index, rectangle) in rectangles.iter().enumerate() {
        let left = index % per_row * RECTANGLE_SIDE;
        let top = index / per_row * RECTANGLE_SIDE;
        for (row, line) in rectangle.words.as_chunks::<RECTANGLE_SIDE>().0.iter().enumerate() {
            let start = (top + row) * width + left;
            words[start..start + RECTANGLE_SIDE].copy_from_slice(line);
        }
    }
    Ok(WordGrid { width, height, words })
}

/// Lays an upload's rectangles out in their own grid, each word read as its RGBA
/// view (`clut::word_to_view`).
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, upload_image};
///
/// let image = upload_image(&vec![0u8; RECTANGLE_BYTES], Descriptor { column: 0, row: 0, chunks_per_row: 1 })
///     .unwrap();
/// assert_eq!(image.rgba.len(), 32 * 32 * 4);
/// ```
pub fn upload_image(payload: &[u8], descriptor: Descriptor) -> Result<UploadImage> {
    let grid = word_grid(payload, descriptor)?;
    let rgba = grid.words.iter().flat_map(|&word| super::clut::word_to_view(word)).collect();
    Ok(UploadImage { width: grid.width as u32, height: grid.height as u32, rgba })
}

/// Rebuilds a type-3 payload from a row-major word grid: the inverse of
/// [`word_grid`]. The grid is whole 32-by-32 rectangles, and they are written in
/// payload order.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{RECTANGLE_BYTES, payload_from_grid};
///
/// let payload = payload_from_grid(32, 32, &vec![0u16; 32 * 32]).unwrap();
/// assert_eq!(payload.len(), RECTANGLE_BYTES);
/// ```
pub fn payload_from_grid(width: usize, height: usize, words: &[u16]) -> Result<Vec<u8>> {
    let whole =
        width != 0 && height != 0 && width.is_multiple_of(RECTANGLE_SIDE) && height.is_multiple_of(RECTANGLE_SIDE);
    if !whole || words.len() != width * height {
        return Err(Error::VramGridShape { width, height, words: words.len() });
    }
    let mut payload = Vec::with_capacity(words.len() * 2);
    for top in (0..height).step_by(RECTANGLE_SIDE) {
        for left in (0..width).step_by(RECTANGLE_SIDE) {
            for row in top..top + RECTANGLE_SIDE {
                for &word in &words[row * width + left..][..RECTANGLE_SIDE] {
                    payload.extend_from_slice(&word.to_le_bytes());
                }
            }
        }
    }
    Ok(payload)
}

/// Reproduces a type-3 payload from its rectangles, in payload order.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::{Descriptor, RECTANGLE_BYTES, decode, encode};
///
/// let payload = vec![0u8; RECTANGLE_BYTES];
/// let rectangles = decode(&payload, Descriptor { column: 0, row: 0, chunks_per_row: 1 }).unwrap();
/// assert_eq!(encode(&rectangles), payload);
/// ```
pub fn encode(rectangles: &[Rectangle]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(rectangles.len() * RECTANGLE_BYTES);
    for rectangle in rectangles {
        for word in rectangle.words {
            payload.extend_from_slice(&word.to_le_bytes());
        }
    }

    payload
}

/// A 1024x512-word frame buffer.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::vram::Plane;
///
/// let plane = Plane::new();
/// assert_eq!(plane.word(0, 0), 0);
/// ```
pub struct Plane {
    words: Vec<u16>,
}

impl Plane {
    /// An all-zero frame buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// let plane = psx_formats::gfx::vram::Plane::new();
    /// assert_eq!(plane.word(1023, 511), 0);
    /// ```
    pub fn new() -> Self {
        Self { words: vec![0; VRAM_WIDTH * VRAM_HEIGHT] }
    }

    /// Reads the word at column `x`, row `y`. Panics outside the plane.
    ///
    /// # Examples
    ///
    /// ```
    /// let plane = psx_formats::gfx::vram::Plane::new();
    /// assert_eq!(plane.word(0, 0), 0);
    /// ```
    pub fn word(&self, x: usize, y: usize) -> u16 {
        self.words[y * VRAM_WIDTH + x]
    }

    /// Places rectangles that fit the frame buffer, in order, and returns how
    /// many it cannot place. A loader skips a rectangle whose 32x32
    /// extent leaves the frame buffer instead of clipping it.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::vram::{Plane, Rectangle, VRAM_WIDTH};
    ///
    /// let inside = Rectangle { x: 0, y: 0, words: [0; 1024] };
    /// let outside = Rectangle { x: VRAM_WIDTH, y: 0, words: [0; 1024] };
    /// let mut plane = Plane::new();
    /// assert_eq!(plane.blit(&[inside, outside]), 1);
    /// ```
    pub fn blit(&mut self, rectangles: &[Rectangle]) -> usize {
        let mut skipped = 0;
        for rectangle in rectangles {
            if rectangle.x + RECTANGLE_SIDE > VRAM_WIDTH || rectangle.y + RECTANGLE_SIDE > VRAM_HEIGHT {
                skipped += 1;
                continue;
            }
            for (row, line) in rectangle.words.as_chunks::<RECTANGLE_SIDE>().0.iter().enumerate() {
                let start = (rectangle.y + row) * VRAM_WIDTH + rectangle.x;
                self.words[start..start + RECTANGLE_SIDE].copy_from_slice(line);
            }
        }

        skipped
    }

    /// The plane as 16-bit grayscale PNG bytes, one sample per VRAM word
    /// (`0x0000` black to `0xffff` white).
    ///
    /// # Examples
    ///
    /// ```
    /// let png = psx_formats::gfx::vram::Plane::new().png();
    /// assert_eq!(&png[..8], &psx_formats::gfx::png::SIGNATURE);
    /// ```
    pub fn png(&self) -> Vec<u8> {
        let samples: Vec<u8> = self.words.iter().flat_map(|word| word.to_be_bytes()).collect();
        png::gray16_png(VRAM_WIDTH as u32, VRAM_HEIGHT as u32, &samples)
    }

    /// Writes the plane as a 16-bit grayscale PNG, one sample per VRAM word
    /// (`0x0000` black to `0xffff` white).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// psx_formats::gfx::vram::Plane::new().write_png(Path::new("vram.png")).unwrap();
    /// ```
    pub fn write_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let png = self.png();
        let mut output = BufWriter::new(File::create(path)?);
        output.write_all(&png)?;
        output.flush()
    }
}

impl Default for Plane {
    fn default() -> Self {
        Self::new()
    }
}
