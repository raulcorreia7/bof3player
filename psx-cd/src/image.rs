//! Disc images: sector layout detection, the ISO9660 file table and file reads.
//!
//! An image is a cooked 2048-byte ISO, a raw 2352-byte Mode 1 or Mode 2 image,
//! or a CUE sheet that names its data track. The layout is detected from the
//! primary volume descriptor at sector 16.
//!
//! # Examples
//!
//! ```
//! use psx_cd::{Image, rebuild};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let input = dir.path().join("input");
//! std::fs::create_dir(&input).unwrap();
//! std::fs::write(input.join("HELLO.TXT"), b"hello").unwrap();
//! let iso = dir.path().join("disc.iso");
//! rebuild::iso(&input, &iso).unwrap();
//!
//! let mut image = Image::open(&iso).unwrap();
//! assert_eq!(image.entries().unwrap().len(), 1);
//! ```

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::cue;
use crate::error::{Error, Result};
use crate::iso9660::{DirectoryRecord, XaAttributes, normalized_name, parse_record};

/// User bytes in a cooked or Form 1 sector.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_cd::ISO_SECTOR_SIZE, 2048);
/// ```
pub const ISO_SECTOR_SIZE: u64 = 2048;
/// Bytes in a raw CD sector: sync, header, subheader, user data and checksums.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_cd::RAW_SECTOR_SIZE, 2352);
/// ```
pub const RAW_SECTOR_SIZE: u64 = 2352;
/// Sync pattern (12) plus sector header (4) before a Mode 2 sector's subheader.
const RAW_HEADER_SIZE: u64 = 16;
/// Raw sync, header and XA subheader (8) before a Mode 2 Form 1 sector's data.
const RAW_FORM1_DATA_OFFSET: u64 = 24;
/// The primary volume descriptor sector (ECMA-119 6.2).
const PVD_SECTOR: u64 = 16;
/// Root directory record inside the primary volume descriptor.
const PVD_ROOT_RECORD: usize = 156;
/// The raw sectors before the volume descriptors hold the license text.
const LICENSE_SECTORS: u64 = 12;
/// Bytes of an XA sector without sync and header: subheader, payload and EDC
/// (`psx-spx` "CDROM XA Subheader, File, Channel, Interleave").
///
/// # Examples
///
/// ```
/// assert_eq!(psx_cd::XA_SECTOR_SIZE, 2336);
/// ```
pub const XA_SECTOR_SIZE: usize = 2336;

/// A source of image bytes: a file, memory or any other random-access reader.
///
/// The trait is implemented for every `Read + Seek + Send` type.
///
/// # Examples
///
/// ```
/// let reader = std::io::Cursor::new(vec![0_u8; 2352]);
/// fn accepts(_: &dyn psx_cd::ReadSeek) {}
/// accepts(&reader);
/// ```
pub trait ReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send + ?Sized> ReadSeek for T {}

#[derive(Clone, Copy, Debug)]
struct SectorLayout {
    size: u64,
    data_offset: u64,
}

/// An open disc image: a cooked 2048-byte or raw 2352-byte image, reached
/// directly or through its CUE sheet.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> Result<(), psx_cd::Error> {
/// let mut image = psx_cd::Image::open("disc.cue")?;
/// let entries = image.entries()?;
/// assert!(!entries.is_empty());
/// # Ok(())
/// # }
/// ```
pub struct Image {
    file: Box<dyn ReadSeek>,
    file_len: u64,
    layout: SectorLayout,
    root: DirectoryRecord,
    cue_path: Option<PathBuf>,
}

/// One file or directory of the disc's file table.
///
/// # Examples
///
/// ```
/// let entry = psx_cd::Entry {
///     path: "DIR/FILE.BIN".into(),
///     lba: 30,
///     size: 4096,
///     is_directory: false,
///     extended_attributes: 0,
///     is_xa: false,
///     xa: None,
/// };
/// assert_eq!(entry.sectors(), 2);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Path below the image root, separated by `/`.
    pub path: PathBuf,
    /// First sector of the entry, as the directory record stores it.
    pub lba: u32,
    /// Size in bytes as the directory record stores it. A Form 2 file records
    /// `sectors * 2048` bytes although each sector holds 2336.
    pub size: u32,
    /// Whether the record marks a directory.
    pub is_directory: bool,
    /// Length of the extended attribute record before the data, in bytes.
    pub extended_attributes: u8,
    /// Whether the record marks an interleaved CD-XA stream.
    pub is_xa: bool,
    /// CD-XA attributes, when the record carries a system-use block.
    pub xa: Option<XaAttributes>,
}

impl Entry {
    /// Sectors the file occupies: its record size in 2048-byte units. A Form 2
    /// file records `sectors * 2048` bytes although each sector holds 2336.
    ///
    /// # Examples
    ///
    /// ```
    /// # let entry = psx_cd::Entry { path: "A.BIN".into(), lba: 20, size: 2049,
    /// #     is_directory: false, extended_attributes: 0, is_xa: false, xa: None };
    /// assert_eq!(entry.sectors(), 2);
    /// ```
    pub fn sectors(&self) -> u64 {
        u64::from(self.size).div_ceil(ISO_SECTOR_SIZE)
    }
}

impl Image {
    /// Opens an image path, or the data track a CUE sheet names.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let image = psx_cd::Image::open("disc.cue")?;
    /// assert!(image.is_raw());
    /// # Ok(())
    /// # }
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let cue_path = path
            .as_ref()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
            .then(|| path.as_ref().to_path_buf());
        let image_path = resolve_image_path(path.as_ref())?;
        let file = File::open(image_path)?;
        let file_len = file.metadata()?.len();
        Self::from_parts(Box::new(file), file_len, cue_path)
    }

    /// Opens a cooked or raw single-track image read from `reader`, which has no CUE sheet.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let bytes = std::fs::read("disc.iso")?;
    /// let image = psx_cd::Image::from_reader(std::io::Cursor::new(bytes))?;
    /// assert!(!image.is_raw());
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_reader(mut reader: impl ReadSeek + 'static) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        Self::from_parts(Box::new(reader), file_len, None)
    }

    fn from_parts(mut file: Box<dyn ReadSeek>, file_len: u64, cue_path: Option<PathBuf>) -> Result<Self> {
        let layout = detect_layout(&mut *file, file_len)?;
        let pvd = read_sector(&mut *file, file_len, layout, PVD_SECTOR)?;
        if !is_pvd(&pvd) {
            return Err(Error::InvalidImage("missing ISO9660 primary volume descriptor"));
        }
        let root =
            parse_record(&pvd[PVD_ROOT_RECORD..])?.ok_or(Error::InvalidImage("missing root directory record"))?;
        Ok(Self { file, file_len, layout, root, cue_path })
    }

    /// Writes the license sectors, every file and the audio tracks under
    /// `output`, and returns the written files in path order.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let written = psx_cd::Image::open("disc.cue")?.extract("extracted")?;
    /// assert!(!written.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn extract(mut self, output: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        fs::create_dir_all(output.as_ref())?;
        let mut extracted = Vec::new();
        if let Some(license) = self.license()? {
            let path = output.as_ref().join("license_data.dat");
            fs::write(&path, license)?;
            extracted.push(path);
        }
        let mut visited = HashSet::new();
        let root = self.root.clone();
        self.walk_directory(&root, output.as_ref(), &mut visited, &mut |image, parent, record, path| {
            ensure_beneath(parent, path)?;
            if record.is_directory() {
                fs::create_dir_all(path)?;
            } else {
                image.extract_file(record, path)?;
                extracted.push(path.to_path_buf());
            }
            Ok(())
        })?;
        if let Some(cue_path) = &self.cue_path {
            extracted.extend(cue::extract_audio_tracks(&cue::read(cue_path)?, output.as_ref())?);
        }
        extracted.sort();
        Ok(extracted)
    }

    /// The license sectors as 2336-byte XA sectors, or `None` on a cooked image.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let mut image = psx_cd::Image::open("disc.bin")?;
    /// assert_eq!(image.license()?.map(|bytes| bytes.len()), Some(12 * 2336));
    /// # Ok(())
    /// # }
    /// ```
    pub fn license(&mut self) -> Result<Option<Vec<u8>>> {
        if self.layout.size != RAW_SECTOR_SIZE {
            return Ok(None);
        }
        Ok(Some(self.read_xa_sectors(0, LICENSE_SECTORS)?))
    }

    /// The CUE sheet the image was opened through, if any.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let image = psx_cd::Image::open("disc.cue")?;
    /// assert!(image.cue_sheet()?.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn cue_sheet(&self) -> Result<Option<cue::Sheet>> {
        self.cue_path.as_deref().map(cue::read).transpose()
    }

    /// Whether the image stores raw 2352-byte sectors.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// assert!(psx_cd::Image::open("disc.bin")?.is_raw());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn is_raw(&self) -> bool {
        self.layout.size == RAW_SECTOR_SIZE
    }

    /// Reads up to `out.len()` bytes of `entry` at `offset`.
    ///
    /// A Form 2 file on a raw image reads as 2336-byte sectors; any other file
    /// reads its 2048-byte user data, after its extended attributes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// # let mut image = psx_cd::Image::open("disc.iso")?;
    /// # let entry = image.entries()?.remove(0);
    /// let mut header = [0_u8; 4];
    /// let read = image.read_entry_range(&entry, 0, &mut header)?;
    /// assert!(read <= 4);
    /// # Ok(())
    /// # }
    /// ```
    pub fn read_entry_range(&mut self, entry: &Entry, offset: u64, out: &mut [u8]) -> Result<usize> {
        if entry.is_directory || out.is_empty() {
            return Ok(0);
        }
        let xa = entry.is_xa && self.is_raw();
        let size = if xa { entry.sectors() * XA_SECTOR_SIZE as u64 } else { u64::from(entry.size) };
        if offset >= size {
            return Ok(0);
        }
        let want = out.len().min(usize::try_from(size - offset).unwrap_or(usize::MAX));
        let (capacity, payload, attributes) = if xa {
            (XA_SECTOR_SIZE, RAW_HEADER_SIZE as usize, 0)
        } else {
            (ISO_SECTOR_SIZE as usize, 0, usize::from(entry.extended_attributes))
        };
        let capacity = capacity - attributes;
        let mut done = 0;
        while done < want {
            let position = offset + done as u64;
            let sector = position / capacity as u64;
            let within = (position % capacity as u64) as usize;
            let chunk = (want - done).min(capacity - within);
            let sector = u32::try_from(sector).map_err(|_| Error::InvalidImage("extent overflows"))?;
            let lba = entry.lba.checked_add(sector).ok_or(Error::InvalidImage("extent overflows"))?;
            if payload > 0 {
                let at = u64::from(lba) * RAW_SECTOR_SIZE + (payload + attributes + within) as u64;
                self.file.seek(SeekFrom::Start(at))?;
                self.file.read_exact(&mut out[done..done + chunk])?;
            } else {
                let data = read_sector(&mut *self.file, self.file_len, self.layout, u64::from(lba))?;
                out[done..done + chunk].copy_from_slice(&data[attributes + within..attributes + within + chunk]);
            }
            done += chunk;
        }
        Ok(done)
    }

    /// Every file and directory of the file table, in path order.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let mut image = psx_cd::Image::open("disc.iso")?;
    /// let entries = image.entries()?;
    /// assert_eq!(entries.iter().filter(|entry| !entry.is_directory).count(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let mut visited = HashSet::new();
        let root = self.root.clone();
        self.walk_directory(&root, Path::new(""), &mut visited, &mut |_image, _parent, record, path| {
            entries.push(Entry {
                path: path.to_path_buf(),
                lba: record.extent,
                size: record.size,
                is_directory: record.is_directory(),
                extended_attributes: record.extended_attributes,
                is_xa: record.is_xa,
                xa: record.xa,
            });
            Ok(())
        })?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    /// The bytes of one file: 2048-byte sectors trimmed to the recorded size,
    /// or whole 2336-byte sectors for an XA file on a raw image.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), psx_cd::Error> {
    /// let mut image = psx_cd::Image::open("disc.iso")?;
    /// let entry = image.entries()?.into_iter().find(|entry| !entry.is_directory).unwrap();
    /// assert!(!image.read_file(&entry)?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn read_file(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        if entry.is_directory {
            return Err(Error::InvalidInput("a directory has no file bytes"));
        }
        let size =
            if entry.is_xa && self.is_raw() { entry.sectors() * XA_SECTOR_SIZE as u64 } else { u64::from(entry.size) };
        let mut bytes = vec![0; usize::try_from(size).map_err(|_| Error::InvalidImage("file is too large"))?];
        self.read_entry_range(entry, 0, &mut bytes)?;
        Ok(bytes)
    }

    fn walk_directory(
        &mut self,
        directory: &DirectoryRecord,
        parent: &Path,
        visited: &mut HashSet<(u32, u32)>,
        on_record: &mut impl FnMut(&mut Image, &Path, &DirectoryRecord, &Path) -> Result<()>,
    ) -> Result<()> {
        if !visited.insert((directory.extent, directory.size)) {
            return Ok(());
        }
        let bytes = self.read_extent(directory.extent, directory.size)?;
        let mut cursor = 0;
        while cursor < bytes.len() {
            let length = bytes[cursor] as usize;
            if length == 0 {
                // A record never straddles a sector: skip to the next one.
                cursor = ((cursor / ISO_SECTOR_SIZE as usize) + 1) * ISO_SECTOR_SIZE as usize;
                continue;
            }
            let end = cursor.checked_add(length).ok_or(Error::InvalidImage("directory record range overflows"))?;
            if end > bytes.len() {
                return Err(Error::InvalidImage("directory record is truncated"));
            }
            if let Some(record) = parse_record(&bytes[cursor..end])? {
                // Identifiers 0 and 1 are the directory itself and its parent.
                if record.name != [0] && record.name != [1] {
                    let path = parent.join(normalized_name(&record.name)?);
                    on_record(self, parent, &record, &path)?;
                    if record.is_directory() {
                        self.walk_directory(&record, &path, visited, on_record)?;
                    }
                }
            }
            cursor = end;
        }
        Ok(())
    }

    fn extract_file(&mut self, record: &DirectoryRecord, path: &Path) -> Result<()> {
        let entry = Entry {
            path: path.to_path_buf(),
            lba: record.extent,
            size: record.size,
            is_directory: false,
            extended_attributes: record.extended_attributes,
            is_xa: record.is_xa,
            xa: record.xa,
        };
        let bytes = self.read_file(&entry)?;
        let mut output = File::create(path)?;
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(())
    }

    /// `sectors` whole 2336-byte XA sectors starting at `extent`.
    fn read_xa_sectors(&mut self, extent: u32, sectors: u64) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(sectors as usize * XA_SECTOR_SIZE);
        for sector_index in 0..sectors {
            let sector = u64::from(extent).checked_add(sector_index).ok_or(Error::InvalidImage("extent overflows"))?;
            let offset = sector
                .checked_mul(RAW_SECTOR_SIZE)
                .and_then(|value| value.checked_add(RAW_HEADER_SIZE))
                .ok_or(Error::InvalidImage("XA sector offset overflows"))?;
            self.file.seek(SeekFrom::Start(offset))?;
            let mut sector_bytes = [0; XA_SECTOR_SIZE];
            self.file.read_exact(&mut sector_bytes)?;
            bytes.extend_from_slice(&sector_bytes);
        }
        Ok(bytes)
    }

    fn read_extent(&mut self, extent: u32, size: u32) -> Result<Vec<u8>> {
        let sectors = u64::from(size).div_ceil(ISO_SECTOR_SIZE);
        let mut bytes = Vec::with_capacity(size as usize);
        for sector_index in 0..sectors {
            let sector = u64::from(extent).checked_add(sector_index).ok_or(Error::InvalidImage("extent overflows"))?;
            bytes.extend_from_slice(&read_sector(&mut *self.file, self.file_len, self.layout, sector)?);
        }
        bytes.truncate(size as usize);
        Ok(bytes)
    }
}

/// The one disc image in `directory`: a lone `.cue`, else a lone `.iso`, else a
/// lone `.bin`. Several candidates of the first kind present are an error.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("disc.iso"), b"").unwrap();
/// let found = psx_cd::discover_image(dir.path()).unwrap();
/// assert!(found.ends_with("disc.iso"));
/// ```
pub fn discover_image(directory: &Path) -> Result<PathBuf> {
    if !directory.is_dir() {
        return Err(Error::NoImage(format!("directory not found: {}", directory.display())));
    }
    for (extension, label) in [("cue", "cue sheets"), ("iso", "ISO images"), ("bin", "BIN files")] {
        let mut matches = fs::read_dir(directory)?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().is_some_and(|value| value.eq_ignore_ascii_case(extension))
            })
            .collect::<Vec<_>>();
        matches.sort();
        match matches.as_slice() {
            [path] => return Ok(path.clone()),
            [] => {}
            _ => {
                return Err(Error::NoImage(format!(
                    "multiple {label} found in {}; choose one explicitly",
                    directory.display()
                )));
            }
        }
    }
    Err(Error::NoImage(format!("no supported input image found in {}", directory.display())))
}

fn is_pvd(sector: &[u8]) -> bool {
    sector[0] == 1 && &sector[1..6] == b"CD001" && sector[6] == 1
}

fn resolve_image_path(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        return Err(Error::InvalidImageDetail(format!("input not found: {}", path.display())));
    }
    if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cue")) {
        return Ok(path.to_path_buf());
    }
    let sheet = cue::read(path)?;
    let data_track = sheet
        .tracks
        .iter()
        .find(|track| track.mode != cue::TrackMode::Audio)
        .ok_or(Error::InvalidImage("cue sheet has no data track"))?;
    if data_track.index01 != 0 {
        return Err(Error::InvalidImage("data track with nonzero INDEX 01 is not supported yet"));
    }
    let resolved = data_track.file.clone();
    if !resolved.exists() {
        return Err(Error::InvalidImageDetail(format!("cue data file not found: {}", resolved.display())));
    }
    Ok(resolved)
}

fn detect_layout(file: &mut dyn ReadSeek, file_len: u64) -> Result<SectorLayout> {
    for layout in [
        SectorLayout { size: ISO_SECTOR_SIZE, data_offset: 0 },
        SectorLayout { size: RAW_SECTOR_SIZE, data_offset: RAW_FORM1_DATA_OFFSET },
        SectorLayout { size: RAW_SECTOR_SIZE, data_offset: RAW_HEADER_SIZE },
    ] {
        if let Ok(sector) = read_sector(file, file_len, layout, PVD_SECTOR)
            && is_pvd(&sector)
        {
            return Ok(layout);
        }
    }
    Err(Error::InvalidImage("unsupported sector layout or missing ISO9660 PVD"))
}

fn read_sector(
    file: &mut dyn ReadSeek,
    file_len: u64,
    layout: SectorLayout,
    sector: u64,
) -> Result<[u8; ISO_SECTOR_SIZE as usize]> {
    let offset = sector
        .checked_mul(layout.size)
        .and_then(|value| value.checked_add(layout.data_offset))
        .ok_or(Error::InvalidImage("sector offset overflows"))?;
    let end = offset.checked_add(ISO_SECTOR_SIZE).ok_or(Error::InvalidImage("sector range overflows"))?;
    if end > file_len {
        return Err(Error::InvalidImage("sector extends beyond image"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; ISO_SECTOR_SIZE as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn ensure_beneath(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) {
        return Err(Error::InvalidImage("output path escapes extraction root"));
    }
    Ok(())
}

pub(crate) fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
