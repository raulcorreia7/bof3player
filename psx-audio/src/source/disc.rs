// SPDX-License-Identifier: MIT
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use psx_cd::cue::{self, TrackMode};
use psx_cd::{Entry, Image};

use super::folder::file_blob;
use super::{Blob, Source};
use crate::error::{Error, Result};

/// A lone image smaller than this is not taken for a disc image: it is more
/// likely the CD-DA track next to one.
const MIN_DISC_IMAGE_BYTES: u64 = 400 * 1024 * 1024;

struct AudioTrack {
    name: String,
    blob: Arc<dyn Blob>,
}

/// A disc image (`.bin`/`.iso`, optionally described by a `.cue`) read through
/// its ISO 9660 volume.
///
/// Raw 2352-byte and plain 2048-byte images are understood. XA streams
/// (`*.STR`) are exposed as 2336-byte sectors; the audio track, when known, is
/// the pseudo file `CDDA/TRACKnn.RAW` of raw 2352-byte sectors.
///
/// # Examples
///
/// ```
/// use psx_audio::{DiscSource, Source};
///
/// let dir = tempfile::tempdir().unwrap();
/// let input = dir.path().join("input");
/// std::fs::create_dir(&input).unwrap();
/// std::fs::write(input.join("A.TXT"), b"hi").unwrap();
/// let image = dir.path().join("disc.iso");
/// psx_cd::rebuild::iso(&input, &image).unwrap();
/// let bytes = std::fs::read(&image).unwrap();
/// let source = DiscSource::from_reader(std::io::Cursor::new(bytes)).unwrap();
/// assert!(source.list().unwrap().contains(&String::from("A.TXT")));
/// ```
pub struct DiscSource {
    image: Mutex<Image>,
    entries: Vec<Entry>,
    raw: bool,
    audio: Option<AudioTrack>,
    label: String,
}

impl std::fmt::Debug for DiscSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscSource").field("label", &self.label).finish_non_exhaustive()
    }
}

fn cd_error(error: psx_cd::Error) -> Error {
    Error::io(error.to_string())
}

impl DiscSource {
    /// Open a disc from a `.cue` file, an image file, or a directory that holds one.
    ///
    /// # Errors
    ///
    /// Fails when the path names no readable image, or its ISO 9660 volume is malformed.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = tempfile::tempdir().unwrap();
    /// let input = dir.path().join("input");
    /// std::fs::create_dir(&input).unwrap();
    /// std::fs::write(input.join("A.TXT"), b"hi").unwrap();
    /// let image = dir.path().join("disc.iso");
    /// psx_cd::rebuild::iso(&input, &image).unwrap();
    /// let cue = dir.path().join("disc.cue");
    /// std::fs::write(&cue, "FILE \"disc.iso\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00").unwrap();
    /// assert!(psx_audio::DiscSource::open(&cue).is_ok());
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let (image_path, cue_path) = resolve(path.as_ref())?;
        let image = Image::open(&image_path).map_err(cd_error)?;
        let mut source = Self::from_image(image, format!("disc image {}", image_path.display()))?;
        if let Some(cue_path) = cue_path {
            let sheet = cue::read(&cue_path).map_err(cd_error)?;
            let audio = sheet.tracks.iter().position(|track| track.mode == TrackMode::Audio);
            if let Some(position) = audio {
                let (start, end) = cue::audio_track_range(&sheet, position).map_err(cd_error)?;
                let track = &sheet.tracks[position];
                if end > start {
                    let whole = file_blob(&track.file)?;
                    let offset = start * cue::AUDIO_SECTOR_SIZE;
                    let len = (end - start) * cue::AUDIO_SECTOR_SIZE;
                    let blob = Arc::new(SubBlob { inner: whole, offset, len });
                    source.audio = Some(AudioTrack { name: audio_name(u32::from(track.number)), blob });
                }
            }
        }
        Ok(source)
    }

    /// Read a disc image from any `Read + Seek` (an engine's VFS handle).
    ///
    /// # Errors
    ///
    /// Fails when the image is not a cooked or raw disc image.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = tempfile::tempdir().unwrap();
    /// let input = dir.path().join("input");
    /// std::fs::create_dir(&input).unwrap();
    /// std::fs::write(input.join("A.TXT"), b"hi").unwrap();
    /// let image = dir.path().join("disc.iso");
    /// psx_cd::rebuild::iso(&input, &image).unwrap();
    /// let bytes = std::fs::read(&image).unwrap();
    /// assert!(psx_audio::DiscSource::from_reader(std::io::Cursor::new(bytes)).is_ok());
    /// ```
    pub fn from_reader<R: Read + Seek + Send + 'static>(reader: R) -> Result<Self> {
        let image = Image::from_reader(reader).map_err(cd_error)?;
        Self::from_image(image, String::from("disc image"))
    }

    fn from_image(mut image: Image, label: String) -> Result<Self> {
        let raw = image.is_raw();
        let entries = image.entries().map_err(cd_error)?;
        Ok(Self { image: Mutex::new(image), entries, raw, audio: None, label })
    }
}

fn audio_name(number: u32) -> String {
    format!("CDDA/TRACK{number:02}.RAW")
}

struct SubBlob {
    inner: Arc<dyn Blob>,
    offset: u64,
    len: u64,
}

impl Blob for SubBlob {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let n = buf.len().min((self.len - offset) as usize);
        self.inner.read_at(self.offset + offset, &mut buf[..n])
    }
}

/// One ISO file read through the image's sector table.
struct FileBlob<'a> {
    disc: &'a DiscSource,
    entry: Entry,
    size: u64,
}

impl Blob for FileBlob<'_> {
    fn len(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let mut image = self.disc.image.lock().map_err(|_| Error::State("disc lock poisoned".into()))?;
        image.read_entry_range(&self.entry, offset, &mut buf[..want]).map_err(cd_error)
    }
}

impl Source for DiscSource {
    fn list(&self) -> Result<Vec<String>> {
        let mut files: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| !entry.is_directory)
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();
        if let Some(audio) = &self.audio {
            files.push(audio.name.clone());
        }
        files.sort();
        Ok(files)
    }

    fn open<'a>(&'a self, path: &str) -> Result<Arc<dyn Blob + 'a>> {
        if let Some(audio) = self.audio.as_ref().filter(|a| a.name.eq_ignore_ascii_case(path.trim_start_matches('/'))) {
            return Ok(Arc::clone(&audio.blob) as Arc<dyn Blob + 'a>);
        }
        let wanted = path.trim_start_matches('/').replace('\\', "/");
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                !entry.is_directory && entry.path.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(&wanted)
            })
            .cloned()
            .ok_or_else(|| Error::io(format!("no such file: {path}")))?;
        if entry.size == 0 {
            return Err(Error::io(format!("empty file: {path}")));
        }
        let xa = entry.is_xa;
        if xa && !self.raw {
            return Err(Error::Unsupported(format!("XA stream needs a raw 2352-byte image: {path}")));
        }
        let size = if xa { entry.sectors() * psx_cd::XA_SECTOR_SIZE as u64 } else { u64::from(entry.size) };
        Ok(Arc::new(FileBlob { disc: self, entry, size }))
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

/// Find `(image, cue)` for a user-supplied path.
fn resolve(path: &Path) -> Result<(PathBuf, Option<PathBuf>)> {
    let ext = |p: &Path, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
    let cue_image = |cue: &Path| -> Result<(PathBuf, Option<PathBuf>)> {
        let sheet = cue::read(cue).map_err(cd_error)?;
        let data = sheet
            .tracks
            .iter()
            .find(|track| track.mode != TrackMode::Audio)
            .ok_or_else(|| Error::format(format!("cue sheet has no data track: {}", cue.display())))?;
        Ok((data.file.clone(), Some(cue.to_path_buf())))
    };
    if path.is_dir() {
        let mut cues: Vec<PathBuf> = std::fs::read_dir(path)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && ext(p, "cue"))
            .collect();
        cues.sort();
        if let Some(cue) = cues.first() {
            return cue_image(cue);
        }
        let mut images: Vec<PathBuf> = std::fs::read_dir(path)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && (ext(p, "bin") || ext(p, "iso") || ext(p, "img")))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() >= MIN_DISC_IMAGE_BYTES))
            .collect();
        images.sort();
        return match images.len() {
            0 => Err(Error::io(format!("no cue sheet and no image of at least 400 MiB in {}", path.display()))),
            1 => Ok((images.remove(0), None)),
            _ => Err(Error::io(format!("several disc images in {}", path.display()))),
        };
    }
    if path.is_file() && ext(path, "cue") {
        return cue_image(path);
    }
    if path.is_file() && (ext(path, "bin") || ext(path, "iso") || ext(path, "img")) {
        if let Some(parent) = path.parent()
            && let Ok(entries) = std::fs::read_dir(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })
        {
            let mut cues: Vec<PathBuf> =
                entries.filter_map(std::result::Result::ok).map(|e| e.path()).filter(|p| ext(p, "cue")).collect();
            cues.sort();
            for cue in cues {
                if let Ok((image, _)) = cue_image(&cue)
                    && std::fs::canonicalize(&image).ok() == std::fs::canonicalize(path).ok()
                {
                    return Ok((path.to_path_buf(), Some(cue)));
                }
            }
        }
        let len = std::fs::metadata(path)?.len();
        if len < MIN_DISC_IMAGE_BYTES {
            return Err(Error::io(format!("image is smaller than 400 MiB: {}", path.display())));
        }
        return Ok((path.to_path_buf(), None));
    }
    Err(Error::io(format!("not an extracted folder or a disc image: {}", path.display())))
}
