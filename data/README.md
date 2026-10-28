# bof3-names.json

The names `bof3player` shows for the US disc (SLUS_004.22). One file; the schema is in
[docs/player.md](../docs/player.md#names-file). The players find it without a flag: next to the
executable, in the working directory, or in `data/` of this repository. `--names FILE` or
`$BOF3_NAMES` picks another file.

## Sources

| `source` | Meaning                                                                                |
| -------- | -------------------------------------------------------------------------------------- |
| `game`   | A table in the game files: bank names, ability names, XA cue ids                       |
| `data`   | Soundtrack titles from the "Breath of Fire III" PSF set (OverClocked ReMix chip 12102) |
| `label`  | A neutral label from where the music plays                                             |
| `none`   | No source names the item                                                               |

The disc holds no music titles: EMI entries carry type and size only, SEP headers carry id,
resolution, tempo and length, and no table or text names a track.

## Music

- `sound` lists the rows of the game's sound table that select the slot.
- A slot with `same_as` has byte-identical sequence data (SHA-256 of the body, header excluded) to
  the named slot. The original is the titled slot with the lowest sound id. A copy omits `title`
  when it equals the original's and keeps its own when the two data sets differ.
- Slots that are silent in their own bank are not listed.

## Banks

`bank:ARCHIVE` entries come from the area, enemy, character and magic loaders. `same_as` groups
archives whose first VAB header and body are byte-identical (SHA-256).

Boss labels combine the battle-mode archive and formation tables with area enemy records. Magic
labels can list party members who learn an ability in the `GAME.EMI` level-growth table and one
enemy whose initial or active AI skills include it. These are examples, not exhaustive lists of all
users.

## Editing the file

Edit it by hand and check each fact against your disc.

1. **Slots.** Each EMI archive has a 16-byte header and 16-byte TOC entries (`u32` size, `u16` type at
   byte 12); payloads start at `0x800`, each padded to `0x800`. Type 10 is a SEP package: `pQES`,
   `u16` version, then per sequence a 13-byte header (`u16` id at 0, `u32` big-endian body size at 9)
   and the body. The slot is `ARCHIVE#id`. `bof3player list --kind music` shows which are playable.
2. **Sequence duplicates.** SHA-256 of each body, header excluded (headers differ in the id only).
   Slots with equal hashes are one group.
3. **Bank duplicates.** SHA-256 of the first type-6 (VH) payload followed by the first type-7 (VB)
   payload of each archive.
4. **Titles.** Never invent one. A title needs a `source` and an `evidence` pointer; a track no
   source names stays `none`. Run `cargo test -p bof3player --features names` after an edit.
