# The bof3player command

Three tools read a *Breath of Fire III* disc image:

- `bof3player`: the command line (list, search, play, render, export, verify, doctor). With no
  command it prints a usage line.
- `bof3player-tui`: the terminal browser. It takes the shared switches and no command.
- `bof3player-gui`: the window.

Build the first two from the repository root (the binaries end in `.exe` on Windows):

```sh
cargo build --release -p bof3player --features cli --bin bof3player --bin bof3player-tui
```

The binary is `target/release/bof3player`. Every switch shows its default in `--help`, and each
command has its own help, for example `bof3player render --help`.

## Game data

Use your own disc image or an extracted folder. `-i/--input PATH` names it (`--dir` is an alias).
Without `-i` the player looks, in order, at:

1. `$BOF3_GAME_DIR`, as given and as `<value>/out/extracted`;
2. the working directory and its parents;
3. the executable's directory and its parents.

Each folder is also tried with `out/extracted`, `rebof3/build/disc` (the monorepo's disc), `BIN` and
`inputs/external` below it.

`bof3player doctor` lists every place it tried and the audio device it found. If nothing opens,
the `bof3player` error names the places and the next step (`-i <disc.cue or folder>`), and the
exit code is not zero. `bof3player-tui` and `bof3player-gui` open a
chooser: walk the folders and pick a `.cue` sheet, a disc image or a folder of extracted files.
In the browser, Enter opens, `h` goes up, `o` uses the current folder and `q` quits.

## Desktop window

`bof3player-gui` is a window with the terminal browser's features: the catalog as a tree of kind,
family and item, ranked search, play, pause, seek, volume (click the speaker to mute), loop, play on, level meters, a spectrum
analyzer, and export to WAV, FLAC, Ogg Vorbis and native files. It uses [gpui](https://gpui.rs)
and runs on Linux, macOS and Windows. Build and run it from the repository root:

```sh
cargo run --release -p bof3player --features gui --bin bof3player-gui -- -i "Breath of Fire III.cue"
```

The window takes the same switches as `bof3player`; `--help` lists them. `--names FILE` reads
a [names file](#names-file). `--mute` starts with the sound off; click the speaker or press `M` to turn it on.

The window shows the disc or folder in use. When no game data opens, it says what failed, lists
the places it tried and opens a chooser. Browse folders, then pick a `.cue` sheet, a disc image or
a folder of extracted files. Browse does the same at any time, and Change in the export menu picks
the export folder. You can also type a path in the game data box and press Open. The
keys are the browser's keys ([Browser keys](#browser-keys)); `?` lists them in the window.

## Names file

The repository ships [`data/bof3-names.json`](../data/README.md): soundtrack titles, place labels
and duplicate marks, each with its source. `--names FILE` reads another file. Without the switch all
three tools use the first of: `$BOF3_NAMES`, then `bof3-names.json`, `build/bof3-names.json` or
`data/bof3-names.json` in the working directory and its parents, then in the executable's directory
and its parents. With none found they show archive names. A copy of a track stays in the list under its own id and shows its plain title; the details and `list` show `same as ID`.

```json
{
  "version": 1,
  "music": {
    "music:BGM000#0": {
      "title": "Title",
      "source": "data",
      "sound": [0],
      "evidence": "where it is from"
    },
    "music:BGM008#0": { "same_as": "music:BGM003#0" }
  },
  "banks": {
    "bank:AREA001": {
      "title": "Name",
      "source": "label",
      "same_as": "bank:AREA000"
    }
  },
  "xa": { "xa:S_XA00#ch0@0": { "title": "Cue 0", "source": "game" } }
}
```

| Field       | Where | Meaning                                                                   |
| ----------- | ----- | ------------------------------------------------------------------------- |
| `version`   | top   | Required; `1`.                                                            |
| `music`     | top   | Entries keyed by catalog id, `music:ARCHIVE#SEQUENCE`.                    |
| `banks`     | top   | Entries keyed `bank:ARCHIVE`.                                             |
| `xa`        | top   | Entries keyed by catalog id, for example `xa:S_XA00#ch0@0`.               |
| `title`     | entry | Display name. Omit it on a copy that shares its original's title.         |
| `source`    | entry | `game`, `text`, `data`, `label` or `none` (default). How far to trust it. |
| `evidence`  | entry | A short pointer: file and offset, or table and symbol.                    |
| `area`      | entry | Where the item plays, for a `label`.                                      |
| `same_as`   | entry | Id of the item whose content this one repeats.                            |
| `sound`     | music | The game's sound ids that select the track.                               |
| `abilities` | bank  | Ability indexes of a magic bank, in the order of the names in `title`.    |

Every section and field except `version` may be absent. Unknown fields are ignored; an unknown
`source`, another `version` or text that is not JSON is an error: `--names` stops, and a default
file is reported and skipped.

On Linux, gpui needs the Wayland or X11, xkbcommon, fontconfig and Vulkan development packages.

## Commands

| Command            | What it does                                           |
| ------------------ | ------------------------------------------------------ |
| `list` (`ls`)      | List items, optionally filtered.                       |
| `search` (`find`)  | Rank matches across ids, names, titles and families.   |
| `info` (`show`)    | Show one item's id, title, duration, path and details. |
| `play`             | Play an item or an audio file.                         |
| `render` (`save`)  | Write WAV, raw s16, FLAC or Ogg.                       |
| `export`           | Write a native MIDI, VAG or VAB file.                  |
| `verify` (`check`) | Render matching items and report which have sound.     |
| `stats`            | Show catalog totals.                                   |
| `music` (`songs`)  | List music archives and their titles.                  |

`--json` works for `list`, `search`, `info`, `stats` and `music`. `-q/--quiet` drops the
`(n of total shown)` and `wrote` lines, which helps in scripts. A first argument that is not a
command is a search query: `bof3player battle` plays the best match.

```sh
bof3player -i GAME.cue doctor
bof3player -i GAME.cue list --kind music
bof3player search "music"
bof3player play music:ITEM#0 --loops 1
bof3player render music:ITEM#0 --format flac -o opening.flac
bof3player render --all --out-dir out/ --kind music --format flac
bof3player export midi music:ITEM#0 -o opening.mid
bof3player verify --kind sample --seconds 5
```

`ITEM` is a placeholder. Use an id from `bof3player list --kind music`.

## Render formats

| Format     | Extension | Tags                                    |
| ---------- | --------- | --------------------------------------- |
| WAV        | `.wav`    | none                                    |
| Raw s16le  | `.s16`    | none; interleaved little-endian samples |
| FLAC       | `.flac`   | Vorbis comments                         |
| Ogg Vorbis | `.ogg`    | Vorbis comments                         |

`--quality` sets Ogg quality from 0 (smallest) to 10 (best); the default is 6. `--loops N` counts
plays of a looping track including the first; the default is 1. FLAC and Ogg music carry `TITLE`,
`ARTIST`, `ALBUM`, `TRACKNUMBER`, `LOOPSTART` and `LOOPLENGTH` tags. Without `--loops` they hold
the intro and one loop, and stop at the loop end.

## Browser keys

| Key                               | Action                                                                                        |
| --------------------------------- | --------------------------------------------------------------------------------------------- |
| `j`/`k`, arrows, `g`/`G`, `d`/`u` | Move.                                                                                         |
| `l`/`h`, Right/Left               | Open or close a group.                                                                        |
| `Enter`                           | Play, or open a group.                                                                        |
| `Space`                           | Pause or resume.                                                                              |
| `[` `]`                           | Seek 5 s back or forward.                                                                     |
| `+` `-`                           | Change the volume.                                                                            |
| `M`                               | Mute or unmute.                                                                               |
| `m`                               | Toggle looping (on at start).                                                                 |
| `n` `N`                           | Next or previous playable item.                                                               |
| `c`                               | Play on when an item ends.                                                                    |
| `/`                               | Ranked search. Enter keeps the results, Esc clears them.                                      |
| `w`                               | Save the selected item as WAV. `--loops N` plays N times; `--fade SECONDS` shortens the tail. |
| `e`                               | Export menu: WAV, FLAC and Ogg Vorbis. `[` `]` set the loop count.                            |
| `E`                               | Choose the folder exports are written into.                                                   |
| `x`                               | Export the native package (MIDI, VAG or VAB).                                                 |
| `X`                               | Export the sequence package (music).                                                          |
| `a`                               | Show all items. Off shows playable items only.                                                |
| `?`, `q`                          | Help, quit.                                                                                   |

The now-playing panel shows the position, the item's loop window, left and right level meters
and a waveform.

`w`, `x` and `X` write into `--export-dir`. The default is `$XDG_MUSIC_DIR/Breath of Fire III` on
Linux, `~/Music/Breath of Fire III` on macOS, `%USERPROFILE%\Music\Breath of Fire III` on Windows,
and `./exports` when no home folder is known. Each write prints its absolute path.

## Limits and troubleshooting

The player renders the game's sound banks and streams. It does not emulate a console. XA and CD-DA
use their own decoders, not the SPU model. [spu-model.md](spu-model.md) states the hardware
contract.

| Symptom                      | What to do                                                                                     |
| ---------------------------- | ---------------------------------------------------------------------------------------------- |
| `no game data found`         | Pass `-i PATH`, set `$BOF3_GAME_DIR`, or run `bof3player doctor`.                              |
| No sound                     | On Linux, install the ALSA headers and check the device `doctor` reports.                      |
| `ogg` build fails            | Build with `--no-default-features`, or render FLAC. Ogg needs a C compiler.                    |
| Names or titles are empty    | Keep `bof3-names.json` next to the program or pass `--names FILE` ([Names file](#names-file)). |
| Slow browser on a disc image | Extract the tree first and pass that folder with `-i`.                                         |
