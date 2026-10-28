# Changelog

All notable changes to `bof3player-capi` are recorded here. The API revision is
`BOF3PLAYER_API_VERSION` in the header.

## 0.1.0

First release of `bof3player-capi`.

- Package `bof3player-capi`; the library files are `libbof3player.so` and
  `libbof3player.a`.
- Every C name carries the `bof3player_` prefix and every constant the
  `BOF3PLAYER_` prefix; the header is `bof3player.h`.
- API revision 1.
- Catalog items expose ids and archive names; applications can associate
  their own display labels with the ids.
