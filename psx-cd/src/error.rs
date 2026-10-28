//! The crate's one error type.
//!
//! Every operation reports [`Error`]; there is no second error enum.
//!
//! # Examples
//!
//! ```
//! let error = psx_cd::Error::InvalidImage("missing primary volume descriptor");
//! assert_eq!(error.to_string(), "invalid disc image: missing primary volume descriptor");
//! assert!(std::error::Error::source(&error).is_none());
//! ```

use std::fmt;

/// Every failure one of this crate's operations can report.
///
/// # Examples
///
/// ```
/// let error = psx_cd::Error::NoImage("no image in the folder".into());
/// assert_eq!(error.to_string(), "no image in the folder");
/// ```
#[derive(Debug)]
pub enum Error {
    /// An input or output file could not be read or written.
    Io(std::io::Error),
    /// The image is not a valid ISO9660 or CUE source; the message is fixed.
    InvalidImage(&'static str),
    /// The image is invalid; the message names the file or the range at fault.
    InvalidImageDetail(String),
    /// A rebuild input is not a folder of top-level regular files.
    InvalidInput(&'static str),
    /// No image to extract could be found or chosen; the message is complete.
    NoImage(String),
    /// A patch the disc cannot take: a missing file, a file that would grow, or
    /// a sector that is not a file sector. The message is complete.
    Patch(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::InvalidImage(message) => write!(f, "invalid disc image: {message}"),
            Self::InvalidImageDetail(message) => write!(f, "invalid disc image: {message}"),
            Self::InvalidInput(message) => write!(f, "invalid rebuild input: {message}"),
            Self::NoImage(message) | Self::Patch(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The crate's result type.
///
/// # Examples
///
/// ```
/// fn parse(value: &str) -> psx_cd::Result<u8> {
///     value.parse().map_err(|_| psx_cd::Error::InvalidImage("not a number"))
/// }
/// assert!(parse("7").is_ok());
/// assert!(parse("x").is_err());
/// ```
pub type Result<T, E = Error> = std::result::Result<T, E>;
