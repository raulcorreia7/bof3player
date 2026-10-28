# Rust API

The `bof3player` crate (`core/`) gives you a catalog of the game's audio, a pull-based player and
exporters. It does not open an audio device. The `bof3player` tool uses `cpal` for that, and an
embedding engine brings its own mixer. The crate is not on crates.io, so add it by path or git:

```toml
[dependencies]
bof3player = { path = "bof3player/core" }
```

PCM comes out at each item's native rate, with no resampling. Music, samples and CD-DA use
44.1 kHz. XA uses 37.8 or 18.9 kHz.

## Play an item

```rust,no_run
use bof3player::{FolderSource, Library};

fn main() -> Result<(), bof3player::Error> {
    // An extracted tree. Use DiscSource for a .cue/.bin disc image.
    let library = Library::open(FolderSource::open("path/to/extracted")?)?;

    let item = library.catalog().find("music:ITEM#0").expect("the item exists");
    println!("{} - {}", item.id, item.name);

    let mut player = library.player(item)?;
    let mut pcm = vec![0i16; 1024 * player.channels() as usize];
    let frames = player.render_i16(&mut pcm)?; // 0 means the item ended
    println!("{} frames at {} Hz", frames, player.sample_rate());
    Ok(())
}
```

`render_f32` fills interleaved `f32` samples the same way. To seek, loop and export:

```rust,no_run
# use bof3player::Library;
# fn demo(library: &Library<'_>, item: &bof3player::Item) -> Result<(), bof3player::Error> {
let mut player = library.player(item)?;
player.seek(30.0)?;
player.set_loop(bof3player::player::Loop::Forever);
let pcm = player.render_all()?;
let (rate, channels) = (player.sample_rate(), player.channels());

let midi = library.export_midi(item)?; // also export_vag, export_sequence, export_bank
let wav = bof3player::export::wav_bytes(&pcm, rate, channels)?;
let flac = bof3player::export::flac_bytes(&pcm, rate, channels, "Opening")?;
let ogg = bof3player::export::ogg_bytes(&pcm, rate, channels, 6.0, "Opening")?;
# let _ = (midi, wav, flac, ogg);
# Ok(())
# }
```

## Give it data

A `Source` is a volume of files addressed by path. Pick the mode that fits your engine:

```rust,no_run
use bof3player::{DiscSource, Library, MemorySource, ReaderBlob};

# fn demo(emi_bytes: &[u8], pak_file_handle: std::fs::File) -> Result<(), bof3player::Error> {
// 1. Bytes the engine already loaded. The source borrows them.
let mut source = MemorySource::new();
source.add("audio/archive.emi", emi_bytes);
source.add_blob("audio/stream.bin", std::sync::Arc::new(ReaderBlob::new(pak_file_handle)?));
let library = Library::open(source)?;

// 2. A path: an extracted tree, or a .cue/.bin disc image.
let library = Library::open(DiscSource::open("disc.cue")?)?;
# let _ = library;
# Ok(())
# }
```

A third mode, `CallbackSource`, takes three callbacks (`list`, `size` and `read-at`) so an engine
can serve files from its own virtual file system.

## Names and titles

The catalog knows ids, paths and formats. `Library::open` uses `NoNaming`, so items show ids and
archive names. To show display names or soundtrack titles, implement `Naming`
(see `core/src/catalog.rs`) and pass it to `Library::open_with`. Every method has a default that
returns `None`. `names::Names` is a `Naming` that reads the JSON names file
([player.md](player.md#names-file)) with the `names` feature; `data/bof3-names.json` is the file this repository ships.

## Features and threads

| Feature | Default | What it adds                                                          |
| ------- | ------- | --------------------------------------------------------------------- |
| `std`   | yes     | `FolderSource`, `DiscSource`, `ReaderBlob` and `discover`.            |
| `ogg`   | yes     | `export::ogg_bytes`, through a bundled libvorbis. Needs a C compiler. |
| `cli`   | no      | The `bof3player` binary and its terminal browser.                     |

With `--no-default-features` the crate builds without `std` or Ogg (`no_std` plus `alloc`), and WAV
and FLAC export still work.

`Library` is `Send + Sync`. A `Player` is `Send`, but use it from one thread at a time. Rendering
allocates only on the first call after a seek.
