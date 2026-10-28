/* SPDX-License-Identifier: MIT */
/* Compile the public header alone: cc -std=c99 -Wall -Wextra -pedantic -Werror
 * -Ilibs/audio/capi/include -fsyntax-only libs/audio/capi/tests/header.c */
#include "bof3player.h"

int main(void) {
    return BOF3PLAYER_API_VERSION == 0;
}
