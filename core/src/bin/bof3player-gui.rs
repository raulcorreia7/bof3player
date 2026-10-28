// SPDX-License-Identifier: MIT
//! The desktop window: browse and play the game's audio.

use std::process::ExitCode;

fn main() -> ExitCode {
    bof3player::gui::main(std::env::args_os().skip(1).collect())
}
