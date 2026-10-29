# bof3player

[![CI](https://github.com/raulcorreia7/bof3player/actions/workflows/ci.yml/badge.svg?branch=main&event=workflow_dispatch)](https://github.com/raulcorreia7/bof3player/actions/workflows/ci.yml)

A player for the music and sound of *Breath of Fire III* (PlayStation). It reads the game's sound
banks, sequences, XA streams and CD audio from your own disc image and renders them through a model
of the PlayStation sound chip (the SPU).

**No game data is included.** Bring your own disc image (`.cue` and `.bin`) or a folder extracted
from one. This is a fan project, not affiliated with or endorsed by Capcom. *Breath of Fire* is
Capcom's trademark.

## The tools

| Tool             | What it does                                                                      |
| ---------------- | --------------------------------------------------------------------------------- |
| `bof3player`     | Command line: list, search, play, render to WAV, FLAC or Ogg, export MIDI, check. |
| `bof3player-tui` | Terminal browser: search, play with loop band and spectrum, export.               |
| `bof3player-gui` | Window with the same features. Linux, macOS and Windows.                          |

All three take `-i PATH` for the disc image or folder. Without it they search the working directory,
the program's folder and `$BOF3_GAME_DIR`; the browser and the window open a chooser when nothing is
found. The commands and switches are in [docs/player.md](docs/player.md) and in `--help`.

```sh
bof3player -i "Breath of Fire III.cue" doctor
bof3player -i "Breath of Fire III.cue" list --kind music
bof3player -i "Breath of Fire III.cue" render --format flac -o song.flac "music:BGMOPN#0"
bof3player-tui -i "Breath of Fire III.cue"
```

## Game data

`-i` takes any of these:

- a `.cue` sheet, with its `.bin` next to it;
- a raw disc image (`.bin` or `.iso`);
- a folder of files extracted from the disc. Use the folder that holds `SLUS_004.22` and `BIN/`,
  or the `BIN` folder itself.

Any tool that copies the disc's files out works for extraction. Then pass the folder:

```sh
bof3player -i ~/games/bof3-extracted list --kind music
```

## Install

Download an archive from [Releases](https://github.com/raulcorreia7/bof3player/releases), unpack it
and run the tools from the folder. Each archive contains all three tools and `data/bof3-names.json`:

- `bof3player-linux-x86_64.tar.gz` or `bof3player-linux-aarch64.tar.gz`
- `bof3player-macos-arm64.tar.gz` or `bof3player-macos-x86_64.tar.gz`
- `bof3player-windows-x86_64.zip`

Each archive has a matching `.sha256` checksum file.

## Build from source

You need Rust 1.88 or newer and GNU make. On Linux, audio output needs the ALSA headers. Ogg export
builds a bundled libvorbis and needs a C compiler.

```sh
make build   # release builds of the three tools into target/release
make check   # format, clippy, cargo deny, tests and no-default-features check
```

`make help` lists every target and the plain cargo commands (use those on Windows without make).

## Names file

The disc names no track, so `data/bof3-names.json` ships soundtrack titles, place labels and
duplicate marks, each with its source. The tools find it next to the program, in the working
directory or in `data/`. `--names FILE` or `$BOF3_NAMES` picks another file. See
[data/README.md](data/README.md) and [docs/player.md](docs/player.md#names-file).

## Use it as a library

| Part                                 | What it is                                                                                           | Guide                                        |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| [bof3-names](bof3-names/README.md)   | Bundled game names for Rust library callers.                                                          | [Names file](data/README.md)                 |
| [core](core/README.md)               | `bof3player`: catalog, playback and export in Rust, and the `bof3player` and `bof3player-tui` tools. | [docs/rust-api.md](docs/rust-api.md)         |
| [psx-audio](psx-audio/README.md)     | SPU voices, XA and CD-DA playback, sources, WAV, FLAC and Ogg export.                                | [docs/crates.md](docs/crates.md#psx-audio)   |
| [psx-formats](psx-formats/README.md) | VAB, SEP, ADPCM, sector, VRAM, CLUT and TIM formats.                                                 | [docs/crates.md](docs/crates.md#psx-formats) |
| [psx-cd](psx-cd/README.md)           | ISO 9660 and cue-sheet disc images: read, extract, patch, rebuild.                                   | [docs/crates.md](docs/crates.md#psx-cd)      |
| [emi](emi/README.md)                 | The EMI archive reader.                                                                              | [docs/crates.md](docs/crates.md#emi)         |

The crates are not on crates.io. Use them as path or git dependencies. [docs/spu-model.md](docs/spu-model.md)
lists the hardware documentation the SPU model follows.

## Contributing and license

See [CONTRIBUTING.md](CONTRIBUTING.md). The code is MIT licensed ([LICENSE](LICENSE)). Do not add
game data, extracted assets or game text to the repository.
