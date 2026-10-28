// SPDX-License-Identifier: MIT
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{Blob, Source};
use crate::error::{Error, Result};

/// An extracted tree on disk (`<root>/ASSETS/DATA.BIN`, ...).
///
/// Path lookups ignore ASCII case, so `assets/data.bin` finds `ASSETS/DATA.BIN`.
///
/// # Examples
///
/// ```
/// use psx_audio::{FolderSource, Source};
///
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::create_dir(dir.path().join("ASSETS")).unwrap();
/// std::fs::write(dir.path().join("ASSETS/DATA.BIN"), b"hi").unwrap();
/// let source = FolderSource::open(dir.path()).unwrap();
/// assert_eq!(source.list().unwrap(), ["ASSETS/DATA.BIN"]);
/// ```
#[derive(Debug, Clone)]
pub struct FolderSource {
    root: PathBuf,
}

impl FolderSource {
    /// Use `root` as the volume root.
    ///
    /// # Errors
    ///
    /// Fails when `root` is not a directory.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = tempfile::tempdir().unwrap();
    /// assert!(psx_audio::FolderSource::open(dir.path()).is_ok());
    /// assert!(psx_audio::FolderSource::open(dir.path().join("missing")).is_err());
    /// ```
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(Error::io(format!("not a folder: {}", root.display())));
        }
        Ok(Self { root: root.to_path_buf() })
    }

    /// Resolve a volume path to a real path, matching each component without case.
    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let mut out = self.root.clone();
        for component in path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".") {
            if component == ".." {
                return Err(Error::invalid("asset path escapes the data root"));
            }
            let direct = out.join(component);
            if direct.exists() {
                out = direct;
                continue;
            }
            let found = std::fs::read_dir(&out)?
                .filter_map(std::result::Result::ok)
                .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(component));
            match found {
                Some(entry) => out = entry.path(),
                None => return Err(Error::io(format!("no such file: {path}"))),
            }
        }
        Ok(out)
    }
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, out)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            out.push(relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct FileBlob {
    file: Mutex<File>,
    len: u64,
}

impl Blob for FileBlob {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let mut file = self.file.lock().map_err(|_| Error::State("file lock poisoned".into()))?;
        file.seek(SeekFrom::Start(offset))?;
        let want = buf.len().min((self.len - offset) as usize);
        Ok(file.read(&mut buf[..want])?)
    }
}

/// Open a file as a blob (shared with the disc source's audio track).
pub(super) fn file_blob(path: &Path) -> Result<Arc<dyn Blob>> {
    let file = File::open(path).map_err(|e| Error::io(format!("cannot open {}: {e}", path.display())))?;
    let len = file.metadata()?.len();
    Ok(Arc::new(FileBlob { file: Mutex::new(file), len }))
}

impl Source for FolderSource {
    fn list(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        walk(&self.root, &self.root, &mut out)
            .map_err(|e| Error::io(format!("cannot list {}: {e}", self.root.display())))?;
        Ok(out)
    }

    fn open<'a>(&'a self, path: &str) -> Result<Arc<dyn Blob + 'a>> {
        let real = self.resolve(path)?;
        file_blob(&real)
    }

    fn describe(&self) -> String {
        format!("folder {}", self.root.display())
    }
}
