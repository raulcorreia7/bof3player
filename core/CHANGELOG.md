# Changelog

All notable changes to `bof3player` are recorded here. The project follows
[Semantic Versioning](https://semver.org/) and is distributed as a repository,
not through a package registry.

## 0.1.2

- Added content-based deduplication for library items.
- Removed the C API.
- Improved spectrum bands, colours and TUI responsiveness.
- Added grouping of same-content items with `--all`.
- Added `bof3-names` crate for bundled catalog names.
- Patched `taffy`/grid for a security update.
- Meter the signal before volume and mute.
- Report a missing GPU context instead of panicking.

## Unreleased

- Removed the C API.

## 0.1.0

First release of `bof3player`. The [SPU model evidence](../docs/spu-model.md#evidence)
records its hardware sources.

- `bof3player` binary: `doctor`, `list`, `search`, `info`, `play`, `render`,
  `export`, `verify`, `stats`, `music`, with runnable
  examples in every `--help`.
- `bof3player` library: catalog and archive access over the generic
  `psx-audio` crate, with SPU, XA and CD-DA engines.
- `catalog::Naming` supplies display names. `Library::open` uses `NoNaming`;
  `Library::open_with` accepts a naming adapter.
- MIT `LICENSE` and an SPDX header on every source file.
- C ABI: the `bof3player-capi` crate, `libbof3player.so`/`.a`, header
  `bof3player.h`, symbols `bof3player_*` and `BOF3PLAYER_*`, API revision 1.
