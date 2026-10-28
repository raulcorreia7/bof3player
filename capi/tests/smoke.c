/* SPDX-License-Identifier: MIT */
/* C smoke test. Build it from the capi folder:
 *   cc tests/smoke.c -Iinclude <target>/libbof3player.a -lpthread -ldl -lm -o smoke
 * Run `./smoke [game-dir-or-cue]`. With no argument it runs only the error paths. */
#include <stdio.h>
#include <string.h>
#include "bof3player.h"

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #c); return 1; } } while (0)

int main(int argc, char **argv) {
    CHECK(bof3player_api_version() == BOF3PLAYER_API_VERSION);
    CHECK(strlen(bof3player_version()) > 0);
    bof3player_library *lib = NULL;
    CHECK(bof3player_library_open_path(NULL, &lib) == BOF3PLAYER_ERR_INVALID);
    CHECK(strlen(bof3player_last_error_message()) > 0);
    CHECK(bof3player_library_open_path("/definitely/not/here", &lib) != BOF3PLAYER_OK);
    CHECK(bof3player_catalog_count(NULL) == 0);
    bof3player_bytes none = {0, 0};
    CHECK(bof3player_export_wav(NULL, 0, 0, &none) == BOF3PLAYER_ERR_INVALID);
    CHECK(bof3player_export_flac(NULL, 0, 0, NULL, &none) == BOF3PLAYER_ERR_INVALID);
    CHECK(bof3player_export_ogg(NULL, 0, 0, 6.0f, NULL, &none) == BOF3PLAYER_ERR_INVALID);
    bof3player_bytes_free(NULL);
    bof3player_library_free(NULL);
    if (argc < 2) { puts("ok (no data)"); return 0; }

    CHECK(bof3player_library_open_path(argv[1], &lib) == BOF3PLAYER_OK);
    size_t count = bof3player_catalog_count(lib);
    CHECK(count > 0);
    bof3player_item item;
    size_t index = 0;
    for (; index < count; index++) {
        CHECK(bof3player_catalog_item(lib, index, &item) == BOF3PLAYER_OK);
        if (item.playable) break;
    }
    CHECK(index < count);
    bof3player_bytes wav = {0, 0}, twice = {0, 0}, flac = {0, 0}, ogg = {0, 0};
    CHECK(bof3player_export_wav(lib, item.catalog_index, 0, &wav) == BOF3PLAYER_OK);
    CHECK(wav.len > 44 && memcmp(wav.data, "RIFF", 4) == 0);
    CHECK(bof3player_export_wav(lib, item.catalog_index, 2, &twice) == BOF3PLAYER_OK);
    CHECK(twice.len > wav.len);
    CHECK(bof3player_export_flac(lib, item.catalog_index, 0, NULL, &flac) == BOF3PLAYER_OK);
    CHECK(flac.len > 4 && memcmp(flac.data, "fLaC", 4) == 0);
    CHECK(bof3player_export_ogg(lib, item.catalog_index, 0, 6.0f, NULL, &ogg) == BOF3PLAYER_OK);
    CHECK(ogg.len > 4 && memcmp(ogg.data, "OggS", 4) == 0);
    bof3player_bytes_free(&wav);
    bof3player_bytes_free(&twice);
    bof3player_bytes_free(&flac);
    bof3player_bytes_free(&ogg);
    bof3player_player *player = NULL;
    CHECK(bof3player_player_create(lib, item.catalog_index, NULL, &player) == BOF3PLAYER_OK);
    bof3player_library_free(lib); /* players keep the library alive */
    CHECK(bof3player_player_channels(player) == 2);
    short pcm[1024 * 2];
    size_t got = 0, total = 0;
    int loud = 0;
    while (bof3player_player_render_i16(player, pcm, 1024, &got) == BOF3PLAYER_OK && got > 0) {
        total += got;
        for (size_t i = 0; i < got * 2; i++) if (pcm[i] > 1000 || pcm[i] < -1000) loud = 1;
    }
    CHECK(loud && total > 44100);
    CHECK(bof3player_player_seek(player, 1.0) == BOF3PLAYER_OK);
    bof3player_player_free(player);
    printf("ok: %zu frames\n", total);
    return 0;
}
