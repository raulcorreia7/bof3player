# Crates

The generic PlayStation crates and the EMI reader. The PlayStation crates know no game. `emi` reads the container format of *Breath of Fire III*.

## psx-audio

PlayStation audio in Rust: an SPU voice engine, XA and CD-DA playback, file sources, and WAV, FLAC
and Ogg Vorbis export. The hardware model follows the public psx-spx documentation: 24 ADPCM
voices, ADSR envelopes, 4-point Gaussian interpolation, reverb and the main mixer at 44.1 kHz. The
crate knows no game, archive container or catalog.

| Module   | Contents                                                                               |
| -------- | -------------------------------------------------------------------------------------- |
| `spu`    | 24 ADPCM voices, ADSR, Gaussian interpolation, reverb, the main mixer and the presets  |
| `player` | pull-based `Player`, the `Engine` trait, XA streams and CD-DA tracks                   |
| `source` | `Blob` and `Source`, over memory, callbacks, folders, disc images or any `Read + Seek` |
| `export` | 16-bit WAVE, raw little-endian samples, FLAC and Ogg Vorbis, with loop and title tags  |
| `error`  | one `Error` type and `Result`                                                          |

```rust,no_run
# fn example() -> Result<(), psx_audio::Error> {
use psx_audio::player::XaStream;
use psx_audio::{MemorySource, Player, Source};

let mut source = MemorySource::new();
source.add("XA.BIN", &[0u8; 2336 * 2]);
let stream = XaStream { channel: 0, first_sector: 0, sector_count: 0, duration: 0.0 };
let mut player = Player::xa(source.open("XA.BIN")?, stream)?;

let mut pcm = [0i16; 1024 * 2];
let frames = player.render_i16(&mut pcm)?; // 0 means the item ended
# let _ = (frames, player.sample_rate(), player.channels());
# Ok(())
# }
```

`Player::set_loop(Loop::Off | Loop::Times(n) | Loop::Forever)` and `player.seek(seconds)` control
playback; `render_all()` returns the intro plus the requested loops.

| Part            | Format                                                          | Reference                                                                                             |
| --------------- | --------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| SPU voices      | 16-byte ADPCM blocks, ADSR, Gaussian interpolation, reverb      | psx-spx "SPU ADPCM Samples", "SPU ADPCM Pitch", "SPU Volume and ADSR Generator", "SPU Reverb Formula" |
| XA streams      | 2336-byte sectors, subheaders, channels and interleave          | psx-spx "CDROM XA Subheader, File, Channel, Interleave"                                               |
| CD-DA           | 2352-byte raw frames or a 16-bit WAVE file                      | psx-spx "CDROM Disk Format"                                                                           |
| Disc images     | ISO 9660, cooked 2048-byte or raw 2352-byte sectors, CUE sheets | [psx-cd](#psx-cd)                                                                                     |
| VAB, SEP, ADPCM | sound banks, sequence packages and block codecs                 | [psx-formats](#psx-formats)                                                                           |

| Feature | Default | What it adds                                                                     |
| ------- | ------- | -------------------------------------------------------------------------------- |
| `std`   | yes     | `FolderSource`, `DiscSource`, `ReaderBlob` and `Read + Seek` adapters            |
| `ogg`   | yes     | `export::ogg_bytes` through a bundled libvorbis; adds a C compiler at build time |

WAV and FLAC export work without `std` (`no_std + alloc`). Build without Ogg and the C compiler
with `cargo build -p psx-audio --no-default-features --features std`.

## psx-cd

Read and write PlayStation CD-ROM images: ISO9660 file tables with CD-XA directory attributes,
Mode 2 (2336-byte) XA sectors, CUE/BIN sheets, CD-DA track extraction, checksum manifests, and a
patch writer that replaces files in place and re-seals their sectors. It depends on `psx-formats`
for the Mode 2 sector checksums.

| Part           | Format                                                            | Reference                                                                                    |
| -------------- | ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Images         | ISO9660 volume descriptors, directory records, both-endian fields | psx-spx "CDROM ISO Volume Descriptors", "CDROM ISO File and Directory Descriptors"; ECMA-119 |
| Sectors        | 2352-byte raw sectors, 2336-byte XA sectors, XA subheaders        | psx-spx "CDROM Sector Encoding", "CDROM XA Subheader, File, Channel, Interleave"             |
| CUE sheets     | `FILE`, `TRACK`, `INDEX`, `PREGAP` and timecodes                  | cdrdao cue-sheet syntax                                                                      |
| CD-DA tracks   | 2352-byte frames, 588 stereo samples, 44.1 kHz                    | psx-spx "CDROM Disk Format"                                                                  |
| Patched images | Mode 2 Form 1 EDC and ECC, re-sealed per sector                   | psx-spx "CDROM Sector Encoding" (sealing through `psx-formats`)                              |

An image opens from a path (a cooked `.iso`, a raw `.bin`, or a `.cue` that names its data track)
or from any `Read + Seek` source. The layout is detected from the primary volume descriptor at
sector 16.

| Module           | Contents                                                               |
| ---------------- | ---------------------------------------------------------------------- |
| `Image`, `Entry` | open an image, walk the file table, read files and XA streams, extract |
| `cue`            | CUE sheet parsing and CD-DA extraction as WAV or raw PCM               |
| `checksum`       | MD5 and SHA-256 manifests of an extracted tree                         |
| `metadata`       | project XML and `disc_lba.json`                                        |
| `patch`          | a patched copy of the data track                                       |
| `rebuild`        | a deterministic cooked ISO from a folder of files                      |

## psx-formats

Decoders, encoders and validators for the file formats of the original PlayStation: SPU and XA
ADPCM, VAB sound banks, SEP sequence packages, Mode 2 Form 1 sectors, RIFF/WAVE files, and the
GPU's VRAM uploads, CLUTs, TIM images and PNG views. The crate is `no_std + alloc` without `std`;
it knows nothing about voices, players, archive containers or a particular game.

| Module   | Format                              | Reference                                                                                         |
| -------- | ----------------------------------- | ------------------------------------------------------------------------------------------------- |
| `adpcm`  | 16-byte SPU ADPCM blocks, XA filter | psx-spx "SPU ADPCM Samples", "CDROM XA Audio ADPCM Compression"                                   |
| `xa`     | CD-ROM XA streams                   | psx-spx "CDROM XA Subheader, File, Channel, Interleave"                                           |
| `sector` | Mode 2 Form 1 sectors               | psx-spx "CDROM Sector Encoding", ECMA-130 Annex B                                                 |
| `vab`    | VAB sound banks                     | Sony PlayStation sound-library VAB layout; sample data as "SPU ADPCM Samples"                     |
| `sep`    | SEP sequence packages               | Sequence events and running status as in the Standard MIDI File specification (format 0)          |
| `wav`    | RIFF/WAVE 16-bit PCM                | RIFF and WAVE specifications                                                                      |
| `gfx`    | VRAM uploads, CLUTs, TIM, PNG       | psx-spx "GPU Video Memory (VRAM)", "GPU Render Polygon Commands"; Sony *File Formats* ch. 3 (TIM) |

`gfx` and `sf2` need the `std` feature. The `fixtures` feature adds small generated banks,
sequences and sectors for dependents' tests.

## emi

A byte-oriented reader for EMI archives and their entries.

```rust
use emi::Archive;

let bytes: &[u8] = &[]; // an EMI archive
if let Ok(archive) = Archive::parse(bytes) {
    for entry in archive.entries() {
        println!("type {} at {:#x}, {} bytes", entry.file_type, entry.offset, entry.size);
    }
}
```

`Archive::parse` and the payload reads work on bytes and need no `std`; the `std` feature adds
`Archive::open`, `read_entry`, extraction and entry naming.

Every crate is MIT licensed and carries its own `LICENSE`. Every source file has an
`SPDX-License-Identifier: MIT` header.
