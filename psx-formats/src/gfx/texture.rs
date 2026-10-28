//! Texture-page and CLUT words of the PlayStation GPU, and the packed texel rows of
//! 4bpp and 8bpp textures.
//!
//! A textured primitive names the texture it samples with a `tpage` word and the
//! palette row it reads with a `clut` word. [`Tpage`] and [`Clut`] decode and encode
//! those words. [`texel_indices`] unpacks a texel row: each VRAM word holds
//! `16 / depth` texel indices, the lowest bits first. The codec knows nothing of
//! which texture or palette a draw uses; that association belongs to the caller.
//!
//! # Examples
//!
//! ```
//! use psx_formats::gfx::texture::{Depth, texel_indices, texel_words};
//!
//! let words = texel_words(&[1, 2, 3, 4], Depth::Four);
//! assert_eq!(words, [0x4321]);
//! assert_eq!(texel_indices(&words, Depth::Four), [1, 2, 3, 4]);
//! ```

/// VRAM words per texture-page column step (`tpage` bits 0-3).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::texture::PAGE_COLUMN_WORDS, 64);
/// ```
pub const PAGE_COLUMN_WORDS: u16 = 64;
/// VRAM rows between the two texture-page rows (`tpage` bit 4).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::texture::PAGE_ROW_ROWS, 256);
/// ```
pub const PAGE_ROW_ROWS: u16 = 256;
/// VRAM words per CLUT column step: a CLUT starts at a multiple of this.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::gfx::texture::CLUT_COLUMN_WORDS, 16);
/// ```
pub const CLUT_COLUMN_WORDS: u16 = 16;
/// The CLUT word holds the VRAM row from this bit up (`clut` bits 6-14).
const CLUT_ROW_SHIFT: u32 = 6;
/// Bits of the CLUT word that hold the VRAM row.
const CLUT_ROW_MASK: u16 = 0x1ff;
/// The `tpage` bit that selects the lower page row.
const PAGE_ROW_BIT: u16 = 0x10;
/// The `tpage` bits that hold the texture color mode (bits 7-8).
const COLOR_SHIFT: u32 = 7;
/// The `tpage` bits that hold the semi-transparency mode (bits 5-6).
const BLEND_SHIFT: u32 = 5;
/// Bits 0-8 of a `tpage` word: the fields this module reads and writes.
const TPAGE_FIELD_MASK: u16 = 0x1ff;

/// The bit depth of a palettized texel.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::Depth;
///
/// assert_eq!(Depth::Four.bits(), 4);
/// assert_eq!(Depth::Eight.colors(), 256);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// Four-bit indices into a 16-color CLUT row.
    Four,
    /// Eight-bit indices into a 256-color CLUT row.
    Eight,
}

impl Depth {
    /// Bits per texel index.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::texture::Depth::Eight.bits(), 8);
    /// ```
    pub fn bits(self) -> u32 {
        match self {
            Self::Four => 4,
            Self::Eight => 8,
        }
    }

    /// Texel indices packed in one VRAM word.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::texture::Depth::Four.texels_per_word(), 4);
    /// ```
    pub fn texels_per_word(self) -> usize {
        (u16::BITS / self.bits()) as usize
    }

    /// Colors in one CLUT row of this depth.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::texture::Depth::Four.colors(), 16);
    /// ```
    pub fn colors(self) -> usize {
        1 << self.bits()
    }
}

/// The color mode a texture page selects (`tpage` bits 7-8).
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::{Color, Depth, Tpage};
///
/// let page = Tpage { x: 0, y: 0, color: Color::Indexed(Depth::Eight), blend: 0 };
/// assert_eq!(page.word(), 1 << 7);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    /// Palettized texels read through a CLUT.
    Indexed(Depth),
    /// Direct 15-bit texels; no CLUT is read.
    Direct15,
}

/// The fields of a texture-page word.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::{Color, Depth, Tpage};
///
/// let page = Tpage::from_word(0x0080).unwrap();
/// assert_eq!(page.color, Color::Indexed(Depth::Eight));
/// assert_eq!(Tpage { x: 64, y: 0, ..page }.word(), 0x0081);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tpage {
    /// The page's left edge in VRAM words.
    pub x: u16,
    /// The page's top edge in VRAM rows.
    pub y: u16,
    /// The texel color mode.
    pub color: Color,
    /// The semi-transparency mode (bits 5-6), `0` to `3`.
    pub blend: u8,
}

impl Tpage {
    /// Decodes a texture-page word. Returns `None` for color mode `3`, which the GPU
    /// reserves. Dither and display bits (9-15) are not part of the model.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::texture::Tpage;
    ///
    /// assert!(Tpage::from_word(0x0180).is_none(), "color mode 3 is reserved");
    /// assert_eq!(Tpage::from_word(0x0000).unwrap().x, 0);
    /// ```
    pub fn from_word(word: u16) -> Option<Self> {
        let color = match (word >> COLOR_SHIFT) & 3 {
            0 => Color::Indexed(Depth::Four),
            1 => Color::Indexed(Depth::Eight),
            2 => Color::Direct15,
            _ => return None,
        };
        Some(Self {
            x: (word & 0xf) * PAGE_COLUMN_WORDS,
            y: if word & PAGE_ROW_BIT != 0 { PAGE_ROW_ROWS } else { 0 },
            color,
            blend: ((word >> BLEND_SHIFT) & 3) as u8,
        })
    }

    /// Encodes the fields back to a texture-page word (bits 0-8 only).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::texture::Tpage;
    ///
    /// let page = Tpage::from_word(0x0040).unwrap();
    /// assert_eq!(page.word(), 0x0040);
    /// ```
    pub fn word(self) -> u16 {
        let color = match self.color {
            Color::Indexed(Depth::Four) => 0,
            Color::Indexed(Depth::Eight) => 1,
            Color::Direct15 => 2,
        };
        let page_row = if self.y >= PAGE_ROW_ROWS { PAGE_ROW_BIT } else { 0 };
        let page_column = self.x / PAGE_COLUMN_WORDS;
        ((color << COLOR_SHIFT) | (u16::from(self.blend) << BLEND_SHIFT) | page_row | page_column) & TPAGE_FIELD_MASK
    }
}

/// A CLUT's VRAM position: `x` in words (a multiple of 16) and `y` in rows.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::Clut;
///
/// let clut = Clut::new(16, 2).unwrap();
/// assert_eq!(clut.word(), 0x0081);
/// assert_eq!(Clut::from_word(clut.word()), clut);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Clut {
    /// The first color's VRAM column in words.
    pub x: u16,
    /// The VRAM row.
    pub y: u16,
}

impl Clut {
    /// Decodes a `clut` word (`x / 16` in bits 0-5, the row in bits 6-14).
    ///
    /// # Examples
    ///
    /// ```
    /// let clut = psx_formats::gfx::texture::Clut::from_word(0x0081);
    /// assert_eq!((clut.x, clut.y), (16, 2));
    /// ```
    pub fn from_word(word: u16) -> Self {
        Self { x: (word & 0x3f) * CLUT_COLUMN_WORDS, y: (word >> CLUT_ROW_SHIFT) & CLUT_ROW_MASK }
    }

    /// The position as a `clut` word, or `None` when `x` is not a multiple of 16 or
    /// either field is outside VRAM.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::gfx::texture::Clut;
    ///
    /// assert!(Clut::new(8, 0).is_none(), "a CLUT starts on a multiple of 16");
    /// assert_eq!(Clut::new(32, 0).unwrap().x, 32);
    /// ```
    pub fn new(x: u16, y: u16) -> Option<Self> {
        let in_vram = usize::from(x) < crate::gfx::vram::VRAM_WIDTH && usize::from(y) < crate::gfx::vram::VRAM_HEIGHT;
        (x.is_multiple_of(CLUT_COLUMN_WORDS) && in_vram).then_some(Self { x, y })
    }

    /// Encodes the position as a `clut` word.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_formats::gfx::texture::Clut::new(32, 1).unwrap().word(), 0x0042);
    /// ```
    pub fn word(self) -> u16 {
        (self.x / CLUT_COLUMN_WORDS) | (self.y << CLUT_ROW_SHIFT)
    }
}

/// Packs texel indices into VRAM words, lowest bits first: the inverse of
/// [`texel_indices`]. A short last word keeps its low texels and reads zero above
/// them.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::{Depth, texel_words};
///
/// assert_eq!(texel_words(&[1, 2, 3, 4], Depth::Four), [0x4321]);
/// ```
pub fn texel_words(indices: &[u8], depth: Depth) -> Vec<u16> {
    let bits = depth.bits();
    let mask = (1u16 << bits) - 1;
    indices
        .chunks(depth.texels_per_word())
        .map(|texels| {
            texels
                .iter()
                .enumerate()
                .fold(0u16, |word, (texel, &index)| word | ((u16::from(index) & mask) << (texel as u32 * bits)))
        })
        .collect()
}

/// Unpacks the texel indices of a row of VRAM words, lowest bits first.
///
/// # Examples
///
/// ```
/// use psx_formats::gfx::texture::{Depth, texel_indices};
///
/// assert_eq!(texel_indices(&[0x4321], Depth::Four), [1, 2, 3, 4]);
/// ```
pub fn texel_indices(words: &[u16], depth: Depth) -> Vec<u8> {
    let bits = depth.bits();
    let mask = (1u16 << bits) - 1;
    let mut indices = Vec::with_capacity(words.len() * depth.texels_per_word());
    for word in words {
        for texel in 0..depth.texels_per_word() {
            indices.push(((word >> (texel as u32 * bits)) & mask) as u8);
        }
    }
    indices
}
