# C API

`bof3player-capi` (`capi/`) exposes the library to C: opaque library and player handles, status
codes, a per-thread last-error message, and one header, [`bof3player.h`](../capi/include/bof3player.h).
Every function starts with `bof3player_` and every constant with `BOF3PLAYER_`.

## Build

From the repository root:

```sh
cargo build --release -p bof3player-capi
```

This writes `libbof3player.so` and `libbof3player.a` into `target/release`. Link the static
library with `-lpthread -ldl -lm`.

## Example

```c
#include <stdio.h>
#include <stdint.h>
#include "bof3player.h"

int main(void) {
    bof3player_library *library;
    if (bof3player_library_open_path("/path/to/game.cue", &library) != BOF3PLAYER_OK) {
        fprintf(stderr, "%s\n", bof3player_last_error_message());
        return 1;
    }

    bof3player_item item;
    if (bof3player_catalog_find(library, "music:ITEM#0", &item) != BOF3PLAYER_OK) {
        fprintf(stderr, "%s\n", bof3player_last_error_message());
        bof3player_library_free(library);
        return 1;
    }

    bof3player_player *player;
    if (bof3player_player_create(library, item.catalog_index, NULL, &player) != BOF3PLAYER_OK) {
        fprintf(stderr, "%s\n", bof3player_last_error_message());
        bof3player_library_free(library);
        return 1;
    }

    int16_t pcm[1024 * 2];
    size_t frames = 0;
    while (bof3player_player_render_i16(player, pcm, 1024, &frames) == BOF3PLAYER_OK && frames > 0) {
        /* pcm holds frames * channels interleaved samples. Feed your audio device. */
    }

    bof3player_player_free(player);
    bof3player_library_free(library);
    return 0;
}
```

`music:ITEM#0` is a placeholder. Get real ids with `bof3player_catalog_count` and
`bof3player_catalog_item`.

## Opening data

| Function                            | Source                                                   |
| ----------------------------------- | -------------------------------------------------------- |
| `bof3player_library_open_path`      | An extracted folder, or a `.cue`/`.bin` disc image.      |
| `bof3player_library_open_memory`    | Files you already hold in memory.                        |
| `bof3player_library_open_callbacks` | Your own file system, through `list`, `size` and `read`. |

Memory files are borrowed. Keep them alive until the library and every player made from it are
freed.

## Threads and errors

A `bof3player_library` is immutable after opening and can be shared. Use a `bof3player_player` from
one thread at a time, and render from your audio thread. A player keeps its library alive.
`bof3player_last_error_message` is per thread. `bof3player_api_version` returns
`BOF3PLAYER_API_VERSION`.

## Header

`include/bof3player.h` is generated from `capi/src/lib.rs`. Change the Rust declarations and
regenerate it. Do not edit the header by hand.

```sh
cd capi
cbindgen --config cbindgen.toml --crate bof3player-capi --output include/bof3player.h
```

## Smoke test

`capi/tests/smoke.c` checks the error paths, and with a game path also the catalog, rendering,
exports and seeking. From `capi`:

```sh
cc tests/smoke.c -Iinclude ../target/release/libbof3player.a -lpthread -ldl -lm -o smoke
./smoke                      # error paths only
./smoke /path/to/game.cue    # catalog, render, exports and seek
```
