// SPDX-License-Identifier: MIT
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

use super::Blob;
use crate::error::{Error, Result};

/// A [`Blob`] over any `Read + Seek`, for engines that hand over a stream
/// (a pak-file handle, a decompressor with seek support).
///
/// Reads take an internal lock, so concurrent readers serialize.
///
/// # Examples
///
/// ```
/// use psx_audio::{Blob, ReaderBlob};
///
/// let blob = ReaderBlob::new(std::io::Cursor::new(b"hello".to_vec())).unwrap();
/// let mut buf = [0u8; 2];
/// assert_eq!(blob.read_at(3, &mut buf).unwrap(), 2);
/// assert_eq!(&buf, b"lo");
/// ```
#[derive(Debug)]
pub struct ReaderBlob<R> {
    inner: Mutex<R>,
    len: u64,
}

impl<R: Read + Seek + Send> ReaderBlob<R> {
    /// Wrap `reader`; its length is found by seeking to the end.
    ///
    /// # Errors
    ///
    /// Fails when the final seek fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{Blob, ReaderBlob};
    ///
    /// let blob = ReaderBlob::new(std::io::Cursor::new(vec![0u8; 16])).unwrap();
    /// assert_eq!(blob.len(), 16);
    /// ```
    pub fn new(mut reader: R) -> Result<Self> {
        let len = reader.seek(SeekFrom::End(0))?;
        Ok(Self { inner: Mutex::new(reader), len })
    }
}

impl<R: Read + Seek + Send> Blob for ReaderBlob<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let mut reader = self.inner.lock().map_err(|_| Error::State("reader lock poisoned".into()))?;
        reader.seek(SeekFrom::Start(offset))?;
        let want = buf.len().min((self.len - offset) as usize);
        Ok(reader.read(&mut buf[..want])?)
    }
}
