//! PNG output and input for graphics, over the `png` crate.
//!
//! The writers take unfiltered, tightly packed scanlines and return the file
//! bytes. The bytes depend on the `png` crate's deflate encoder, so a pixel
//! comparison, not a byte comparison, is the stable check across crate
//! versions. [`read`] takes back the pictures the writers produce, and any
//! 8-bit RGB or RGBA PNG and any 4- or 8-bit indexed PNG.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::png::{self, Picture};
//!
//! let bytes = png::rgba_png(1, 1, &[255, 0, 0, 255]);
//! let Picture::Rgba(rgba) = png::read(&bytes).unwrap() else { panic!("an RGBA PNG") };
//! assert_eq!(rgba.pixels, [255, 0, 0, 255]);
//! ```

use std::io::Cursor;

use png::{BitDepth, ColorType, Encoder};

use crate::error::Error;

/// The eight bytes every PNG starts with.
///
/// # Examples
///
/// ```
/// let png = psx_formats::gfx::png::rgba_png(1, 1, &[0, 0, 0, 255]);
/// assert_eq!(&png[..8], &psx_formats::gfx::png::SIGNATURE);
/// ```
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// An 8-bit RGBA picture: four bytes per pixel, row-major.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::png::{self, Picture};
///
/// let bytes = png::rgba_png(1, 1, &[255, 0, 0, 255]);
/// let Picture::Rgba(rgba) = png::read(&bytes).unwrap() else { panic!("an RGBA PNG") };
/// assert_eq!(rgba.pixels, [255, 0, 0, 255]);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rgba {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

/// A palette-indexed picture: one index per pixel, and the palette's colors.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::png::{self, Picture};
///
/// let colors = [[255, 0, 0, 255], [0, 255, 0, 255]];
/// let bytes = png::indexed_png(2, 1, 4, &colors, &[0, 1]);
/// let Picture::Indexed(indexed) = png::read(&bytes).unwrap() else { panic!("an indexed PNG") };
/// assert_eq!(indexed.indices, [0, 1]);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Indexed {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per index in the file: 4 or 8.
    pub bits: u8,
    /// The palette in file order. Alpha is 255 where the file gives none.
    pub colors: Vec<[u8; 4]>,
    /// One unpacked index per pixel, row-major.
    pub indices: Vec<u8>,
}

/// A PNG read back: an RGB or RGBA picture, or an indexed one.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::png::{self, Picture};
///
/// let bytes = png::rgba_png(1, 1, &[0, 0, 0, 255]);
/// match png::read(&bytes).unwrap() {
///     Picture::Rgba(rgba) => assert_eq!(rgba.width, 1),
///     Picture::Indexed(_) => panic!("an RGBA PNG"),
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Picture {
    /// An 8-bit RGBA picture. RGB files read as opaque RGBA.
    Rgba(Rgba),
    /// A 4- or 8-bit indexed picture.
    Indexed(Indexed),
}

/// Encodes `data` as one `color`/`depth` image.
///
/// The writer is a `Vec`, and `data` is sized from `width`, `height` and the
/// pixel format by every caller in this crate, so encoding cannot fail.
fn encode(
    width: u32,
    height: u32,
    color: ColorType,
    depth: BitDepth,
    palette: Option<(Vec<u8>, Vec<u8>)>,
    data: &[u8],
) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    if let Some((colors, alpha)) = palette {
        encoder.set_palette(colors);
        encoder.set_trns(alpha);
    }
    let mut writer = encoder.write_header().expect("a Vec never fails to take a header");
    writer.write_image_data(data).expect("image data matches the declared geometry");
    writer.finish().expect("a Vec never fails to finish");
    out.into_inner()
}

/// An 8-bit RGBA PNG over `width * height` pixels of four bytes each.
///
/// # Examples
///
/// ```
/// let png = psx_formats::gfx::png::rgba_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
/// assert_eq!(&png[..8], &psx_formats::gfx::png::SIGNATURE);
/// ```
pub fn rgba_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    encode(width, height, ColorType::Rgba, BitDepth::Eight, None, pixels)
}

/// A 16-bit grayscale PNG over `width * height` big-endian samples.
///
/// # Examples
///
/// ```
/// let png = psx_formats::gfx::png::gray16_png(1, 2, &[0, 0, 255, 255]);
/// assert_eq!(&png[..8], &psx_formats::gfx::png::SIGNATURE);
/// ```
pub fn gray16_png(width: u32, height: u32, samples: &[u8]) -> Vec<u8> {
    encode(width, height, ColorType::Grayscale, BitDepth::Sixteen, None, samples)
}

/// An indexed PNG over `width * height` pixels: one palette index per pixel in
/// `indices`, packed to `bits` per pixel (4 or 8), and up to `2^bits` RGBA
/// colors. Alpha goes in the transparency table.
///
/// # Panics
///
/// Panics if `bits` is not 4 or 8, or if `indices` is not `width * height` long.
/// Callers size both.
///
/// # Examples
///
/// ```
/// let colors = [[255, 0, 0, 255], [0, 255, 0, 255]];
/// let png = psx_formats::gfx::png::indexed_png(2, 1, 4, &colors, &[1, 0]);
/// assert_eq!(&png[..8], &psx_formats::gfx::png::SIGNATURE);
/// ```
pub fn indexed_png(width: u32, height: u32, bits: u8, colors: &[[u8; 4]], indices: &[u8]) -> Vec<u8> {
    let depth = match bits {
        4 => BitDepth::Four,
        8 => BitDepth::Eight,
        _ => panic!("an indexed PNG is 4- or 8-bit, not {bits}-bit"),
    };
    assert_eq!(indices.len(), width as usize * height as usize, "one index per pixel");
    let rgb = colors.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
    let alpha = colors.iter().map(|c| c[3]).collect();
    let packed = pack_rows(width as usize, bits, indices);
    encode(width, height, ColorType::Indexed, depth, Some((rgb, alpha)), &packed)
}

/// Reads a PNG as a [`Picture`]: 8-bit RGB or RGBA, or 4- or 8-bit indexed.
/// Any other PNG is an error.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::png::{self, Picture};
///
/// let bytes = png::rgba_png(1, 1, &[0, 0, 0, 255]);
/// assert!(matches!(png::read(&bytes), Ok(Picture::Rgba(_))));
/// assert!(png::read(b"not a png").is_err());
/// ```
pub fn read(bytes: &[u8]) -> Result<Picture, Error> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(decode_error)?;
    let size = reader.output_buffer_size().ok_or_else(|| Error::PngPicture("the image is too large to read".into()))?;
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(decode_error)?;
    let (width, height, line) = (frame.width, frame.height, frame.line_size);
    let info = reader.info();
    match (frame.color_type, frame.bit_depth) {
        (ColorType::Rgba, BitDepth::Eight) => Ok(Picture::Rgba(Rgba { width, height, pixels: buffer })),
        (ColorType::Rgb, BitDepth::Eight) => Ok(Picture::Rgba(Rgba {
            width,
            height,
            pixels: buffer.as_chunks::<3>().0.iter().flat_map(|px| [px[0], px[1], px[2], 0xff]).collect(),
        })),
        (ColorType::Indexed, BitDepth::Four | BitDepth::Eight) => {
            let bits = if frame.bit_depth == BitDepth::Four { 4 } else { 8 };
            let palette = info.palette.as_deref().unwrap_or_default();
            let alpha = info.trns.as_deref().unwrap_or_default();
            let colors = palette
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
                .map(|(index, rgb)| [rgb[0], rgb[1], rgb[2], alpha.get(index).copied().unwrap_or(0xff)])
                .collect();
            let indices = unpack_rows(width as usize, height as usize, bits, line, &buffer);
            Ok(Picture::Indexed(Indexed { width, height, bits, colors, indices }))
        }
        (color, depth) => {
            Err(Error::PngPicture(format!("{color:?} at {depth:?} bits is not a picture the importer reads")))
        }
    }
}

fn decode_error(error: png::DecodingError) -> Error {
    Error::PngPicture(error.to_string())
}

/// Packs one index per pixel into PNG scanlines: `bits` per pixel, the first
/// pixel in the high bits, each row padded with zero bits to a whole byte.
fn pack_rows(width: usize, bits: u8, indices: &[u8]) -> Vec<u8> {
    if width == 0 {
        return Vec::new();
    }
    let per_byte = usize::from(8 / bits);
    let mask = ((1u16 << bits) - 1) as u8;
    let mut packed = Vec::new();
    for row in indices.chunks(width) {
        for group in row.chunks(per_byte) {
            packed.push(group.iter().enumerate().fold(0u8, |byte, (slot, &index)| {
                byte | ((index & mask) << (8 - u32::from(bits) * (slot as u32 + 1)))
            }));
        }
    }
    packed
}

/// Unpacks PNG scanlines of `bits` per pixel, `line` bytes each, into one index
/// per pixel.
fn unpack_rows(width: usize, height: usize, bits: u8, line: usize, data: &[u8]) -> Vec<u8> {
    let per_byte = usize::from(8 / bits);
    let mask = ((1u16 << bits) - 1) as u8;
    let mut indices = Vec::with_capacity(width * height);
    for row in data.chunks_exact(line).take(height) {
        for x in 0..width {
            let slot = (x % per_byte) as u32;
            indices.push((row[x / per_byte] >> (8 - u32::from(bits) * (slot + 1))) & mask);
        }
    }
    indices
}
