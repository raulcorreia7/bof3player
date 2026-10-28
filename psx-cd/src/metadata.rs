//! Project XML and `disc_lba.json`, the two metadata files an extraction writes.
//!
//! The project XML is the flat `mkpsxiso` project shape: one data track, an
//! optional license file and a list of files. `disc_lba.json` records every
//! file's LBA, size and sector length for tools that seek directly.
//!
//! # Examples
//!
//! ```
//! let entries = vec![psx_cd::Entry {
//!     path: "HELLO.TXT".into(),
//!     lba: 21,
//!     size: 5,
//!     is_directory: false,
//!     extended_attributes: 0,
//!     is_xa: false,
//!     xa: None,
//! }];
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path().join("disc.xml");
//! psx_cd::metadata::write_project(&path, &entries, "disc.iso").unwrap();
//! assert!(std::fs::read_to_string(path).unwrap().contains("image_name=\"disc.iso\""));
//! ```

use std::fs;
use std::path::Path;

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use serde::Serialize;

use crate::Entry;
use crate::cue::format_timecode;
use crate::error::{Error, Result};

#[derive(Serialize)]
struct LbaDocument {
    entries: Vec<LbaRow>,
}

// NOTE: nlohmann::json emits keys in lexical order; keep this order for byte parity.
#[derive(Serialize)]
struct LbaRow {
    archive_name: String,
    archive_type: String,
    bytes: u32,
    family: String,
    lba: u32,
    length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_path: Option<String>,
    name: String,
    row_type: &'static str,
    source_path: String,
    timecode: String,
}

/// Names that identify an archive container and its manifest in `disc_lba.json`.
///
/// # Examples
///
/// ```
/// let naming = psx_cd::metadata::ArchiveNaming { extension: Some("EMI"), manifest: "emi.json" };
/// assert_eq!(naming.manifest, "emi.json");
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveNaming<'a> {
    /// Extension that marks a container, without the dot; `None` maps no file to a manifest.
    pub extension: Option<&'a str>,
    /// Manifest file name inside the container's directory.
    pub manifest: &'a str,
}

impl Default for ArchiveNaming<'static> {
    fn default() -> Self {
        Self { extension: None, manifest: "manifest.json" }
    }
}

/// Writes a project XML for `entries` to `path`; `image_name` names the image
/// the project builds.
///
/// # Examples
///
/// ```
/// let entries = vec![psx_cd::Entry {
///     path: "HELLO.TXT".into(),
///     lba: 21,
///     size: 5,
///     is_directory: false,
///     extended_attributes: 0,
///     is_xa: false,
///     xa: None,
/// }];
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path().join("disc.xml");
/// psx_cd::metadata::write_project(&path, &entries, "disc.iso").unwrap();
/// assert!(std::fs::read_to_string(path).unwrap().contains("source=\"HELLO.TXT\""));
/// ```
pub fn write_project(path: &Path, entries: &[Entry], image_name: &str) -> Result<(), Error> {
    crate::ensure_parent(path)?;
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 4);
    let mut project = BytesStart::new("iso_project");
    project.push_attribute(("image_name", image_name));
    project.push_attribute(("cue_sheet", "mkpsxiso.cue"));
    write_xml(&mut writer, Event::Start(project))?;
    let mut track = BytesStart::new("track");
    track.push_attribute(("type", "data"));
    track.push_attribute(("xa_edc", "false"));
    track.push_attribute(("new_type", "false"));
    write_xml(&mut writer, Event::Start(track))?;
    let mut license = BytesStart::new("license");
    license.push_attribute(("file", "license_data.dat"));
    write_xml(&mut writer, Event::Empty(license))?;
    write_xml(&mut writer, Event::Start(BytesStart::new("directory_tree")))?;
    for entry in entries.iter().filter(|entry| !entry.is_directory) {
        let source = entry.path.to_string_lossy().replace('\\', "/");
        let mut file = BytesStart::new("file");
        file.push_attribute(("name", entry.path.file_name().and_then(|value| value.to_str()).unwrap_or("")));
        file.push_attribute(("source", source.as_str()));
        file.push_attribute(("type", "data"));
        write_xml(&mut writer, Event::Empty(file))?;
    }
    for name in ["directory_tree", "track", "iso_project"] {
        write_xml(&mut writer, Event::End(BytesEnd::new(name)))?;
    }
    let mut xml = writer.into_inner();
    xml.push(b'\n');
    fs::write(path, xml)?;
    Ok(())
}

/// Writes `disc_lba.json` with caller-named archive manifests.
///
/// A file whose extension matches [`ArchiveNaming::extension`] gains a `manifest_path` in
/// [`ArchiveNaming::manifest`] under its directory. [`ArchiveNaming::default`] maps no archives.
///
/// # Examples
///
/// ```
/// let entries = vec![psx_cd::Entry {
///     path: "HELLO.TXT".into(),
///     lba: 21,
///     size: 5,
///     is_directory: false,
///     extended_attributes: 0,
///     is_xa: false,
///     xa: None,
/// }];
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path().join("disc_lba.json");
/// psx_cd::metadata::write_lba_json_with(
///     &path,
///     &entries,
///     std::path::Path::new("raw"),
///     std::path::Path::new("extract"),
///     psx_cd::metadata::ArchiveNaming::default(),
/// )
/// .unwrap();
/// assert!(std::fs::read_to_string(path).unwrap().contains("\"lba\": 21"));
/// ```
pub fn write_lba_json_with(
    path: &Path,
    entries: &[Entry],
    raw_root: &Path,
    extracted_root: &Path,
    naming: ArchiveNaming<'_>,
) -> Result<(), Error> {
    crate::ensure_parent(path)?;
    let mut files = entries.iter().filter(|entry| !entry.is_directory).collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.lba);

    let rows = files
        .into_iter()
        .map(|entry| {
            let source_path = extracted_root.join(&entry.path).to_string_lossy().replace('\\', "/");
            let extension = entry.path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_uppercase();
            let archive_name = entry.path.file_stem().and_then(|value| value.to_str()).unwrap_or("");
            let sector_size = if entry.is_xa { 2_336 } else { 2_048 };
            LbaRow {
                archive_name: archive_name.to_owned(),
                archive_type: extension.clone(),
                bytes: entry.size,
                family: family(&entry.path),
                lba: entry.lba,
                length: u64::from(entry.size).div_ceil(sector_size),
                manifest_path: naming
                    .extension
                    .filter(|archive_extension| extension.eq_ignore_ascii_case(archive_extension))
                    .map(|_| manifest_path(raw_root, extracted_root, &entry.path, naming.manifest)),
                name: entry.path.file_name().and_then(|value| value.to_str()).unwrap_or("").to_owned(),
                row_type: if entry.is_xa { "XA" } else { "File" },
                source_path,
                timecode: format_timecode(150 + entry.lba),
            }
        })
        .collect();
    let mut json = serde_json::to_string_pretty(&LbaDocument { entries: rows })
        .map_err(|error| Error::InvalidImageDetail(error.to_string()))?;
    json.push('\n');
    fs::write(path, json)?;
    Ok(())
}

fn manifest_path(raw_root: &Path, extracted_root: &Path, entry_path: &Path, manifest_name: &str) -> String {
    let has_bin_component = entry_path
        .components()
        .any(|part| matches!(part, std::path::Component::Normal(value) if value.eq_ignore_ascii_case("BIN")));
    let mut manifest = if has_bin_component { raw_root.join(entry_path) } else { extracted_root.join(entry_path) };
    manifest.set_extension("");
    manifest.join(manifest_name).to_string_lossy().replace('\\', "/")
}

fn family(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    parts
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case("BIN"))
        .map_or_else(|| "unknown".to_owned(), |pair| pair[1].to_owned())
}

fn write_xml(writer: &mut Writer<Vec<u8>>, event: Event<'_>) -> Result<(), Error> {
    writer.write_event(event).map_err(|error| Error::InvalidImageDetail(format!("cannot encode project XML: {error}")))
}
