//! MD5 and SHA-256 manifests of an extracted tree.
//!
//! A manifest records the image files (`bin`, `cue`, `img` and `iso`, matched
//! case-insensitively) below a root by their path, size and two digests. Equal
//! content is recorded once, under the lexically first path.
//!
//! # Examples
//!
//! ```
//! let dir = tempfile::tempdir().unwrap();
//! std::fs::write(dir.path().join("disc.bin"), b"abc").unwrap();
//! let records = psx_cd::checksum::scan(dir.path(), dir.path()).unwrap();
//! assert_eq!(records[0].path, "disc.bin");
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use md5::{Digest as _, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{Error, Result};

/// One recorded file: its path, size and both digests.
///
/// # Examples
///
/// ```
/// let record = psx_cd::checksum::Record {
///     path: "disc.bin".into(),
///     size: 3,
///     md5: "900150983cd24fb0d6963f7d28e17f72".into(),
///     sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
/// };
/// assert_eq!(record.size, 3);
/// ```
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Record {
    /// Path relative to the current directory, with `/` separators.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Lowercase hex MD5 of the file.
    pub md5: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
}

/// The JSON document a manifest file holds.
#[derive(Deserialize, Serialize)]
struct Manifest {
    files: Vec<Record>,
}

/// Scans the image files below `root`, recording paths relative to `current_dir`.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::create_dir(dir.path().join("sub")).unwrap();
/// std::fs::write(dir.path().join("sub/disc.iso"), b"abc").unwrap();
/// let records = psx_cd::checksum::scan(dir.path(), dir.path()).unwrap();
/// assert_eq!(records[0].path, "sub/disc.iso");
/// ```
pub fn scan(root: &Path, current_dir: &Path) -> Result<Vec<Record>, Error> {
    if !root.is_dir() {
        return Err(Error::InvalidImageDetail(format!("disk directory not found: {}", root.display())));
    }
    let mut paths = Vec::new();
    collect(root, &mut paths)?;
    let mut unique = BTreeMap::new();
    for path in paths {
        let size = path.metadata()?.len();
        let (md5, sha256) = digest(&path)?;
        let display = path.strip_prefix(current_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let record = Record { path: display, size, md5, sha256 };
        unique
            .entry((record.size, record.md5.clone(), record.sha256.clone()))
            .and_modify(|old: &mut Record| {
                if record.path < old.path {
                    *old = record.clone();
                }
            })
            .or_insert(record);
    }
    Ok(unique.into_values().collect())
}

/// Writes `records` as the manifest JSON at `path`.
///
/// # Examples
///
/// ```
/// # let dir = tempfile::tempdir().unwrap();
/// # std::fs::write(dir.path().join("disc.bin"), b"abc").unwrap();
/// # let records = psx_cd::checksum::scan(dir.path(), dir.path()).unwrap();
/// let manifest = dir.path().join("checksums.json");
/// psx_cd::checksum::write(&manifest, &records).unwrap();
/// assert!(std::fs::read_to_string(manifest).unwrap().contains("disc.bin"));
/// ```
pub fn write(path: &Path, records: &[Record]) -> Result<(), Error> {
    crate::ensure_parent(path)?;
    let mut bytes = serde_json::to_vec_pretty(&Manifest { files: records.to_vec() })
        .map_err(|error| Error::InvalidImageDetail(format!("cannot encode checksums JSON: {error}")))?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;
    Ok(())
}

/// Reads a manifest written by [`write()`].
///
/// # Examples
///
/// ```
/// # let dir = tempfile::tempdir().unwrap();
/// # std::fs::write(dir.path().join("disc.bin"), b"abc").unwrap();
/// # let records = psx_cd::checksum::scan(dir.path(), dir.path()).unwrap();
/// # let manifest = dir.path().join("checksums.json");
/// # psx_cd::checksum::write(&manifest, &records).unwrap();
/// assert_eq!(psx_cd::checksum::read(&manifest).unwrap(), records);
/// ```
pub fn read(path: &Path) -> Result<Vec<Record>, Error> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| Error::InvalidImageDetail(format!("invalid checksums JSON: {error}")))?;
    for record in &manifest.files {
        if record.path.is_empty() || record.md5.is_empty() || record.sha256.is_empty() {
            return Err(Error::InvalidImage("checksum entry has an empty path or digest"));
        }
    }
    Ok(manifest.files)
}

/// Compares a fresh scan of `root` with `expected`; returns one message per
/// difference, and an empty vector when every file matches.
///
/// # Examples
///
/// ```
/// # let dir = tempfile::tempdir().unwrap();
/// # std::fs::write(dir.path().join("disc.bin"), b"abc").unwrap();
/// # let expected = psx_cd::checksum::scan(dir.path(), dir.path()).unwrap();
/// assert!(psx_cd::checksum::verify(dir.path(), dir.path(), &expected).unwrap().is_empty());
/// std::fs::write(dir.path().join("disc.bin"), b"abcd").unwrap();
/// assert_eq!(psx_cd::checksum::verify(dir.path(), dir.path(), &expected).unwrap().len(), 1);
/// ```
pub fn verify(root: &Path, current_dir: &Path, expected: &[Record]) -> Result<Vec<String>, Error> {
    let actual = scan(root, current_dir)?;
    let actual_by_path: BTreeMap<_, _> = actual.iter().map(|row| (&row.path, row)).collect();
    let expected_by_path: BTreeMap<_, _> = expected.iter().map(|row| (&row.path, row)).collect();
    let paths: BTreeSet<_> = actual_by_path.keys().chain(expected_by_path.keys()).collect();
    let mut errors = Vec::new();
    for path in paths {
        match (actual_by_path.get(path), expected_by_path.get(path)) {
            (Some(_), None) => errors.push(format!("unexpected file: {path}")),
            (None, Some(_)) => errors.push(format!("missing file: {path}")),
            (Some(actual), Some(expected)) if actual.size != expected.size => {
                errors.push(format!("size mismatch: {path}"));
            }
            (Some(actual), Some(expected)) => {
                if actual.md5 != expected.md5 {
                    errors.push(format!("md5 mismatch: {path}"));
                }
                if actual.sha256 != expected.sha256 {
                    errors.push(format!("sha256 mismatch: {path}"));
                }
            }
            (None, None) => {}
        }
    }
    Ok(errors)
}

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, paths)?;
        } else if matches!(
            path.extension().and_then(|value| value.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("bin" | "cue" | "img" | "iso")
        ) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(())
}

fn digest(path: &Path) -> Result<(String, String), Error> {
    let mut file = File::open(path)?;
    let mut md5 = Md5::new();
    let mut sha256 = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        md5.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
    }
    Ok((hex(&md5.finalize()), hex(&sha256.finalize())))
}

/// The lowercase hex SHA-256 of `bytes`.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     psx_cd::checksum::sha256_hex(b"abc"),
///     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
/// );
/// ```
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}
