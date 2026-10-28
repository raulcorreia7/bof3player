//! The crate's one error type.

use core::fmt;

/// Everything a parser or codec in this crate can report.
///
/// # Examples
///
/// ```
/// let error = psx_formats::Error::VabBadMagic;
/// assert_eq!(error.to_string(), "not a VAB header (bad magic)");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A palette payload holds an odd number of bytes.
    #[cfg(feature = "std")]
    PaletteOddByteCount(usize),
    /// A palette's word count is not a whole number of CLUT rows.
    #[cfg(feature = "std")]
    PaletteNotRowAligned(usize),
    /// A VAB header does not open with the bank magic.
    VabBadMagic,
    /// A VAB header is not the size its program count gives it.
    VabHeaderSize {
        /// Bytes the header holds.
        length: usize,
        /// Bytes its program count gives.
        expected: usize,
    },
    /// A VAG number names no sample of the bank.
    VagOutOfRange {
        /// The 1-based number asked for.
        index: u32,
        /// Samples the bank declares.
        count: u16,
    },
    /// A VAG range is shorter than one ADPCM block.
    VagEmpty(u32),
    /// A VAG range reaches past the body.
    VagOutsideBody(u32),
    /// A SEP package does not open with the sequence magic.
    SepBadMagic,
    /// A SEP package declares a version other than zero.
    SepVersion(u16),
    /// A SEP package is shorter than one sequence header.
    SepTooShort(usize),
    /// A sequence header or payload does not fit the bytes that follow it.
    SepSequenceOverflow {
        /// Position of the sequence in the package.
        sequence: usize,
    },
    /// An event's bytes are not the ones its status byte and the format need.
    SepEvent {
        /// Position of the sequence in the package.
        sequence: usize,
        /// What is wrong.
        reason: &'static str,
    },
    /// A sequence number names no sequence of the package.
    SepSequenceOutOfRange {
        /// The position asked for.
        index: usize,
        /// Sequences the package holds.
        count: usize,
    },
    /// A sequence states no resolution, so it has no musical time.
    SepNoResolution,
    /// A Standard MIDI File that the sequence reader does not take.
    MidiFile {
        /// What the file breaks or lacks.
        reason: &'static str,
    },
    /// A CD-ROM XA sector is not the stored 2336 bytes.
    XaSectorSize(usize),
    /// A type-3 payload is empty or not a whole number of rectangles.
    #[cfg(feature = "std")]
    VramPayloadLength {
        /// Bytes the payload holds.
        bytes: usize,
    },
    /// A type-3 rectangle count does not tile into rows of the descriptor's width.
    #[cfg(feature = "std")]
    VramRowShape {
        /// Rectangles the payload holds.
        rectangles: usize,
        /// Rectangles per row the descriptor declares.
        chunks_per_row: u8,
    },
    /// A word grid is not whole 32-by-32 rectangles, or its word count does not match its size.
    #[cfg(feature = "std")]
    VramGridShape {
        /// Grid width in words.
        width: usize,
        /// Grid height in rows.
        height: usize,
        /// Words the grid holds.
        words: usize,
    },
    /// A PNG does not decode, or is not 8-bit RGB or RGBA or 4- or 8-bit indexed.
    #[cfg(feature = "std")]
    PngPicture(String),
    /// A TIM file does not decode, or does not hold what this codec reads.
    #[cfg(feature = "std")]
    Tim(String),
    /// A SoundFont 2 file the reader does not accept, and why.
    #[cfg(feature = "std")]
    SoundFont(&'static str),
    /// A bank the VAB writer cannot write, and why.
    VabEncode(&'static str),
    /// A WAVE file cannot hold the given shape, and why.
    WaveShape(&'static str),
    /// The bytes are not a WAVE file this reader takes, and why.
    WaveFormat(&'static str),
    /// A WAVE file uses a feature this reader does not implement, and why.
    WaveUnsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "std")]
            Self::PaletteOddByteCount(bytes) => {
                write!(f, "palette payload has an odd byte count ({bytes})")
            }
            #[cfg(feature = "std")]
            Self::PaletteNotRowAligned(words) => write!(
                f,
                "palette holds {words} color words; a CLUT bank is a whole number of \
                 {}-color rows",
                crate::gfx::clut::ROW_WORDS_4BPP
            ),
            Self::VabBadMagic => write!(f, "not a VAB header (bad magic)"),
            Self::VabHeaderSize { length, expected } => {
                write!(f, "VAB header is {length} bytes; its program count gives {expected}")
            }
            Self::VagOutOfRange { index, count } => {
                write!(f, "VAG {index} is out of range (bank has {count})")
            }
            Self::VagEmpty(index) => write!(f, "VAG {index} is empty"),
            Self::VagOutsideBody(index) => write!(f, "VAG {index} lies outside the VAB body"),
            Self::SepBadMagic => write!(f, "not a SEP package (bad magic)"),
            Self::SepVersion(version) => write!(f, "SEP package declares version {version}"),
            Self::SepTooShort(length) => write!(f, "SEP package is {length} bytes, shorter than one sequence header"),
            Self::SepSequenceOverflow { sequence } => {
                write!(f, "SEP sequence {sequence} runs past the end of the package")
            }
            Self::SepEvent { sequence, reason } => write!(f, "SEP sequence {sequence}: {reason}"),
            Self::SepSequenceOutOfRange { index, count } => {
                write!(f, "sequence {index} is out of range (package has {count})")
            }
            Self::SepNoResolution => write!(f, "sequence has no resolution"),
            Self::MidiFile { reason } => write!(f, "not a MIDI file this reader takes: {reason}"),
            Self::XaSectorSize(length) => {
                write!(f, "XA sector is {length} bytes; it must be 2336")
            }
            #[cfg(feature = "std")]
            Self::VramPayloadLength { bytes } => {
                write!(f, "payload length {bytes} is not a nonzero multiple of {}", crate::gfx::vram::RECTANGLE_BYTES)
            }
            #[cfg(feature = "std")]
            Self::VramRowShape { rectangles, chunks_per_row } => {
                write!(f, "{rectangles} rectangles do not divide into rows of {chunks_per_row}")
            }
            #[cfg(feature = "std")]
            Self::VramGridShape { width, height, words } => write!(
                f,
                "a {width}x{height} word grid is not whole {}-word rectangles, or holds {words} words",
                crate::gfx::vram::RECTANGLE_SIDE
            ),
            #[cfg(feature = "std")]
            Self::PngPicture(reason) => write!(f, "PNG: {reason}"),
            #[cfg(feature = "std")]
            Self::Tim(reason) => write!(f, "TIM: {reason}"),
            #[cfg(feature = "std")]
            Self::SoundFont(reason) => write!(f, "SoundFont: {reason}"),
            Self::VabEncode(reason) => write!(f, "VAB: {reason}"),
            Self::WaveShape(reason) | Self::WaveFormat(reason) | Self::WaveUnsupported(reason) => {
                write!(f, "WAVE: {reason}")
            }
        }
    }
}

impl core::error::Error for Error {}

/// The crate's result type.
///
/// # Examples
///
/// ```
/// fn first(bytes: &[u8]) -> psx_formats::Result<u8> {
///     bytes.first().copied().ok_or(psx_formats::Error::XaSectorSize(0))
/// }
///
/// assert_eq!(first(&[1]), Ok(1));
/// assert!(first(&[]).is_err());
/// ```
pub type Result<T, E = Error> = core::result::Result<T, E>;
