// SPDX-License-Identifier: MIT
//! C ABI for [`bof3player`].
//!
//! Every function is `extern "C"`, never unwinds, and reports failure through a
//! [`bof3player_status`] code; [`bof3player_last_error_message`] then returns a
//! human-readable message for the calling thread. The header
//! `include/bof3player.h` is generated from this file with cbindgen.
//!
//! # Thread safety
//!
//! * A `bof3player_library` is immutable after opening: any number of threads may
//!   read the catalog and create players from it at once.
//! * A `bof3player_player` may be moved between threads but must be used by one
//!   thread at a time. Call `render` from the audio thread; it does not block
//!   except through the engine's own read callbacks (streaming XA and CD-DA read
//!   from the source while rendering).
//! * A player keeps its library alive, so `bof3player_library_free` may be called
//!   before the players created from it are freed. Memory handed to
//!   `bof3player_library_open_memory` must stay valid until the library *and* every player
//!   created from it are freed.
//! * Callbacks given to `bof3player_library_open_callbacks` may be invoked from any
//!   thread that calls into the library, including the audio thread, and must
//!   be thread-safe.
//! * [`bof3player_last_error_message`] is per thread.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]
#![warn(missing_docs)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use bof3player::catalog::{Filter, Item, Kind};
use bof3player::export::{self, Tags};
use bof3player::player::Loop;
use bof3player::spu::ReverbModel;
use bof3player::{CallbackSource, Error, Library, MemorySource, Player, PlayerOptions};

/// Result of every fallible call: `BOF3PLAYER_OK` (0) or a negative error code.
pub type bof3player_status = i32;

/// Success.
pub const BOF3PLAYER_OK: bof3player_status = 0;
/// A read or write on the underlying storage failed.
pub const BOF3PLAYER_ERR_IO: bof3player_status = -1;
/// The data is not the format it claims to be.
pub const BOF3PLAYER_ERR_FORMAT: bof3player_status = -2;
/// A pointer argument was null or a value was out of range.
pub const BOF3PLAYER_ERR_INVALID: bof3player_status = -3;
/// The input uses a feature that is not implemented.
pub const BOF3PLAYER_ERR_UNSUPPORTED: bof3player_status = -4;
/// The call is not valid in the current state (for example an unplayable item).
pub const BOF3PLAYER_ERR_STATE: bof3player_status = -5;
/// An internal panic was caught at the boundary; the handle should be freed.
pub const BOF3PLAYER_ERR_PANIC: bof3player_status = -6;

/// Item kind: sequenced music.
pub const BOF3PLAYER_KIND_MUSIC: u32 = 0;
/// Item kind: soundbank or cue record.
pub const BOF3PLAYER_KIND_BANK: u32 = 1;
/// Item kind: one sample.
pub const BOF3PLAYER_KIND_SAMPLE: u32 = 2;
/// Item kind: streaming XA audio.
pub const BOF3PLAYER_KIND_XA: u32 = 3;
/// Item kind: CD-DA track.
pub const BOF3PLAYER_KIND_CDDA: u32 = 4;
/// Kind mask value that selects every kind.
pub const BOF3PLAYER_KIND_ALL: u32 = 0x1F;

/// Loop request: play once.
pub const BOF3PLAYER_LOOP_OFF: i32 = 0;
/// Loop request: repeat forever (any negative value also means forever).
pub const BOF3PLAYER_LOOP_FOREVER: i32 = -1;

/// Reverb timing: hardware (default).
pub const BOF3PLAYER_REVERB_HARDWARE: u32 = 0;
/// Earlier reverb timing, to reproduce old renders.
pub const BOF3PLAYER_REVERB_LEGACY: u32 = 1;

/// API revision. Bumped on every incompatible change to this header.
pub const BOF3PLAYER_API_VERSION: u32 = 1;

/// An owned export buffer. Free it with `bof3player_bytes_free`.
#[repr(C)]
pub struct bof3player_bytes {
    /// The file bytes; null when `len` is 0.
    pub data: *mut u8,
    /// Length in bytes.
    pub len: usize,
}

/// Opaque handle: an opened game source and its catalog.
pub struct bof3player_library {
    inner: Arc<LibraryInner>,
}

/// Opaque handle: one item being played.
pub struct bof3player_player {
    // Field order matters: the player must drop before the library it borrows.
    player: Player<'static>,
    _library: Arc<LibraryInner>,
}

struct LibraryInner {
    library: Library<'static>,
    strings: Vec<ItemStrings>,
}

struct ItemStrings {
    id: CString,
    name: CString,
    title: CString,
    family: CString,
    detail: CString,
    path: CString,
}

/// One catalog entry. All strings are NUL-terminated UTF-8 owned by the library
/// and stay valid until `bof3player_library_free` and every player are freed.
#[repr(C)]
pub struct bof3player_item {
    /// One of the `BOF3PLAYER_KIND_*` values.
    pub kind: u32,
    /// Stable id such as `music:BGM000#0`.
    pub id: *const c_char,
    /// Display name.
    pub name: *const c_char,
    /// Soundtrack title, empty when none is known.
    pub title: *const c_char,
    /// Group (`BGM`, `BOSS`, ...).
    pub family: *const c_char,
    /// One-line summary.
    pub detail: *const c_char,
    /// Source file path.
    pub path: *const c_char,
    /// Sequence id, bank number or channel.
    pub index: u32,
    /// Sample number or first sector.
    pub sub_index: u32,
    /// Length in seconds, or a negative value when unknown without rendering.
    pub duration_seconds: f64,
    /// Whether the item has audio to play.
    pub playable: bool,
    /// Position in the catalog; pass it to `bof3player_player_create`.
    pub catalog_index: usize,
}

/// A file the engine already holds in memory, for `bof3player_library_open_memory`.
#[repr(C)]
pub struct bof3player_memory_file {
    /// Slash-separated path inside the game volume, for example `BIN/BGM/BGM000.EMI`.
    pub path: *const c_char,
    /// The file's bytes. Not copied: they must outlive the library and its players.
    pub data: *const u8,
    /// Length of `data` in bytes.
    pub len: usize,
}

/// Callbacks that expose the engine's own virtual file system.
///
/// All callbacks receive `user_data` unchanged. Strings passed to `file_size`
/// and `read` are paths as returned by `file_path`.
#[repr(C)]
pub struct bof3player_callbacks {
    /// Passed to every callback.
    pub user_data: *mut c_void,
    /// Number of files in the volume.
    pub file_count: Option<unsafe extern "C" fn(user_data: *mut c_void) -> usize>,
    /// Path of file `index` (`0 <= index < file_count`). The pointer must stay
    /// valid until the next callback call.
    pub file_path: Option<unsafe extern "C" fn(user_data: *mut c_void, index: usize) -> *const c_char>,
    /// Store the size of `path` in `*size`; return 0 on success or non-zero if the file does not exist.
    pub file_size: Option<unsafe extern "C" fn(user_data: *mut c_void, path: *const c_char, size: *mut u64) -> i32>,
    /// Read up to `len` bytes of `path` at `offset` into `buffer`; return the
    /// bytes read (0 at the end of the file) or a negative value on error.
    pub read: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            path: *const c_char,
            offset: u64,
            buffer: *mut u8,
            len: usize,
        ) -> i64,
    >,
}

/// Optional settings for `bof3player_player_create`.
#[repr(C)]
pub struct bof3player_player_options {
    /// `sizeof(bof3player_player_options)` as the caller compiled it.
    pub struct_size: usize,
    /// One of the `BOF3PLAYER_REVERB_*` values.
    pub reverb_model: u32,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: &str) {
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = text);
}

fn status_of(error: &Error) -> bof3player_status {
    set_error(&error.to_string());
    match error {
        Error::Io(_) => BOF3PLAYER_ERR_IO,
        Error::Format(_) => BOF3PLAYER_ERR_FORMAT,
        Error::Invalid(_) => BOF3PLAYER_ERR_INVALID,
        Error::Unsupported(_) => BOF3PLAYER_ERR_UNSUPPORTED,
        _ => BOF3PLAYER_ERR_STATE,
    }
}

fn invalid(message: &str) -> bof3player_status {
    set_error(message);
    BOF3PLAYER_ERR_INVALID
}

/// Run `body`, converting a panic into `BOF3PLAYER_ERR_PANIC`.
fn guard(body: impl FnOnce() -> bof3player_status) -> bof3player_status {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| {
        set_error("internal panic");
        BOF3PLAYER_ERR_PANIC
    })
}

unsafe fn str_arg<'a>(pointer: *const c_char) -> Result<&'a str, bof3player_status> {
    if pointer.is_null() {
        return Err(invalid("null string argument"));
    }
    // SAFETY: the caller promises a valid NUL-terminated string.
    unsafe { CStr::from_ptr(pointer) }.to_str().map_err(|_| invalid("string is not valid UTF-8"))
}

fn kind_value(kind: Kind) -> u32 {
    match kind {
        Kind::Music => BOF3PLAYER_KIND_MUSIC,
        Kind::Bank => BOF3PLAYER_KIND_BANK,
        Kind::Sample => BOF3PLAYER_KIND_SAMPLE,
        Kind::Xa => BOF3PLAYER_KIND_XA,
        Kind::Cdda => BOF3PLAYER_KIND_CDDA,
    }
}

fn cstring(text: &str) -> CString {
    CString::new(text.replace('\0', " ")).unwrap_or_default()
}

fn wrap(library: Library<'static>) -> *mut bof3player_library {
    let strings = library
        .catalog()
        .items()
        .iter()
        .map(|item| ItemStrings {
            id: cstring(&item.id),
            name: cstring(&item.name),
            title: cstring(&item.title),
            family: cstring(&item.family),
            detail: cstring(&item.detail),
            path: cstring(&item.path),
        })
        .collect();
    Box::into_raw(Box::new(bof3player_library { inner: Arc::new(LibraryInner { library, strings }) }))
}

/// The version of the library as `"major.minor.patch"`. Static, never null.
#[unsafe(no_mangle)]
pub extern "C" fn bof3player_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// The API revision this build implements (`BOF3PLAYER_API_VERSION`).
#[unsafe(no_mangle)]
pub extern "C" fn bof3player_api_version() -> u32 {
    BOF3PLAYER_API_VERSION
}

/// Free an export buffer. Safe to call with null; the fields are cleared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_bytes_free(bytes: *mut bof3player_bytes) {
    if bytes.is_null() {
        return;
    }
    // SAFETY: the caller passes a buffer from an export function.
    let bytes = unsafe { &mut *bytes };
    if !bytes.data.is_null() {
        let slice = std::ptr::slice_from_raw_parts_mut(bytes.data, bytes.len);
        // SAFETY: the buffer was a boxed slice with this pointer and length.
        drop(unsafe { Box::from_raw(slice) });
    }
    bytes.data = std::ptr::null_mut();
    bytes.len = 0;
}

/// Which container an export writes.
#[derive(Clone, Copy)]
enum ExportKind {
    Wav,
    Flac,
    Ogg(f32),
}

/// Render one catalog item and encode it, then hand the bytes to the caller.
unsafe fn export_item(
    library: *const bof3player_library,
    catalog_index: usize,
    loops: u32,
    title: *const c_char,
    kind: ExportKind,
    out: *mut bof3player_bytes,
) -> bof3player_status {
    guard(|| {
        // SAFETY: the caller passes a handle from this API or null.
        let (Some(handle), false) = (unsafe { library.as_ref() }, out.is_null()) else {
            return invalid("null pointer argument");
        };
        let title = if title.is_null() {
            None
        } else {
            // SAFETY: the caller promises a valid NUL-terminated string.
            match unsafe { CStr::from_ptr(title) }.to_str() {
                Ok(text) => Some(text),
                Err(_) => return invalid("title is not valid UTF-8"),
            }
        };
        let inner = &handle.inner;
        let Some(item) = inner.library.catalog().get(catalog_index) else {
            return invalid("catalog index out of range");
        };
        let mut player = match inner.library.player_with(item, PlayerOptions::default()) {
            Ok(player) => player,
            Err(error) => return status_of(&error),
        };
        let rate = player.sample_rate();
        let channels = player.channels();
        let music = item.kind == Kind::Music;
        let window = player.loop_window().filter(|_| music);
        let requested = if loops == 0 { 1 } else { loops };
        let intro_loop = matches!(kind, ExportKind::Flac | ExportKind::Ogg(_)) && window.is_some() && requested == 1;
        let extra = if intro_loop { 0 } else { requested - 1 };
        player.set_loop(Loop::Times(extra));
        let (loop_start, loop_length) = window.map_or((None, None), |(start, end)| {
            (Some((start * f64::from(rate)).round() as u64), Some(((end - start) * f64::from(rate)).round() as u64))
        });
        let title = title.unwrap_or(if item.title.is_empty() { &item.name } else { &item.title });
        let tags = Tags {
            title,
            artist: if music { "Breath of Fire III" } else { "" },
            album: if music { "Breath of Fire III" } else { "" },
            track: music.then_some(item.index),
            loop_start,
            loop_length,
        };
        let mut pcm = match player.render_all() {
            Ok(pcm) => pcm,
            Err(error) => return status_of(&error),
        };
        if let Some((_, end)) = window.filter(|_| intro_loop) {
            let end = (end * f64::from(rate)).round() as usize * channels as usize;
            pcm.truncate(end);
        }
        let bytes = match kind {
            ExportKind::Wav => export::wav_bytes(&pcm, rate, channels),
            ExportKind::Flac => export::flac_bytes(&pcm, rate, channels, &tags),
            ExportKind::Ogg(quality) => export::ogg_bytes(&pcm, rate, channels, quality, &tags),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => return status_of(&error),
        };
        let mut bytes = bytes.into_boxed_slice();
        let len = bytes.len();
        let data = bytes.as_mut_ptr();
        std::mem::forget(bytes);
        // SAFETY: `out` is a non-null caller-provided buffer.
        unsafe { *out = bof3player_bytes { data, len } };
        BOF3PLAYER_OK
    })
}

/// Render `catalog_index` to a 16-bit PCM WAVE file in `*out`.
///
/// `loops` is `0` for the CLI default (one full pass for WAV) or the number of plays,
/// counting the first. The buffer must be freed with `bof3player_bytes_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_export_wav(
    library: *const bof3player_library,
    catalog_index: usize,
    loops: u32,
    out: *mut bof3player_bytes,
) -> bof3player_status {
    // SAFETY: the caller passes the same pointers the shared helper validates.
    unsafe { export_item(library, catalog_index, loops, std::ptr::null(), ExportKind::Wav, out) }
}

/// Render `catalog_index` to a lossless FLAC file in `*out`; `title` may be
/// null to use the item's title. The title, artist, album, track and loop
/// fields are written as Vorbis comments. `loops` is `0` for the CLI default (the intro
/// plus one loop for music with a loop window) or the number of plays, counting the first.
/// Free with `bof3player_bytes_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_export_flac(
    library: *const bof3player_library,
    catalog_index: usize,
    loops: u32,
    title: *const c_char,
    out: *mut bof3player_bytes,
) -> bof3player_status {
    // SAFETY: the caller passes the same pointers the shared helper validates.
    unsafe { export_item(library, catalog_index, loops, title, ExportKind::Flac, out) }
}

/// Render `catalog_index` to an Ogg Vorbis file in `*out`; `quality` is the
/// Vorbis `-q` scale (0-10), and `title` may be null to use the item's title. `loops` is
/// `0` for the CLI default (the intro plus one loop for music with a loop window) or the
/// number of plays, counting the first. Free with `bof3player_bytes_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_export_ogg(
    library: *const bof3player_library,
    catalog_index: usize,
    loops: u32,
    quality: f32,
    title: *const c_char,
    out: *mut bof3player_bytes,
) -> bof3player_status {
    // SAFETY: the caller passes the same pointers the shared helper validates.
    unsafe { export_item(library, catalog_index, loops, title, ExportKind::Ogg(quality), out) }
}

/// Wording shared with the assets ABI for the statuses both define.
fn common_status(status: bof3player_status) -> Option<&'static [u8]> {
    Some(match status {
        BOF3PLAYER_OK => b"ok\0",
        BOF3PLAYER_ERR_IO => b"i/o error\0",
        BOF3PLAYER_ERR_FORMAT => b"bad format\0",
        BOF3PLAYER_ERR_INVALID => b"invalid argument\0",
        BOF3PLAYER_ERR_PANIC => b"internal panic\0",
        _ => return None,
    })
}

/// A short name for a status code. Static, never null.
#[unsafe(no_mangle)]
pub extern "C" fn bof3player_status_string(status: bof3player_status) -> *const c_char {
    let text = common_status(status).unwrap_or(match status {
        BOF3PLAYER_ERR_UNSUPPORTED => b"unsupported\0",
        BOF3PLAYER_ERR_STATE => b"wrong state\0",
        _ => b"unknown status\0",
    });
    text.as_ptr().cast()
}

/// The message of the last failed call on this thread, or an empty string.
/// The pointer is valid until the next call into the library on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn bof3player_last_error_message() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Open an extracted game folder, a `.cue` sheet, or a disc image `.bin`/`.iso`.
///
/// On success `*out` receives a library to free with `bof3player_library_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_library_open_path(
    path: *const c_char,
    out: *mut *mut bof3player_library,
) -> bof3player_status {
    guard(|| {
        if out.is_null() {
            return invalid("null output pointer");
        }
        let path = match unsafe { str_arg(path) } {
            Ok(p) => p,
            Err(status) => return status,
        };
        match open_path(path) {
            Ok(library) => {
                unsafe { *out = wrap(library) };
                BOF3PLAYER_OK
            }
            Err(error) => status_of(&error),
        }
    })
}

fn open_path(path: &str) -> bof3player::Result<Library<'static>> {
    let p = std::path::Path::new(path);
    if p.is_dir() && p.join("BIN").exists() {
        Library::open(bof3player::FolderSource::open(p)?)
    } else {
        Library::open(bof3player::DiscSource::open(p)?)
    }
}

/// Open a library over files the engine already loaded.
///
/// `files` holds `count` entries. The bytes are borrowed, not copied: keep
/// them valid until `bof3player_library_free` has been called and every player
/// created from the library has been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_library_open_memory(
    files: *const bof3player_memory_file,
    count: usize,
    out: *mut *mut bof3player_library,
) -> bof3player_status {
    guard(|| {
        if out.is_null() || (files.is_null() && count != 0) {
            return invalid("null pointer argument");
        }
        let mut source = MemorySource::new();
        for index in 0..count {
            // SAFETY: `files` points at `count` entries.
            let file = unsafe { &*files.add(index) };
            let path = match unsafe { str_arg(file.path) } {
                Ok(p) => p,
                Err(status) => return status,
            };
            if file.data.is_null() && file.len != 0 {
                return invalid("null file data");
            }
            // SAFETY: the caller promises the bytes outlive the library and its players.
            let bytes: &'static [u8] =
                if file.len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(file.data, file.len) } };
            source.add(path, bytes);
        }
        match Library::open(source) {
            Ok(library) => {
                unsafe { *out = wrap(library) };
                BOF3PLAYER_OK
            }
            Err(error) => status_of(&error),
        }
    })
}

struct UserData(*mut c_void);
// SAFETY: the callback contract requires thread-safe callbacks.
unsafe impl Send for UserData {}
unsafe impl Sync for UserData {}

/// Open a library over the engine's own file system, reached through callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_library_open_callbacks(
    callbacks: *const bof3player_callbacks,
    out: *mut *mut bof3player_library,
) -> bof3player_status {
    guard(|| {
        if out.is_null() || callbacks.is_null() {
            return invalid("null pointer argument");
        }
        // SAFETY: checked non-null; the struct is copied field by field.
        let cb = unsafe { &*callbacks };
        let (Some(file_count), Some(file_path), Some(file_size), Some(read)) =
            (cb.file_count, cb.file_path, cb.file_size, cb.read)
        else {
            return invalid("every callback must be set");
        };
        let user = Arc::new(UserData(cb.user_data));
        let (u1, u2, u3) = (Arc::clone(&user), Arc::clone(&user), user);
        let source = CallbackSource::new(
            move || {
                let count = unsafe { file_count(u1.0) };
                let mut paths = Vec::with_capacity(count);
                for index in 0..count {
                    let pointer = unsafe { file_path(u1.0, index) };
                    if pointer.is_null() {
                        return Err(Error::Invalid("file_path returned null".into()));
                    }
                    paths.push(unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned());
                }
                Ok(paths)
            },
            move |path| {
                let c = CString::new(path).map_err(|_| Error::Invalid("path contains NUL".into()))?;
                let mut size = 0u64;
                if unsafe { file_size(u2.0, c.as_ptr(), &mut size) } != 0 {
                    return Err(Error::Io(format!("no such file: {path}")));
                }
                Ok(size)
            },
            move |path, offset, buffer| {
                let c = CString::new(path).map_err(|_| Error::Invalid("path contains NUL".into()))?;
                let got = unsafe { read(u3.0, c.as_ptr(), offset, buffer.as_mut_ptr(), buffer.len()) };
                usize::try_from(got).map_err(|_| Error::Io(format!("read failed: {path}")))
            },
        );
        match Library::open(source) {
            Ok(library) => {
                unsafe { *out = wrap(library) };
                BOF3PLAYER_OK
            }
            Err(error) => status_of(&error),
        }
    })
}

/// Free a library. Safe to call with null. Players created from it stay valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_library_free(library: *mut bof3player_library) {
    if !library.is_null() {
        drop(catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(library) }))));
    }
}

fn lib<'a>(library: *const bof3player_library) -> Option<&'a LibraryInner> {
    // SAFETY: the caller passes a handle from this API or null.
    unsafe { library.as_ref() }.map(|l| &*l.inner)
}

/// Number of items in the catalog (0 for a null library).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_catalog_count(library: *const bof3player_library) -> usize {
    lib(library).map_or(0, |l| l.library.catalog().len())
}

fn fill_item(inner: &LibraryInner, index: usize, item: &Item, out: &mut bof3player_item) {
    let s = &inner.strings[index];
    *out = bof3player_item {
        kind: kind_value(item.kind),
        id: s.id.as_ptr(),
        name: s.name.as_ptr(),
        title: s.title.as_ptr(),
        family: s.family.as_ptr(),
        detail: s.detail.as_ptr(),
        path: s.path.as_ptr(),
        index: item.index,
        sub_index: item.sub_index,
        duration_seconds: item.duration.unwrap_or(-1.0),
        playable: item.playable,
        catalog_index: index,
    };
}

/// Describe item `index` (`0 <= index < bof3player_catalog_count`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_catalog_item(
    library: *const bof3player_library,
    index: usize,
    out: *mut bof3player_item,
) -> bof3player_status {
    guard(|| {
        let (Some(inner), Some(out)) = (lib(library), unsafe { out.as_mut() }) else {
            return invalid("null pointer argument");
        };
        match inner.library.catalog().get(index) {
            Some(item) => {
                fill_item(inner, index, item, out);
                BOF3PLAYER_OK
            }
            None => invalid("catalog index out of range"),
        }
    })
}

/// Find an item by id (ASCII case-insensitive).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_catalog_find(
    library: *const bof3player_library,
    id: *const c_char,
    out: *mut bof3player_item,
) -> bof3player_status {
    guard(|| {
        let (Some(inner), Some(out)) = (lib(library), unsafe { out.as_mut() }) else {
            return invalid("null pointer argument");
        };
        let id = match unsafe { str_arg(id) } {
            Ok(id) => id,
            Err(status) => return status,
        };
        match inner.library.catalog().index_of(id) {
            Some(index) => {
                fill_item(inner, index, &inner.library.catalog().items()[index], out);
                BOF3PLAYER_OK
            }
            None => invalid("no catalog item with that id"),
        }
    })
}

/// Ranked search (exact id, name prefix, substring, subsequence).
///
/// `kind_mask` is a bit set of `1 << BOF3PLAYER_KIND_*` (0 means every kind). At
/// most `capacity` catalog indices are written to `indices` in rank order; the
/// return value is the total number of matches, which may exceed `capacity`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_catalog_search(
    library: *const bof3player_library,
    query: *const c_char,
    kind_mask: u32,
    playable_only: bool,
    indices: *mut usize,
    capacity: usize,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(inner) = lib(library) else { return 0 };
        let query = unsafe { str_arg(query) }.unwrap_or("");
        let filter = Filter {
            kinds: Kind::ALL.into_iter().filter(|k| kind_mask != 0 && kind_mask & (1 << kind_value(*k)) != 0).collect(),
            family: None,
            playable_only,
        };
        let hits = inner.library.catalog().search(query, &filter);
        if !indices.is_null() {
            for (slot, item) in hits.iter().take(capacity).enumerate() {
                let index = inner.library.catalog().index_of(&item.id).unwrap_or(0);
                // SAFETY: the caller promises room for `capacity` entries.
                unsafe { *indices.add(slot) = index };
            }
        }
        hits.len()
    }))
    .unwrap_or(0)
}

/// Create a player for catalog item `catalog_index`, positioned at the start.
///
/// `options` may be null for the defaults. On success `*out` must be freed with
/// `bof3player_player_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_create(
    library: *const bof3player_library,
    catalog_index: usize,
    options: *const bof3player_player_options,
    out: *mut *mut bof3player_player,
) -> bof3player_status {
    guard(|| {
        // SAFETY: the caller passes a handle from this API or null.
        let (Some(handle), false) = (unsafe { library.as_ref() }, out.is_null()) else {
            return invalid("null pointer argument");
        };
        let inner = &handle.inner;
        let Some(item) = inner.library.catalog().get(catalog_index) else {
            return invalid("catalog index out of range");
        };
        let mut settings = PlayerOptions::default();
        if let Some(options) = unsafe { options.as_ref() }
            && options.struct_size >= std::mem::size_of::<bof3player_player_options>()
        {
            settings.reverb = if options.reverb_model == BOF3PLAYER_REVERB_LEGACY {
                ReverbModel::Legacy
            } else {
                ReverbModel::Hardware
            };
        }
        match inner.library.player_with(item, settings) {
            Ok(player) => {
                // SAFETY: the player borrows `inner.library`, which the Arc stored beside it keeps alive
                // at a stable heap address for as long as the player exists.
                let player: Player<'static> = unsafe { std::mem::transmute::<Player<'_>, Player<'static>>(player) };
                let boxed = Box::new(bof3player_player { player, _library: Arc::clone(inner) });
                unsafe { *out = Box::into_raw(boxed) };
                BOF3PLAYER_OK
            }
            Err(error) => status_of(&error),
        }
    })
}

/// Free a player. Safe to call with null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_free(player: *mut bof3player_player) {
    if !player.is_null() {
        drop(catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(player) }))));
    }
}

fn pl<'a>(player: *mut bof3player_player) -> Option<&'a mut bof3player_player> {
    // SAFETY: the caller passes a handle from this API or null and uses it from one thread at a time.
    unsafe { player.as_mut() }
}

/// Sample rate of the produced audio in Hz (0 for a null player).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_sample_rate(player: *mut bof3player_player) -> u32 {
    pl(player).map_or(0, |p| p.player.sample_rate())
}

/// Interleaved channels per frame, 1 or 2 (0 for a null player).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_channels(player: *mut bof3player_player) -> u32 {
    pl(player).map_or(0, |p| p.player.channels())
}

/// Length of one pass in seconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_duration(player: *mut bof3player_player) -> f64 {
    pl(player).map_or(0.0, |p| p.player.duration())
}

/// Current position in seconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_position(player: *mut bof3player_player) -> f64 {
    pl(player).map_or(0.0, |p| p.player.position())
}

/// The item's own loop window; returns false when it has none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_loop_window(
    player: *mut bof3player_player,
    start: *mut f64,
    end: *mut f64,
) -> bool {
    let Some((s, e)) = pl(player).and_then(|p| p.player.loop_window()) else { return false };
    // SAFETY: optional out pointers.
    unsafe {
        if let Some(start) = start.as_mut() {
            *start = s;
        }
        if let Some(end) = end.as_mut() {
            *end = e;
        }
    }
    true
}

/// The body both render calls share: validate the pointers, size the buffer
/// for the player's channels, render, and report the frames written.
unsafe fn render_into<T>(
    player: *mut bof3player_player,
    out: *mut T,
    frames: usize,
    frames_written: *mut usize,
    render: fn(&mut Player<'static>, &mut [T]) -> bof3player::Result<usize>,
) -> bof3player_status {
    guard(|| {
        let Some(p) = pl(player) else { return invalid("null player") };
        if out.is_null() || frames_written.is_null() {
            return invalid("null pointer argument");
        }
        // SAFETY: the caller promises `frames * channels` writable samples.
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, frames * p.player.channels() as usize) };
        match render(&mut p.player, buffer) {
            Ok(n) => {
                unsafe { *frames_written = n };
                BOF3PLAYER_OK
            }
            Err(error) => status_of(&error),
        }
    })
}

/// Render up to `frames` frames of interleaved 16-bit PCM into `out`
/// (`frames * channels` samples). `*frames_written` receives the frames
/// produced; 0 means the item has ended (or the player is stopped).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_render_i16(
    player: *mut bof3player_player,
    out: *mut i16,
    frames: usize,
    frames_written: *mut usize,
) -> bof3player_status {
    // SAFETY: the caller passes the same promises the two render calls state.
    unsafe { render_into(player, out, frames, frames_written, |player, out| player.render_i16(out)) }
}

/// Like `bof3player_player_render_i16`, producing floats in -1.0..1.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_render_f32(
    player: *mut bof3player_player,
    out: *mut f32,
    frames: usize,
    frames_written: *mut usize,
) -> bof3player_status {
    // SAFETY: the caller passes the same promises the two render calls state.
    unsafe { render_into(player, out, frames, frames_written, |player, out| player.render_f32(out)) }
}

/// Move to `seconds` (clamped to the item). Also resumes a stopped player.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_seek(player: *mut bof3player_player, seconds: f64) -> bof3player_status {
    guard(|| match pl(player) {
        Some(p) => p.player.seek(seconds).map_or_else(|e| status_of(&e), |()| BOF3PLAYER_OK),
        None => invalid("null player"),
    })
}

/// Loop request: 0 plays once, N > 0 repeats the loop window N extra times,
/// a negative value repeats forever. Only sequenced music loops.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_set_loop(player: *mut bof3player_player, passes: i32) -> bof3player_status {
    guard(|| match pl(player) {
        Some(p) => {
            p.player.set_loop(match passes {
                0 => Loop::Off,
                n if n < 0 => Loop::Forever,
                n => Loop::Times(n.unsigned_abs()),
            });
            BOF3PLAYER_OK
        }
        None => invalid("null player"),
    })
}

/// Output gain, 0.0 to 4.0 (1.0 is a bit-exact bypass).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_set_gain(player: *mut bof3player_player, gain: f32) -> bof3player_status {
    guard(|| match pl(player) {
        Some(p) => {
            p.player.set_gain(gain);
            BOF3PLAYER_OK
        }
        None => invalid("null player"),
    })
}

/// Stop: renders return 0 frames until `bof3player_player_play` or a seek.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_stop(player: *mut bof3player_player) -> bof3player_status {
    guard(|| match pl(player) {
        Some(p) => {
            p.player.stop();
            BOF3PLAYER_OK
        }
        None => invalid("null player"),
    })
}

/// Undo `bof3player_player_stop` without moving the position.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bof3player_player_play(player: *mut bof3player_player) -> bof3player_status {
    guard(|| match pl(player) {
        Some(p) => {
            p.player.play();
            BOF3PLAYER_OK
        }
        None => invalid("null player"),
    })
}
