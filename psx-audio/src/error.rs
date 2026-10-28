// SPDX-License-Identifier: MIT
//! The one error type of the library.

use alloc::string::String;
use core::fmt;

/// Result alias used across the crate.
///
/// # Examples
///
/// ```
/// fn first(bytes: &[u8]) -> psx_audio::Result<u8> {
///     bytes.first().copied().ok_or_else(|| psx_audio::Error::invalid("empty input"))
/// }
/// assert_eq!(first(b"a")?, b'a');
/// assert!(first(b"").is_err());
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub type Result<T> = core::result::Result<T, Error>;

/// Why an operation failed.
///
/// Every variant carries a human-readable message that names the offending
/// file, item or field. Match on the variant to decide what to do; show the
/// message to the user.
///
/// # Examples
///
/// ```
/// let error = psx_audio::Error::Unsupported("no encoder".into());
/// assert_eq!(error.to_string(), "unsupported: no encoder");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A read or write on the underlying storage failed.
    Io(String),
    /// The bytes are not the format they claim to be, or are truncated.
    Format(String),
    /// A caller-supplied argument is out of range or names nothing.
    Invalid(String),
    /// The input is valid but uses a feature this library does not implement.
    Unsupported(String),
    /// A call was made in the wrong state (for example rendering after `stop`).
    State(String),
}

impl Error {
    /// A malformed or truncated input.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = psx_audio::Error::format("bad block header");
    /// assert_eq!(error.message(), "bad block header");
    /// ```
    pub fn format(message: impl Into<String>) -> Self {
        Self::Format(message.into())
    }

    /// A caller-supplied argument that is out of range or names nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = psx_audio::Error::invalid("channel 9 does not exist");
    /// assert!(matches!(error, psx_audio::Error::Invalid(_)));
    /// ```
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// A failed read or write on the underlying storage.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = psx_audio::Error::io("unexpected end of file");
    /// assert_eq!(error.message(), "unexpected end of file");
    /// ```
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }

    /// Prefix the message with where the error happened.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = psx_audio::Error::io("unexpected end").context("BGM/TRACK.STR");
    /// assert_eq!(error.message(), "BGM/TRACK.STR: unexpected end");
    /// ```
    #[must_use]
    pub fn context(self, what: &str) -> Self {
        let wrap = |m: String| alloc::format!("{what}: {m}");
        match self {
            Self::Io(m) => Self::Io(wrap(m)),
            Self::Format(m) => Self::Format(wrap(m)),
            Self::Invalid(m) => Self::Invalid(wrap(m)),
            Self::Unsupported(m) => Self::Unsupported(wrap(m)),
            Self::State(m) => Self::State(wrap(m)),
        }
    }

    /// The message without the category.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = psx_audio::Error::Format("bad block".into());
    /// assert_eq!(error.message(), "bad block");
    /// assert_eq!(error.to_string(), "bad format: bad block");
    /// ```
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Io(m) | Self::Format(m) | Self::Invalid(m) | Self::Unsupported(m) | Self::State(m) => m,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Io(_) => "i/o error",
            Self::Format(_) => "bad format",
            Self::Invalid(_) => "invalid argument",
            Self::Unsupported(_) => "unsupported",
            Self::State(_) => "wrong state",
        };
        write!(f, "{kind}: {}", self.message())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[cfg(feature = "std")]
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(alloc::format!("{error}"))
    }
}

impl From<psx_formats::Error> for Error {
    fn from(error: psx_formats::Error) -> Self {
        use psx_formats::Error as Source;
        match error {
            Source::VagOutOfRange { .. } | Source::SepSequenceOutOfRange { .. } => {
                Self::Invalid(alloc::format!("{error}"))
            }
            _ => Self::Format(alloc::format!("{error}")),
        }
    }
}
