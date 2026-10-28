// SPDX-License-Identifier: MIT
//! Saving items to files: rendered WAV, FLAC and Ogg Vorbis exports that advance a slice per UI
//! tick so a browser never blocks, and the native MIDI, VAG and VAB packages.

use std::path::{Path, PathBuf};

use crate::catalog::{Item, Kind};
use crate::player::Loop;
use crate::{Library, Player, PlayerOptions};

/// A rendered-audio file format the browser can write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportFormat {
    /// A 16-bit PCM WAVE file.
    #[default]
    Wav,
    /// A lossless FLAC file with the item title in its comment block.
    Flac,
    /// A lossy Ogg Vorbis file with the item title in its comment header.
    Ogg,
    /// Raw signed 16-bit little-endian PCM, with no header.
    Raw,
}

impl ExportFormat {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Raw => "s16",
        }
    }
}

/// The Ogg Vorbis quality choices the browser offers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OggQuality {
    /// Vorbis `-q 2`, about 96 kbps at 44.1 kHz stereo.
    Low,
    /// Vorbis `-q 6`, about 192 kbps at 44.1 kHz stereo.
    #[default]
    Normal,
    /// Vorbis `-q 9`, about 320 kbps at 44.1 kHz stereo.
    High,
}

impl OggQuality {
    /// The Vorbis `-q` value.
    pub fn quality(self) -> f32 {
        match self {
            Self::Low => 2.0,
            Self::Normal => crate::export::DEFAULT_OGG_QUALITY,
            Self::High => 9.0,
        }
    }

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low (96 kbps)",
            Self::Normal => "normal (192 kbps)",
            Self::High => "high (320 kbps)",
        }
    }

    /// The next choice, wrapping.
    pub fn next(self, delta: i32) -> Self {
        let choices = [Self::Low, Self::Normal, Self::High];
        let at = choices.iter().position(|choice| *choice == self).unwrap_or(1) as i32;
        choices[(at + delta).rem_euclid(choices.len() as i32) as usize]
    }
}

/// Characters a file name cannot carry on Linux, macOS or Windows.
const UNSAFE: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
/// Windows device names, reserved whatever the extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A title with the characters no file system accepts removed and the trailing
/// dots and spaces Windows rejects trimmed.
fn sanitize(title: &str) -> String {
    let name: String = title.chars().filter(|c| !c.is_control() && !UNSAFE.contains(c)).collect();
    name.trim_matches(' ').trim_end_matches(['.', ' ']).to_string()
}

/// Whether Windows would read the name as a device.
fn reserved(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    RESERVED.iter().any(|reserved| base.eq_ignore_ascii_case(reserved))
}

/// A catalog id as a file stem: letters, digits and `-` stay, anything else becomes `_`.
pub fn file_stem(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// The file stem of an export: the song title when it has one, else the id.
fn export_stem(item: &Item) -> String {
    let title = sanitize(&item.title);
    if title.is_empty() || reserved(&title) { file_stem(&item.id) } else { title }
}

/// A path in `dir` for `item`, numbered when an earlier export took the name.
pub fn export_path(dir: &Path, item: &Item, extension: &str) -> PathBuf {
    let stem = export_stem(item);
    let mut path = dir.join(format!("{stem}.{extension}"));
    let mut number = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({number}).{extension}"));
        number += 1;
    }
    path
}

/// Frames rendered per read from the player.
pub const CHUNK_FRAMES: usize = 8192;
/// Frames rendered per UI tick.
pub const FRAMES_PER_TICK: usize = 65_536;

/// One tick of an export.
pub enum Step {
    /// More audio remains.
    Progress,
    /// The WAV file is written.
    Done,
    /// The export stopped; the message says why.
    Failed(String),
}

/// How a listening-length export renders a looping track.
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    /// Plays of the loop window, counting the first; `None` uses the format's default.
    pub loops: Option<u32>,
    /// Seconds of linear fade-out at the end.
    pub fade: f64,
    /// The file format to write.
    pub format: ExportFormat,
    /// The Ogg Vorbis quality, 0 (smallest) to 10 (best); ignored by the other formats.
    pub quality: f32,
    /// Stop after this many seconds of audio; `None` renders the whole item.
    pub seconds: Option<f64>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            loops: None,
            fade: 10.0,
            format: ExportFormat::Wav,
            quality: crate::export::DEFAULT_OGG_QUALITY,
            seconds: None,
        }
    }
}

/// One running audio export.
pub struct Save {
    file: PathBuf,
    id: String,
    title: String,
    artist: String,
    album: String,
    track: Option<u32>,
    loop_start: Option<u64>,
    loop_length: Option<u64>,
    stop_at: Option<usize>,
    format: ExportFormat,
    quality: f32,
    player: Player<'static>,
    pcm: Vec<i16>,
    chunk: Vec<i16>,
    channels: usize,
    rate: u32,
    duration: f64,
    fade: f64,
}

impl Save {
    /// Start exporting `item` into `dir` as `export.format`. Music plays
    /// `loops` passes of its loop window, or of the whole item when it has no
    /// window, then the `fade` seconds at the end fade out. Other kinds
    /// render once, without fade.
    pub fn start(
        library: &'static Library<'static>,
        item: &Item,
        player_options: PlayerOptions,
        dir: &Path,
        export: ExportOptions,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Self::start_to(library, item, player_options, export_path(dir, item, export.format.extension()), export)
    }

    /// Start exporting `item` into the file `file`; the options act as in [`Save::start`]. The
    /// folder of `file` must exist.
    pub fn start_to(
        library: &'static Library<'static>,
        item: &Item,
        player_options: PlayerOptions,
        file: PathBuf,
        export: ExportOptions,
    ) -> Result<Self, String> {
        if !item.playable {
            return Err(format!("{} carries no audio: {}", item.id, item.detail));
        }
        let mut player = library.player_with(item, player_options).map_err(|e| e.to_string())?;
        let music = item.kind == crate::catalog::Kind::Music;
        let window = player.loop_window().filter(|_| music);
        let requested = export.loops.unwrap_or(1).max(1);
        let intro_loop =
            matches!(export.format, ExportFormat::Flac | ExportFormat::Ogg) && window.is_some() && requested == 1;
        let extra = if intro_loop { 0 } else { requested - 1 };
        if music {
            player.set_loop(Loop::Times(extra));
        }
        let channels = player.channels() as usize;
        let rate = player.sample_rate();
        let loop_seconds = window.map_or(player.duration(), |(begin, end)| (end - begin).max(0.0));
        let intro_end = window.filter(|_| intro_loop).map(|(_, end)| end);
        let duration = if let Some(end) = intro_end {
            end
        } else {
            player.duration() + if music { f64::from(extra) * loop_seconds } else { 0.0 }
        };
        let fade = if window.is_some() && !intro_loop { export.fade.max(0.0) } else { 0.0 };
        let cap = export.seconds.map(|seconds| (seconds.max(0.0) * f64::from(rate)).round() as usize);
        let stop_at = match (intro_end.map(|end| (end * f64::from(rate)).round() as usize), cap) {
            (Some(end), Some(cap)) => Some(end.min(cap)),
            (end, cap) => end.or(cap),
        };
        let duration = export.seconds.map_or(duration, |seconds| duration.min(seconds.max(0.0)));
        let title = if item.title.is_empty() { item.name.clone() } else { item.title.clone() };
        let (loop_start, loop_length) = window.map_or((None, None), |(begin, end)| {
            (Some((begin * f64::from(rate)).round() as u64), Some(((end - begin) * f64::from(rate)).round() as u64))
        });
        Ok(Self {
            file,
            id: item.id.clone(),
            title,
            artist: if music { "Breath of Fire III".to_string() } else { String::new() },
            album: if music { "Breath of Fire III".to_string() } else { String::new() },
            track: music.then_some(item.index),
            loop_start,
            loop_length,
            stop_at,
            format: export.format,
            quality: export.quality,
            player,
            pcm: Vec::new(),
            chunk: vec![0i16; CHUNK_FRAMES * channels],
            channels,
            rate,
            duration,
            fade,
        })
    }

    /// Render one tick's worth of audio.
    pub fn step(&mut self) -> Step {
        let mut rendered = 0;
        while rendered < FRAMES_PER_TICK {
            let want = (FRAMES_PER_TICK - rendered).min(CHUNK_FRAMES);
            let frames = match self.player.render_i16(&mut self.chunk[..want * self.channels]) {
                Ok(frames) => frames,
                Err(error) => return Step::Failed(error.to_string()),
            };
            if frames == 0 {
                if self.pcm.is_empty() {
                    return Step::Failed("nothing was rendered".to_string());
                }
                return self.finish();
            }
            self.pcm.extend_from_slice(&self.chunk[..frames * self.channels]);
            rendered += frames;
            if let Some(stop) = self.stop_at
                && self.pcm.len() / self.channels >= stop
            {
                self.pcm.truncate(stop * self.channels);
                return self.finish();
            }
        }
        Step::Progress
    }

    fn finish(&mut self) -> Step {
        apply_fade(&mut self.pcm, self.channels, self.rate, self.fade);
        let channels = self.channels as u32;
        let tags = crate::export::Tags {
            title: &self.title,
            artist: &self.artist,
            album: &self.album,
            track: self.track,
            loop_start: self.loop_start,
            loop_length: self.loop_length,
        };
        let bytes = match self.format {
            ExportFormat::Wav => crate::export::wav_bytes(&self.pcm, self.rate, channels),
            ExportFormat::Flac => crate::export::flac_bytes(&self.pcm, self.rate, channels, &tags),
            ExportFormat::Ogg => crate::export::ogg_bytes(&self.pcm, self.rate, channels, self.quality, &tags),
            ExportFormat::Raw => Ok(crate::export::raw_s16le(&self.pcm)),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => return Step::Failed(error.to_string()),
        };
        if let Err(error) = std::fs::write(&self.file, &bytes) {
            return Step::Failed(format!("{}: {error}", self.file.display()));
        }
        Step::Done
    }

    /// The catalog id of the item being exported.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The file the export writes.
    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Seconds rendered so far.
    pub fn rendered(&self) -> f64 {
        self.pcm.len() as f64 / self.channels as f64 / f64::from(self.rate)
    }

    /// The sample rate of the rendered audio, in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// The channel count of the rendered audio.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Seconds the export will render.
    pub fn duration(&self) -> f64 {
        self.duration
    }
}

/// A linear fade-out over the last `seconds` of interleaved `pcm`.
pub fn apply_fade(pcm: &mut [i16], channels: usize, rate: u32, seconds: f64) {
    if channels == 0 || seconds <= 0.0 {
        return;
    }
    let frames = pcm.len() / channels;
    let fade_frames = ((seconds * f64::from(rate)) as usize).min(frames);
    if fade_frames == 0 {
        return;
    }
    let start = frames - fade_frames;
    for frame in 0..fade_frames {
        let gain = (fade_frames - frame) as f32 / fade_frames as f32;
        for channel in 0..channels {
            let at = (start + frame) * channels + channel;
            pcm[at] = (f32::from(pcm[at]) * gain) as i16;
        }
    }
}

/// A path in the form every "wrote" message prints.
pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The bytes and file extension of an item's native package: MIDI for music, VAG for a sample,
/// VAB for a bank, or with `sequence` the game's own sequence package of a music item.
pub fn native_package(
    library: &Library<'static>,
    item: &Item,
    sequence: bool,
) -> Result<(Vec<u8>, &'static str), String> {
    let (result, extension) = match (item.kind, sequence) {
        (Kind::Music, true) => (library.export_sequence(item), "seq"),
        (_, true) => return Err("only music items have a sequence package".to_string()),
        (Kind::Music, false) => (library.export_midi(item), "mid"),
        (Kind::Sample, false) => (library.export_vag(item), "vag"),
        (Kind::Bank, false) => (library.export_bank(item), "vab"),
        (Kind::Xa | Kind::Cdda, false) => {
            return Err(format!("{}: no native package; export WAV, FLAC or Ogg", item.id));
        }
    };
    result.map(|bytes| (bytes, extension)).map_err(|error| error.to_string())
}

/// Write the native package of `item` into `dir` (see [`native_package`]) and return the status
/// line for the result: where it was saved, or what failed and the next step.
pub fn export_native(library: &Library<'static>, item: &Item, dir: &Path, sequence: bool) -> String {
    let (bytes, extension) = match native_package(library, item, sequence) {
        Ok(package) => package,
        Err(error) => return error,
    };
    let path = export_path(dir, item, extension);
    match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, &bytes)) {
        Ok(()) => saved_message(&path),
        Err(error) => export_failed(&path, &error.to_string()),
    }
}

/// The folder exports go into by default: the user's Music folder below `Breath of Fire III`,
/// else `./exports` when no home is known.
pub fn default_export_dir() -> PathBuf {
    let folder = Path::new("Breath of Fire III");
    #[cfg(target_os = "linux")]
    if let Some(music) = std::env::var_os("XDG_MUSIC_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(music).join(folder);
    }
    if let Some(home) = home_dir() {
        return home.join("Music").join(folder);
    }
    PathBuf::from("exports")
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// One file format the export menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A rendered 16-bit PCM WAVE file.
    Wav,
    /// A lossless FLAC file.
    Flac,
    /// A lossy Ogg Vorbis file.
    Ogg,
    /// A Standard MIDI File of a music sequence.
    Midi,
    /// The game's own sequence package.
    Sequence,
    /// The raw ADPCM blocks of a sample.
    Vag,
    /// The header and body of a bank as one VAB file.
    Vab,
}

impl Format {
    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Flac => "FLAC (.flac)",
            Self::Ogg => "Ogg Vorbis (.ogg)",
            Self::Midi => "MIDI",
            Self::Sequence => "Sequence (.seq)",
            Self::Vag => "VAG (.vag)",
            Self::Vab => "VAB (.vab)",
        }
    }

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Midi => "mid",
            Self::Sequence => "seq",
            Self::Vag => "vag",
            Self::Vab => "vab",
        }
    }

    /// The rendered-audio format this menu row writes, if it renders audio.
    pub fn export_format(self) -> Option<ExportFormat> {
        match self {
            Self::Wav => Some(ExportFormat::Wav),
            Self::Flac => Some(ExportFormat::Flac),
            Self::Ogg => Some(ExportFormat::Ogg),
            Self::Midi | Self::Sequence | Self::Vag | Self::Vab => None,
        }
    }

    /// The formats the export menu offers for an item kind, in menu order.
    pub fn for_kind(kind: Kind) -> &'static [Format] {
        const MUSIC: &[Format] = &[Format::Wav, Format::Flac, Format::Ogg, Format::Midi, Format::Sequence];
        const SAMPLE: &[Format] = &[Format::Wav, Format::Flac, Format::Ogg, Format::Vag];
        const BANK: &[Format] = &[Format::Vab];
        const STREAM: &[Format] = &[Format::Wav, Format::Flac, Format::Ogg];
        match kind {
            Kind::Music => MUSIC,
            Kind::Sample => SAMPLE,
            Kind::Bank => BANK,
            Kind::Xa | Kind::Cdda => STREAM,
        }
    }
}

/// The status line for a finished export.
pub fn saved_message(path: &Path) -> String {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    let folder = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    format!("Saved {name} to {}", absolute(&folder).display())
}

/// The status line for a failed export: what failed, then the next step.
pub fn export_failed(path: &Path, error: &str) -> String {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    format!("Could not save {name}: {error}. Choose another format or folder (--export-dir).")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemorySource, NoNaming};

    fn item(id: &str, title: &str) -> Item {
        let kind = Kind::from_name(id.split(':').next().unwrap()).unwrap();
        Item { id: id.to_string(), title: title.to_string(), ..Item::new(kind) }
    }

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bof3player-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_stems_keep_letters_digits_and_dashes() {
        assert_eq!(file_stem("music:BGM000#0"), "music_BGM000_0");
        assert_eq!(file_stem("xa:S_XA00#ch0@12"), "xa_S_XA00_ch0_12");
    }

    #[test]
    fn export_names_use_the_title_and_number_repeats() {
        let dir = scratch("export-names");
        let titled = item("music:A#0", "Opening: Part 1?");
        let first = export_path(&dir, &titled, "wav");
        assert_eq!(first.file_name().unwrap(), "Opening Part 1.wav");
        std::fs::write(&first, b"").unwrap();
        assert_eq!(export_path(&dir, &titled, "wav").file_name().unwrap(), "Opening Part 1 (2).wav");
        // No title, or a Windows device name, falls back to the id.
        assert_eq!(export_path(&dir, &item("music:A#1", ""), "flac").file_name().unwrap(), "music_A_1.flac");
        assert_eq!(export_path(&dir, &item("music:A#2", "CON"), "flac").file_name().unwrap(), "music_A_2.flac");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_fade_ends_in_silence_and_leaves_the_start_alone() {
        let mut pcm = vec![1000i16; 20];
        apply_fade(&mut pcm, 2, 10, 0.5);
        assert_eq!(pcm[..10], [1000; 10]);
        assert!(pcm[12] < 1000 && pcm[12] > pcm[18]);
        assert!(pcm[18].abs() < 300 && pcm[19].abs() < 300);
        let mut untouched = vec![7i16; 8];
        apply_fade(&mut untouched, 2, 10, 0.0);
        assert_eq!(untouched, [7; 8]);
    }

    #[test]
    fn each_kind_offers_its_formats() {
        assert!(Format::for_kind(Kind::Music).contains(&Format::Midi));
        assert_eq!(Format::for_kind(Kind::Bank), [Format::Vab]);
        assert!(Format::for_kind(Kind::Xa).iter().all(|format| format.export_format().is_some()));
        assert_eq!(ExportFormat::Raw.extension(), "s16");
        assert_eq!(OggQuality::Low.next(-1), OggQuality::High);
    }

    #[test]
    fn a_silent_item_cannot_start_an_export() {
        let library: &'static Library<'static> =
            Box::leak(Box::new(Library::open_with(MemorySource::new(), &NoNaming).unwrap()));
        let dir = scratch("export-silent");
        let result =
            Save::start(library, &item("music:A#0", ""), PlayerOptions::default(), &dir, ExportOptions::default());
        assert!(result.err().is_some_and(|message| message.contains("carries no audio")));
        let message = export_native(library, &item("xa:S#ch0@0", ""), &dir, false);
        assert!(message.contains("no native package"), "{message}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
