// SPDX-License-Identifier: MIT
//! `player`: browse, search, play, render, export and verify the audio of a
//! *Breath of Fire III* disc or extracted tree.

mod audio;
mod commands;
mod pick;
mod tui;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::catalog::Naming;
use crate::names::Names;

pub use crate::options::Global;

const HELP_AFTER: &str = "\
Common:
  bof3player-tui                      open the terminal browser
  bof3player-gui                      open the window
  bof3player -i DISC music            every music archive
  bof3player play \"music:BGMOPN#0\"    play one track

Examples:
  bof3player -i \"Breath of Fire III (USA).cue\" list --kind music
  bof3player music --untitled
  bof3player render -o opening.wav \"music:BGMOPN#0\"

Every switch shows its default; --json prints JSON where a command supports it.";

#[derive(Parser, Debug)]
#[command(name = "bof3player", version, about = "PlayStation audio of Breath of Fire III", long_about = None,
    after_help = HELP_AFTER, allow_external_subcommands = true, args_conflicts_with_subcommands = false)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Item filters shared by `list`, `search` and `verify`.
#[derive(Args, Debug, Clone, Default)]
pub struct Filters {
    /// Only this kind: music, bank, sample, xa, cdda (default: every kind)
    #[arg(long, value_name = "KIND")]
    pub kind: Option<String>,
    /// Only this family, for example BGM or SCE_XA (default: every family)
    #[arg(long, value_name = "FAMILY")]
    pub family: Option<String>,
    /// Only items that have audio (default: off)
    #[arg(long)]
    pub playable: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Report the data source, every probe, and the audio device
    #[command(after_help = "Examples:\n  bof3player doctor\n  bof3player -i \"Breath of Fire III (USA).cue\" doctor")]
    Doctor,
    /// List items
    #[command(
        alias = "ls",
        after_help = "Examples:\n  bof3player list --kind music\n  bof3player list --kind xa --family BMAG_XA\n  bof3player list --limit 20"
    )]
    List {
        #[command(flatten)]
        filters: Filters,
        /// Show at most this many rows (default: every row)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Ranked search over ids, names, titles and families
    #[command(
        alias = "find",
        after_help = "Examples:\n  bof3player search battle\n  bof3player search --kind sample drum\n  bof3player search --playable boss"
    )]
    Search {
        query: String,
        #[command(flatten)]
        filters: Filters,
        /// Show at most this many rows
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Details of one item
    #[command(
        alias = "show",
        after_help = "Examples:\n  bof3player info \"music:BGMOPN#0\"\n  bof3player info xa:MAGIC00#ch9@12665\n  bof3player info bank:MAGIC004"
    )]
    Info { item: String },
    /// Play an item through the audio device
    #[command(
        after_help = "Examples:\n  bof3player play \"music:BGMOPN#0\"\n  bof3player play --loops 3 \"music:BGMOPN#0\"\n  bof3player play --forever \"music:BGMOPN#0\""
    )]
    Play {
        item: String,
        /// Repeat until interrupted (default: off)
        #[arg(long)]
        forever: bool,
    },
    /// Decode an item to WAV, FLAC, Ogg Vorbis or raw s16le
    #[command(
        alias = "save",
        after_help = "Examples:\n  bof3player render -o song.wav \"music:BGMOPN#0\"\n  bof3player render --format flac -o song.flac \"music:BGMOPN#0\"\n  bof3player render --format ogg --quality 6 -o song.ogg \"music:BGMOPN#0\"\n  bof3player render --all --out-dir out --kind music --format flac\n  bof3player --loops 3 render -o song.flac \"music:BGMOPN#0\"
FLAC/Ogg music without --loops renders the intro plus one loop and tags it."
    )]
    Render {
        /// Item id or query (omit with --all)
        item: Option<String>,
        /// Output file (default: the item id with the format's extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Output directory for --all (default: the export folder)
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Render every playable item matching the filters (default: off)
        #[arg(long)]
        all: bool,
        /// `wav`, `s16`, `flac` or `ogg`
        #[arg(long, default_value = "wav")]
        format: String,
        /// Ogg Vorbis quality, 0 (smallest) to 10 (best); 6 is about 192 kbps
        #[arg(long, default_value_t = crate::export::DEFAULT_OGG_QUALITY)]
        quality: f32,
        /// Stop after this many seconds (default: none, so the whole item)
        #[arg(long)]
        seconds: Option<f64>,
        #[command(flatten)]
        filters: Filters,
    },
    /// Export a sequence as MIDI or as its package, a sample as raw VAG blocks,
    /// or a bank as one VAB file
    #[command(
        after_help = "Examples:\n  bof3player export midi \"music:BGMOPN#0\" -o song.mid\n  bof3player export vag \"sample:BOSS001#3\" -o sample.vag\n  bof3player export vab \"bank:BOSS001\" -o bank.vab"
    )]
    Export {
        /// `midi`, `vag`, `seq` or `vab`
        what: String,
        item: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Render every playable item and classify what has sound
    #[command(
        alias = "check",
        after_help = "Examples:\n  bof3player verify --kind sample\n  bof3player verify --seconds 3 --limit 100\n  bof3player verify --kind xa"
    )]
    Verify {
        #[command(flatten)]
        filters: Filters,
        /// Render at most this many seconds of each item
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        /// Check at most this many items (default: every item)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Catalog totals per kind and family
    #[command(after_help = "Examples:\n  bof3player stats\n  bof3player stats --json")]
    Stats,
    /// List every music archive
    #[command(
        alias = "songs",
        after_help = "Examples:\n  bof3player music\n  bof3player music --untitled\n  bof3player music --all"
    )]
    Music {
        /// Include sequences with no audio (default: off)
        #[arg(long)]
        all: bool,
        /// Only sequences no source names (default: off)
        #[arg(long)]
        untitled: bool,
    },
    #[command(external_subcommand)]
    Query(Vec<OsString>),
}

/// Render every playable item that matches `query` as WAV files into `out_dir`, reading `disc`
/// (a .cue sheet or an extracted folder); returns the count of items that failed.
pub fn render_matching(disc: &Path, query: &str, out_dir: &Path, naming: &dyn Naming) -> Result<usize, String> {
    let cli = Cli::try_parse_from([OsString::from("bof3player"), "--input".into(), disc.into()])
        .map_err(|error| error.to_string())?;
    commands::render_matching(&cli.global, query, out_dir, naming)
}

/// `command` shown as `program`: the usage name, and the program name in every example.
fn named(command: clap::Command, program: &str) -> clap::Command {
    examples_for(command, program).bin_name(program)
}

/// Rewrite the program name in the examples of `command` and of every subcommand below it.
fn examples_for(command: clap::Command, program: &str) -> clap::Command {
    let rename = |text: &clap::builder::StyledStr| text.to_string().replace("bof3player", program);
    let after = command.get_after_help().map(rename);
    let subcommands = command.get_subcommands().map(|sub| sub.get_name().to_owned()).collect::<Vec<_>>();
    let mut command = command;
    if let Some(after) = after {
        command = command.after_help(after);
    }
    for name in subcommands {
        command = command.mut_subcommand(name, |sub| examples_for(sub, program));
    }
    command
}

/// The terminal browser's switches: the shared ones, no command.
#[derive(Parser, Debug)]
#[command(version, about = "Terminal browser for the PlayStation audio of Breath of Fire III", long_about = None,
    after_help = "Keys: Enter play  Space pause  e export menu  / search  ? help  q quit\nExample:\n  bof3player-tui -i \"Breath of Fire III (USA).cue\"")]
struct TuiCli {
    #[command(flatten)]
    global: Global,
}

/// Run the terminal browser; `naming` supplies the catalog's names.
pub fn tui_main(program: &str, args: Vec<OsString>, naming: &dyn Naming) -> ExitCode {
    let matches =
        TuiCli::command().bin_name(program).get_matches_from(std::iter::once(OsString::from(program)).chain(args));
    let cli = TuiCli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let titles = match cli.global.names.as_deref().map(Names::read) {
        Some(Ok(titles)) => Some(titles),
        Some(Err(error)) => {
            eprintln!("cannot read the names file: {error}; check the --names path");
            return ExitCode::from(2);
        }
        None => None,
    };
    let naming: &dyn Naming = titles.as_ref().map_or(naming, |titles| titles);
    match tui::run(&cli.global, naming) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{program}: {message}");
            ExitCode::from(2)
        }
    }
}

/// Run the player command line; `naming` supplies the catalog's names.
/// `program` is the name the user typed (`bof3player`, or a command that wraps it); help and errors show it.
pub fn main(program: &str, args: Vec<OsString>, naming: &dyn Naming) -> ExitCode {
    let command = named(Cli::command(), program);
    let matches = command.get_matches_from(std::iter::once(OsString::from(program)).chain(args));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let titles = match cli.global.names.as_deref().map(Names::read) {
        Some(Ok(titles)) => Some(titles),
        Some(Err(error)) => {
            eprintln!("cannot read the names file: {error}; check the --names path");
            return ExitCode::from(2);
        }
        None => None,
    };
    let naming: &dyn Naming = titles.as_ref().map_or(naming, |titles| titles);
    let result = match cli.command {
        None => {
            eprintln!(
                "usage: {program} <command> [options]  (try `{program} --help`)\n\
                 the terminal browser is `{program}-tui`, the window is `{program}-gui`"
            );
            return ExitCode::from(2);
        }
        Some(Command::Doctor) => commands::doctor(&cli.global, naming),
        Some(Command::List { filters, limit }) => commands::list(&cli.global, &filters, limit, naming),
        Some(Command::Search { query, filters, limit }) => {
            commands::search(&cli.global, &query, &filters, limit, naming)
        }
        Some(Command::Info { item }) => commands::info(&cli.global, &item, naming),
        Some(Command::Play { item, forever }) => commands::play(&cli.global, &item, cli.global.loops, forever, naming),
        Some(Command::Render { item, output, out_dir, all, format, quality, seconds, filters }) => commands::render(
            &cli.global,
            item.as_deref(),
            output,
            out_dir,
            all,
            &format,
            quality,
            cli.global.loops,
            seconds,
            &filters,
            naming,
        ),
        Some(Command::Export { what, item, output }) => commands::export(&cli.global, &what, &item, &output, naming),
        Some(Command::Verify { filters, seconds, limit }) => {
            commands::verify(&cli.global, &filters, seconds, limit, naming)
        }
        Some(Command::Stats) => commands::stats(&cli.global, naming),
        Some(Command::Music { all, untitled }) => commands::music(&cli.global, all, untitled, naming),
        Some(Command::Query(words)) => {
            let query = words.iter().map(|w| w.to_string_lossy()).collect::<Vec<_>>().join(" ");
            commands::play(&cli.global, &query, None, false, naming)
        }
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{program}: {message}");
            ExitCode::from(2)
        }
    }
}
