// SPDX-License-Identifier: MIT
//! Where the bytes come from.
//!
//! A [`Source`] is a read-only volume of files addressed by slash separated
//! paths (`ASSETS/DATA.BIN`, `STREAMS/TRACK01.STR`, `CDDA/TRACK02.RAW`). A
//! [`Blob`] is one file's random-access bytes. Three families of source cover
//! how an engine hands data over:
//!
//! * **Memory**: [`MemorySource`] borrows (zero-copy) or owns byte slices the
//!   engine already loaded, or any [`Blob`].
//! * **Callbacks and readers**: [`CallbackSource`] forwards to the engine's own
//!   VFS; [`ReaderBlob`] wraps any `Read + Seek`.
//! * **Paths**: [`FolderSource`] (an extracted tree) and [`DiscSource`] (a
//!   `.cue`/`.bin` image), available with the `std` feature.
//!
//! # Examples
//!
//! ```
//! use psx_audio::{MemorySource, Source};
//!
//! let mut source = MemorySource::new();
//! source.add("ASSETS/DATA.BIN", b"hello");
//! assert_eq!(source.list()?, ["ASSETS/DATA.BIN"]);
//! assert_eq!(&*source.open("assets\\data.bin")?.bytes()?, b"hello");
//! # Ok::<(), psx_audio::Error>(())
//! ```

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::error::{Error, Result};

#[cfg(feature = "std")]
mod disc;
#[cfg(feature = "std")]
mod folder;
#[cfg(feature = "std")]
mod reader;

#[cfg(feature = "std")]
pub use disc::DiscSource;
#[cfg(feature = "std")]
pub use folder::FolderSource;
#[cfg(feature = "std")]
pub use reader::ReaderBlob;

/// Random-access bytes of one file.
///
/// Implementations must be thread-safe; the library may read from the thread
/// that renders audio.
///
/// # Examples
///
/// ```
/// use psx_audio::{Blob, MemorySource, Source};
///
/// let mut source = MemorySource::new();
/// source.add("A.BIN", b"hello");
/// let blob = source.open("A.BIN")?;
/// assert_eq!(blob.len(), 5);
/// let mut buf = [0u8; 3];
/// assert_eq!(blob.read_at(1, &mut buf)?, 3);
/// assert_eq!(&buf, b"ell");
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub trait Blob: Send + Sync {
    /// Total size in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// assert_eq!(source.open("A.BIN")?.len(), 5);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn len(&self) -> u64;

    /// Whether the blob is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"");
    /// assert!(source.open("A.BIN")?.is_empty());
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read up to `buf.len()` bytes at `offset`; returns how many were read
    /// (0 at or past the end).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// let blob = source.open("A.BIN")?;
    /// let mut buf = [0u8; 8];
    /// assert_eq!(blob.read_at(3, &mut buf)?, 2);
    /// assert_eq!(blob.read_at(5, &mut buf)?, 0);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;

    /// The whole content when it is already in memory, so callers can avoid a copy.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// assert_eq!(source.open("A.BIN")?.as_slice(), Some(&b"hello"[..]));
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn as_slice(&self) -> Option<&[u8]> {
        None
    }
}

impl dyn Blob + '_ {
    /// The whole file: borrowed when in memory, read otherwise.
    ///
    /// # Errors
    ///
    /// Fails when a read fails; a short final read ends the data instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// assert_eq!(&*source.open("A.BIN")?.bytes()?, b"hello");
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn bytes(&self) -> Result<Cow<'_, [u8]>> {
        if let Some(slice) = self.as_slice() {
            return Ok(Cow::Borrowed(slice));
        }
        let mut data = alloc::vec![0u8; self.len() as usize];
        let mut done = 0;
        while done < data.len() {
            let got = self.read_at(done as u64, &mut data[done..])?;
            if got == 0 {
                data.truncate(done);
                break;
            }
            done += got;
        }
        Ok(Cow::Owned(data))
    }

    /// Fill `buf` exactly from `offset`.
    ///
    /// # Errors
    ///
    /// Fails with an i/o error when the file ends before `buf` is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// let blob = source.open("A.BIN")?;
    /// let mut buf = [0u8; 5];
    /// blob.read_exact_at(0, &mut buf)?;
    /// assert_eq!(&buf, b"hello");
    /// assert!(blob.read_exact_at(0, &mut [0u8; 6]).is_err());
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut done = 0;
        while done < buf.len() {
            let got = self.read_at(offset + done as u64, &mut buf[done..])?;
            if got == 0 {
                return Err(Error::io("unexpected end of file"));
            }
            done += got;
        }
        Ok(())
    }
}

/// A read-only volume of files.
///
/// # Examples
///
/// ```
/// use psx_audio::{MemorySource, Source};
///
/// let mut source = MemorySource::new();
/// source.add("ASSETS/DATA.BIN", b"hello");
/// assert_eq!(source.list()?, ["ASSETS/DATA.BIN"]);
/// assert_eq!(source.open("ASSETS/DATA.BIN")?.len(), 5);
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub trait Source: Send + Sync {
    /// Every file path, `/`-separated, relative to the volume root.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"");
    /// assert_eq!(source.list()?, ["A.BIN"]);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn list(&self) -> Result<Vec<String>>;

    /// Open one file by path (ASCII case-insensitive).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// assert_eq!(source.open("a.bin")?.len(), 5);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    fn open<'a>(&'a self, path: &str) -> Result<Arc<dyn Blob + 'a>>;

    /// A short description for diagnostics (`folder /path`, `disc image ...`).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"");
    /// assert!(source.describe().starts_with("memory"));
    /// ```
    fn describe(&self) -> String {
        String::from("source")
    }
}

impl<S: Source + ?Sized> Source for Box<S> {
    fn list(&self) -> Result<Vec<String>> {
        (**self).list()
    }
    fn open<'a>(&'a self, path: &str) -> Result<Arc<dyn Blob + 'a>> {
        (**self).open(path)
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// A blob over a byte slice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SliceBlob<'a>(pub(crate) &'a [u8]);

impl Blob for SliceBlob<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(self.0.len());
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }
    fn as_slice(&self) -> Option<&[u8]> {
        Some(self.0)
    }
}

fn normalize(path: &str) -> String {
    path.trim_start_matches('/').replace('\\', "/").to_ascii_lowercase()
}

/// Files the engine already holds in memory, or any other [`Blob`].
///
/// Slices are borrowed, not copied: the source cannot outlive them.
///
/// # Examples
///
/// ```
/// use psx_audio::{MemorySource, Source};
///
/// let mut source = MemorySource::new();
/// source.add("A.BIN", b"hello");
/// assert_eq!(source.list().unwrap(), ["A.BIN"]);
/// ```
#[derive(Default)]
pub struct MemorySource<'a> {
    files: BTreeMap<String, (String, Arc<dyn Blob + 'a>)>,
}

impl core::fmt::Debug for MemorySource<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemorySource").field("files", &self.files.len()).finish()
    }
}

impl<'a> MemorySource<'a> {
    /// An empty source.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// assert!(MemorySource::new().list().unwrap().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file backed by borrowed bytes (zero-copy).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add("A.BIN", b"hello");
    /// assert_eq!(source.open("A.BIN").unwrap().as_slice(), Some(&b"hello"[..]));
    /// ```
    pub fn add(&mut self, path: &str, bytes: &'a [u8]) -> &mut Self {
        self.add_blob(path, Arc::new(SliceBlob(bytes)))
    }

    /// Add a file that owns its bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Source};
    ///
    /// let mut source = MemorySource::new();
    /// source.add_owned("A.BIN", vec![1, 2, 3]);
    /// assert_eq!(source.open("A.BIN").unwrap().len(), 3);
    /// ```
    pub fn add_owned(&mut self, path: &str, bytes: Vec<u8>) -> &mut Self {
        self.add_blob(path, Arc::new(OwnedBlob(bytes)))
    }

    /// Add a file backed by any blob (a streaming reader, an engine VFS handle).
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{Blob, MemorySource, Source};
    ///
    /// let mut inner = MemorySource::new();
    /// inner.add("A.BIN", b"hello");
    /// let blob = inner.open("A.BIN").unwrap();
    /// let mut source = MemorySource::new();
    /// source.add_blob("COPY.BIN", blob);
    /// assert_eq!(source.open("COPY.BIN").unwrap().len(), 5);
    /// ```
    pub fn add_blob(&mut self, path: &str, blob: Arc<dyn Blob + 'a>) -> &mut Self {
        let shown = path.trim_start_matches('/').replace('\\', "/");
        self.files.insert(normalize(path), (shown, blob));
        self
    }
}

#[derive(Debug)]
struct OwnedBlob(Vec<u8>);

impl Blob for OwnedBlob {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        SliceBlob(&self.0).read_at(offset, buf)
    }
    fn as_slice(&self) -> Option<&[u8]> {
        Some(&self.0)
    }
}

impl Source for MemorySource<'_> {
    fn list(&self) -> Result<Vec<String>> {
        Ok(self.files.values().map(|(shown, _)| shown.clone()).collect())
    }

    fn open<'b>(&'b self, path: &str) -> Result<Arc<dyn Blob + 'b>> {
        self.files
            .get(&normalize(path))
            .map(|(_, blob)| Arc::clone(blob) as Arc<dyn Blob + 'b>)
            .ok_or_else(|| Error::io(alloc::format!("no such file: {path}")))
    }

    fn describe(&self) -> String {
        alloc::format!("memory ({} files)", self.files.len())
    }
}

type ListFn<'a> = dyn Fn() -> Result<Vec<String>> + Send + Sync + 'a;
type SizeFn<'a> = dyn Fn(&str) -> Result<u64> + Send + Sync + 'a;
type ReadFn<'a> = dyn Fn(&str, u64, &mut [u8]) -> Result<usize> + Send + Sync + 'a;

/// The engine's own virtual file system, reached through three callbacks.
///
/// `list` returns every path, `size` the byte size of one path, and `read`
/// fills a buffer from a byte offset and returns how many bytes it wrote
/// (0 at the end of the file).
///
/// # Examples
///
/// ```
/// use psx_audio::{CallbackSource, Source};
///
/// let data = b"hello";
/// let source = CallbackSource::new(
///     || Ok(vec![String::from("A.BIN")]),
///     |_| Ok(5),
///     |_, offset, buf| {
///         let rest = &data[offset as usize..];
///         let n = rest.len().min(buf.len());
///         buf[..n].copy_from_slice(&rest[..n]);
///         Ok(n)
///     },
/// );
/// assert_eq!(source.open("A.BIN")?.len(), 5);
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub struct CallbackSource<'a> {
    list: Arc<ListFn<'a>>,
    size: Arc<SizeFn<'a>>,
    read: Arc<ReadFn<'a>>,
}

impl core::fmt::Debug for CallbackSource<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CallbackSource")
    }
}

impl<'a> CallbackSource<'a> {
    /// Build a source from the three callbacks.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{CallbackSource, Source};
    ///
    /// let source = CallbackSource::new(
    ///     || Ok(vec![String::from("A.BIN")]),
    ///     |_| Ok(0),
    ///     |_, _, _| Ok(0),
    /// );
    /// assert_eq!(source.list()?, ["A.BIN"]);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn new(
        list: impl Fn() -> Result<Vec<String>> + Send + Sync + 'a,
        size: impl Fn(&str) -> Result<u64> + Send + Sync + 'a,
        read: impl Fn(&str, u64, &mut [u8]) -> Result<usize> + Send + Sync + 'a,
    ) -> Self {
        Self { list: Arc::new(list), size: Arc::new(size), read: Arc::new(read) }
    }
}

struct CallbackBlob<'a> {
    path: String,
    size: u64,
    read: Arc<ReadFn<'a>>,
}

impl Blob for CallbackBlob<'_> {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.size {
            return Ok(0);
        }
        (self.read)(&self.path, offset, buf)
    }
}

impl Source for CallbackSource<'_> {
    fn list(&self) -> Result<Vec<String>> {
        (self.list)()
    }

    fn open<'b>(&'b self, path: &str) -> Result<Arc<dyn Blob + 'b>> {
        let size = (self.size)(path)?;
        Ok(Arc::new(CallbackBlob { path: path.to_string(), size, read: Arc::clone(&self.read) }))
    }

    fn describe(&self) -> String {
        String::from("callbacks")
    }
}
