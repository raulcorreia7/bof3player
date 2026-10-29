// SPDX-License-Identifier: MIT
//! The BoF3 catalog names: the game's titles, labels and bank names.
//!
//! The data is `data/bof3-names.json`, in the schema `bof3player::names` reads (see
//! `data/README.md`); this crate embeds that file for library callers. The
//! `bof3player` library takes any [`Naming`](bof3player::catalog::Naming) and also reads the file
//! at run time.

use bof3player::names::Names;
use std::sync::LazyLock;

static GAME: LazyLock<Names> = LazyLock::new(|| {
    Names::from_json(include_str!("../../data/bof3-names.json")).expect("data/bof3-names.json is a valid names file")
});

/// The game's bundled names.
#[must_use]
pub fn game_names() -> &'static Names {
    &GAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use bof3player::catalog::Naming;
    use bof3player::names::Source;

    #[test]
    fn bundled_names_answer_every_lookup() {
        let game = game_names();
        assert_eq!(game.music_title("BGMOPN", 0), Some("Opening"));
        assert_eq!(game.music_title("BGMOPN", 999), None);
        assert_eq!(game.music_title("DEMO", 0), Some("Opening"));
        assert_eq!(game.music_copy("DEMO#0"), Some("BGMOPN#0"));
        assert_eq!(game.music_sound("BGM000#0"), Some(0));
        assert_eq!(game.bank_name("BIN/BENEMY/ENEMY001.EMI"), Some("Area 001 enemy bank (no enemy records)"));
        assert_eq!(game.bank_duplicate("AREA001"), Some("AREA000"));
        assert_eq!(game.magic_ability(160), Some("Nue Stomp"));
        assert_eq!(game.xa_span("xa:S_XA00#ch0@0"), Some("S_XA00 cue 0 (game 0x0000)"));
    }

    #[test]
    fn quoted_titles_survive() {
        assert_eq!(game_names().music_title("BGMEND", 1), Some("\"Fin\""));
    }

    #[test]
    fn titles_state_their_source() {
        for (id, entry) in game_names().music() {
            if entry.title.is_some() {
                assert_ne!(entry.source, Source::None, "{id} has a title and no source");
                assert!(entry.evidence.is_some(), "{id} has a title and no evidence");
            }
        }
    }
}
