// SPDX-License-Identifier: MIT
//! The non-interactive commands.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crate::catalog::{Catalog, Filter, Item, Kind, Naming};
use crate::player::Loop;
use crate::save::{ExportFormat, ExportOptions, Save, Step};
use crate::{Library, Player, discover};
use serde_json::json;

use crate::cli::audio::{Deck, FileSource};
use crate::cli::{Filters, Global};

type Outcome = Result<ExitCode, String>;

/// Print one line to standard output. A closed pipe (`bof3player list | head`) ends the program
/// quietly; any other write error is reported and ends it with status 2.
macro_rules! say {
    () => { say!("") };
    ($($arg:tt)*) => { write_line(format_args!($($arg)*)) };
}

fn write_line(line: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    if let Err(error) = writeln!(out, "{line}").and_then(|()| out.flush()) {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!("bof3player: cannot write the output: {error}");
            std::process::exit(2);
        }
        std::process::exit(0);
    }
}

/// Open the game data named by the global options, leaking it so players can be `'static`.
pub fn open_library(global: &Global, naming: &dyn Naming) -> Result<&'static Library<'static>, String> {
    let discovery = discover::discover(global.dir.as_deref());
    if global.verbose {
        for c in &discovery.candidates {
            eprintln!("probe {:<28} {} {}", c.label, if c.accepted { "accepted" } else { "rejected" }, c.detail);
        }
    }
    if discovery.found.is_none() {
        return Err(not_found(&discovery));
    }
    let library = discovery.open_with(naming).map_err(|error| {
        format!("cannot open the game data: {}; run `bof3player doctor` to see every probe", error.message())
    })?;
    Ok(Box::leak(Box::new(library)))
}

/// What a failed search looked at and what to do next.
fn not_found(discovery: &discover::Discovery) -> String {
    let places = discovery.candidates.iter().map(|c| c.path.display().to_string()).collect::<Vec<_>>();
    let shown = places.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    let more = if places.len() > 4 { format!(" and {} more", places.len() - 4) } else { String::new() };
    format!(
        "no game data found; searched {shown}{more}. Pass -i <disc.cue or folder> or set BOF3_GAME_DIR; \
         `bof3player doctor` lists every place"
    )
}

pub fn filter_of<'a>(filters: &'a Filters) -> Result<Filter<'a>, String> {
    let kinds = match &filters.kind {
        Some(name) => vec![
            Kind::from_name(name).ok_or_else(|| format!("unknown kind {name}: use music, bank, sample, xa or cdda"))?,
        ],
        None => Vec::new(),
    };
    Ok(Filter { kinds, family: filters.family.as_deref(), playable_only: filters.playable })
}

pub use crate::deck::clock;

fn duration_text(item: &Item) -> String {
    item.duration.map_or_else(|| "  ?".to_string(), clock)
}

/// An exact id, else the best playable ranked match (reported on stderr).
pub fn resolve<'a>(library: &'a Library<'static>, query: &str) -> Result<&'a Item, String> {
    let catalog = library.catalog();
    if let Some(item) = catalog.find(query) {
        return Ok(item);
    }
    let hits = catalog.search(query, &Filter { playable_only: true, ..Filter::default() });
    match hits.first() {
        Some(best) => {
            eprintln!("player: \"{query}\" resolved to {}", best.id);
            Ok(best)
        }
        None => {
            let hint = catalog.search(query, &Filter::default());
            let list = hint.iter().take(5).map(|i| i.id.as_str()).collect::<Vec<_>>().join(", ");
            Err(format!(
                "no item matches \"{query}\"{}. Try `bof3player list` or a shorter query.",
                if list.is_empty() { String::new() } else { format!("; closest: {list}") }
            ))
        }
    }
}

fn item_json(item: &Item) -> serde_json::Value {
    json!({
        "id": item.id, "kind": item.kind.name(), "name": item.name, "title": item.title, "family": item.family,
        "detail": item.detail, "path": item.path, "index": item.index, "sub_index": item.sub_index,
        "duration": item.duration, "playable": item.playable, "same_as": item.same_as,
    })
}

fn print_rows(global: &Global, catalog: &Catalog, rows: &[&Item], total: usize) {
    if global.json {
        say!(
            "{}",
            serde_json::to_string_pretty(&rows.iter().map(|i| item_json(i)).collect::<Vec<_>>()).unwrap_or_default()
        );
        return;
    }
    for item in rows {
        let title = if item.title.is_empty() { String::new() } else { format!("{} · ", item.title) };
        let same = catalog.same_as_text(item).map_or_else(String::new, |text| format!("  same as {text}"));
        say!("{:<6} {:<28} {:>8}  {}{}{}", item.kind.name(), item.id, duration_text(item), title, item.name, same);
    }
    if rows.len() < total && !global.quiet {
        eprintln!("({} of {total} shown)", rows.len());
    }
}

pub fn list(global: &Global, filters: &Filters, limit: Option<usize>, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let filter = filter_of(filters)?;
    let hits = library.catalog().search("", &filter);
    let shown = &hits[..limit.map_or(hits.len(), |l| l.min(hits.len()))];
    print_rows(global, library.catalog(), shown, hits.len());
    Ok(ExitCode::SUCCESS)
}

pub fn search(global: &Global, query: &str, filters: &Filters, limit: usize, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let filter = filter_of(filters)?;
    let hits = library.catalog().search(query, &filter);
    let shown = &hits[..limit.min(hits.len())];
    print_rows(global, library.catalog(), shown, hits.len());
    Ok(if hits.is_empty() { ExitCode::from(1) } else { ExitCode::SUCCESS })
}

pub fn info(global: &Global, query: &str, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let item = resolve(library, query)?;
    if global.json {
        say!("{}", serde_json::to_string_pretty(&item_json(item)).unwrap_or_default());
        return Ok(ExitCode::SUCCESS);
    }
    say!("id        {}\nkind      {}\nname      {}", item.id, item.kind.name(), item.name);
    if !item.title.is_empty() {
        say!("title     {}", item.title);
    }
    if let Some(same) = library.catalog().same_as_text(item) {
        say!("same as   {same}");
    }
    say!("family    {}\nduration  {}\ndetail    {}", item.family, duration_text(item), item.detail);
    say!(
        "playable  {}\nsource    {}\nindex     {}\nsub index {}",
        if item.playable { "yes" } else { "no" },
        item.path,
        item.index,
        item.sub_index
    );
    if item.playable
        && matches!(item.kind, Kind::Music)
        && let Ok(player) = library.player(item)
    {
        match player.loop_window() {
            Some((a, b)) => say!("loop win  {} to {}", clock(a), clock(b)),
            None => say!("loop win  none (the item plays straight through)"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `m:ss` for a music row, or `--:--` when the catalog does not know.
pub(crate) fn music_length(seconds: Option<f64>) -> String {
    seconds.filter(|s| s.is_finite() && *s >= 0.0).map_or_else(
        || "--:--".to_string(),
        |s| {
            let total = s.round() as u64;
            format!("{}:{:02}", total / 60, total % 60)
        },
    )
}

pub fn music(global: &Global, all: bool, untitled: bool, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let filter = Filter { kinds: vec![Kind::Music], ..Filter::default() };
    let mut archives: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for item in library.catalog().search("", &filter) {
        archives.entry(&item.path).or_default().push(item);
    }
    let selected = |item: &Item| (all || item.playable) && (!untitled || item.title.is_empty());
    let any = archives.values().any(|items| items.iter().copied().any(selected));
    if global.json {
        let rows: Vec<_> = archives
            .iter()
            .map(|(path, items)| {
                let playable = items.iter().filter(|i| i.playable).count();
                let titled = items.iter().filter(|i| !i.title.is_empty()).count();
                json!({
                    "path": path,
                    "playable": playable,
                    "total": items.len(),
                    "titled": titled,
                    "items": items.iter().copied().filter(|i| selected(i)).map(item_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        say!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
    } else {
        for (path, items) in &archives {
            let playable = items.iter().filter(|i| i.playable).count();
            let titled = items.iter().filter(|i| !i.title.is_empty()).count();
            say!("{path}  ({playable}/{} playable, {titled} titled)", items.len());
            for item in items.iter().copied().filter(|i| selected(i)) {
                let title = if item.title.is_empty() { "-" } else { item.title.as_str() };
                let same = library.catalog().same_as_text(item).map_or_else(String::new, |t| format!("  same as {t}"));
                say!(
                    "  #{}   {}  {title}{same}{}",
                    item.index,
                    music_length(item.duration),
                    if item.playable { "" } else { "  [no audio]" }
                );
            }
        }
    }
    Ok(if untitled && !any { ExitCode::from(1) } else { ExitCode::SUCCESS })
}

pub fn stats(global: &Global, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let items = library.catalog().items();
    let mut kinds: BTreeMap<Kind, (usize, usize, f64)> = BTreeMap::new();
    let mut families: BTreeMap<&str, usize> = BTreeMap::new();
    for item in items {
        let e = kinds.entry(item.kind).or_default();
        e.0 += 1;
        e.1 += usize::from(!item.playable);
        e.2 += item.duration.unwrap_or(0.0).max(0.0);
        *families.entry(&item.family).or_default() += 1;
    }
    if global.json {
        let k: Vec<_> = kinds
            .iter()
            .map(|(k, v)| json!({"kind": k.name(), "items": v.0, "no_audio": v.1, "seconds": v.2}))
            .collect();
        say!("{}", json!({"items": items.len(), "kinds": k, "families": families}));
        return Ok(ExitCode::SUCCESS);
    }
    say!("{} items\n\n{:<8} {:>7} {:>9} {:>10}", items.len(), "kind", "items", "no audio", "duration");
    for (kind, (n, silent, secs)) in &kinds {
        say!("{:<8} {n:>7} {silent:>9} {:>10}", kind.name(), clock(*secs));
    }
    let mut top: Vec<_> = families.into_iter().collect();
    top.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    say!("\nlargest families");
    for (family, n) in top.iter().take(10) {
        say!("  {family:<12} {n}");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn doctor(global: &Global, naming: &dyn Naming) -> Outcome {
    let discovery = discover::discover(global.dir.as_deref());
    say!("player {}\n", crate::VERSION);
    match &discovery.found {
        Some((kind, path)) => say!("source:  {kind:?} {}", path.display()),
        None => say!("source:  none found"),
    }
    say!("\ncandidates ({}):", discovery.candidates.len());
    for c in &discovery.candidates {
        say!("  {:<4} {:<30} {}  {}", if c.accepted { "yes" } else { "no" }, c.label, c.path.display(), c.detail);
    }
    let deck = Deck::new(global.mute);
    say!("\naudio:   {}", deck.device);
    if discovery.found.is_none() {
        return Ok(ExitCode::from(1));
    }
    let library = discovery.open_with(naming).map_err(|error| {
        format!("cannot open the game data: {}; run `bof3player doctor` to see every probe", error.message())
    })?;
    let mut counts = BTreeMap::new();
    for item in library.catalog().items() {
        *counts.entry(item.kind).or_insert(0usize) += 1;
    }
    say!("\ncatalog: {} items", library.catalog().len());
    for (kind, n) in counts {
        say!("  {:<8}{n}", kind.name());
    }
    Ok(ExitCode::SUCCESS)
}

pub fn start<'a>(global: &Global, library: &'a Library<'static>, item: &Item) -> Result<Player<'a>, String> {
    library.player_with(item, global.player_options()).map_err(|e| e.to_string())
}

/// A file the player can decode itself, when `query` names one.
fn audio_file(query: &str) -> Option<PathBuf> {
    let path = Path::new(query);
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    (matches!(extension.as_str(), "wav" | "flac" | "ogg") && path.is_file()).then(|| path.to_path_buf())
}

pub fn play(global: &Global, query: &str, loops: Option<u32>, forever: bool, naming: &dyn Naming) -> Outcome {
    if let Some(path) = audio_file(query) {
        let source = FileSource::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let deck = Deck::new(global.mute);
        eprintln!("{}  [{}]", path.display(), deck.device);
        deck.load(source);
        return wait_deck(&deck);
    }
    let library = open_library(global, naming)?;
    let item = resolve(library, query)?;
    let mut player = start(global, library, item)?;
    if forever {
        player.set_loop(Loop::Forever);
    } else if let Some(n) = loops {
        player.set_loop(Loop::Times(n.max(1) - 1));
    }
    let deck = Deck::new(global.mute);
    eprintln!("{}  [{}]  {}", item.id, item.name, deck.device);
    deck.load(player);
    wait_deck(&deck)
}

/// Show the deck's position until it finishes.
fn wait_deck(deck: &Deck) -> Outcome {
    let begin = Instant::now();
    while !deck.finished() {
        if let Some(error) = deck.take_error() {
            eprintln!("player: audio stream: {error}");
        }
        let (pos, dur) = deck.times();
        eprint!("\r{} / {}   ", clock(pos), clock(dur));
        std::thread::sleep(Duration::from_millis(100));
        if begin.elapsed() > Duration::from_secs(60 * 60 * 6) {
            break;
        }
    }
    eprintln!();
    Ok(ExitCode::SUCCESS)
}

/// How `render` writes one item.
struct RenderStyle<'a> {
    format: ExportFormat,
    quality: f32,
    loops: Option<u32>,
    seconds: Option<f64>,
    fade: f64,
    /// The format name for error messages.
    name: &'a str,
}

fn render_item(
    global: &Global,
    library: &'static Library<'static>,
    item: &Item,
    path: &Path,
    style: &RenderStyle,
) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let options = ExportOptions {
        loops: style.loops,
        fade: style.fade,
        format: style.format,
        quality: style.quality,
        seconds: style.seconds,
    };
    let mut save = Save::start_to(library, item, global.player_options(), path.to_path_buf(), options)
        .map_err(|e| format!("{}: {e}", item.id))?;
    loop {
        match save.step() {
            Step::Progress => {}
            Step::Done => break,
            Step::Failed(error) => return Err(format!("{} ({}): {error}", item.id, style.name)),
        }
    }
    eprintln!(
        "wrote {}: {:.1}s, {} Hz, {} ch",
        crate::save::absolute(path).display(),
        save.rendered(),
        save.sample_rate(),
        save.channels()
    );
    Ok(())
}

use crate::save::file_stem;

/// The output format named on the command line.
fn parse_format(name: &str, quality: f32) -> Result<ExportFormat, String> {
    match name {
        "wav" => Ok(ExportFormat::Wav),
        "s16" => Ok(ExportFormat::Raw),
        "flac" => Ok(ExportFormat::Flac),
        "ogg" if crate::export::OGG_QUALITY_RANGE.contains(&quality) => Ok(ExportFormat::Ogg),
        "ogg" => Err(format!(
            "Ogg quality {quality} is outside 0-10: pass --quality 0 (smallest) to 10 (best), for example --quality 6."
        )),
        other => Err(format!("unknown format {other}: use wav, s16, flac or ogg.")),
    }
}

/// Render every playable item matching `query` and `filters` into `dir`; returns how many failed.
fn render_all(
    global: &Global,
    library: &'static Library<'static>,
    query: &str,
    dir: &Path,
    style: &RenderStyle,
    filters: &Filters,
) -> Result<usize, String> {
    let mut filter = filter_of(filters)?;
    filter.playable_only = true;
    let mut failed = 0;
    for hit in library.catalog().search(query, &filter) {
        let path = dir.join(format!("{}.{}", file_stem(&hit.id), style.format.extension()));
        if let Err(e) = render_item(global, library, hit, &path, style) {
            eprintln!("player: {e}");
            failed += 1;
        }
    }
    Ok(failed)
}

/// Render every playable item matching `query` as WAV into `dir`; returns how many failed.
pub fn render_matching(global: &Global, query: &str, dir: &Path, naming: &dyn Naming) -> Result<usize, String> {
    let library = open_library(global, naming)?;
    let style = RenderStyle {
        format: ExportFormat::Wav,
        quality: crate::export::DEFAULT_OGG_QUALITY,
        loops: global.loops,
        seconds: None,
        fade: global.fade.unwrap_or(0.0),
        name: "wav",
    };
    render_all(global, library, query, dir, &style, &Filters::default())
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    global: &Global,
    item: Option<&str>,
    output: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    all: bool,
    format: &str,
    quality: f32,
    loops: Option<u32>,
    seconds: Option<f64>,
    filters: &Filters,
    naming: &dyn Naming,
) -> Outcome {
    let style = RenderStyle {
        format: parse_format(format, quality)?,
        quality,
        loops,
        seconds,
        fade: global.fade.unwrap_or(0.0),
        name: format,
    };
    let library = open_library(global, naming)?;
    if all {
        let dir = out_dir.ok_or("render --all needs an output folder: pass --out-dir DIR.")?;
        let failed = render_all(global, library, item.unwrap_or(""), &dir, &style, filters)?;
        return Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) });
    }
    let query = item.ok_or("render needs an item: pass an id or query, or --all.")?;
    let hit = resolve(library, query)?;
    let path = match (output, out_dir) {
        (Some(_), Some(_)) => return Err("render takes -o FILE or --out-dir DIR, not both: choose one.".into()),
        (Some(p), None) => p,
        (None, Some(d)) => d.join(format!("{}.{}", file_stem(&hit.id), style.format.extension())),
        (None, None) => return Err("render needs an output: pass -o FILE or --out-dir DIR.".into()),
    };
    if !hit.playable {
        eprintln!("player: {} carries no audio - {}", hit.id, hit.detail);
        return Ok(ExitCode::from(2));
    }
    render_item(global, library, hit, &path, &style)?;
    Ok(ExitCode::SUCCESS)
}

pub fn export(global: &Global, what: &str, query: &str, output: &Path, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let item = resolve(library, query)?;
    let bytes = match what {
        "midi" => library.export_midi(item),
        "vag" => library.export_vag(item),
        "seq" => library.export_sequence(item),
        "vab" => library.export_bank(item),
        other => {
            return Err(format!(
                "cannot export {other}: use midi, vag, seq or vab, for example `bof3player export midi ITEM -o song.mid`."
            ));
        }
    }
    .map_err(|e| e.to_string())?;
    std::fs::File::create(output)
        .and_then(|mut f| f.write_all(&bytes))
        .map_err(|e| format!("{}: {e}", output.display()))?;
    if !global.quiet {
        eprintln!("wrote {} ({} bytes)", crate::save::absolute(output).display(), bytes.len());
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Default)]
struct Tally {
    sound: usize,
    silent: usize,
    empty: usize,
    failed: usize,
}

pub fn verify(global: &Global, filters: &Filters, seconds: f64, limit: Option<usize>, naming: &dyn Naming) -> Outcome {
    let library = open_library(global, naming)?;
    let filter = filter_of(filters)?;
    let hits = library.catalog().search("", &filter);
    let mut tallies: BTreeMap<Kind, Tally> = BTreeMap::new();
    for item in hits.iter().take(limit.unwrap_or(usize::MAX)) {
        let tally = tallies.entry(item.kind).or_default();
        if !item.playable || item.kind == Kind::Bank {
            tally.empty += 1;
            continue;
        }
        let verdict = (|| -> Result<bool, String> {
            let mut player = start(global, library, item)?;
            let channels = player.channels() as usize;
            let cap = (seconds * f64::from(player.sample_rate())) as usize;
            let mut chunk = vec![0i16; 4096 * channels];
            let (mut frames_total, mut loud) = (0usize, false);
            while frames_total < cap {
                let frames = player.render_i16(&mut chunk).map_err(|e| e.to_string())?;
                if frames == 0 {
                    break;
                }
                frames_total += frames;
                loud |= chunk[..frames * channels].iter().any(|&s| s != 0);
            }
            Ok(loud)
        })();
        match verdict {
            Ok(true) => tally.sound += 1,
            Ok(false) => {
                tally.silent += 1;
                say!("silent  {}", item.id);
            }
            Err(e) => {
                tally.failed += 1;
                say!("failed  {}  {e}", item.id);
            }
        }
    }
    say!("\n{:<8} {:>7} {:>7} {:>7} {:>7}", "kind", "sound", "silent", "empty", "failed");
    let mut bad = false;
    for (kind, t) in &tallies {
        say!("{:<8} {:>7} {:>7} {:>7} {:>7}", kind.name(), t.sound, t.silent, t.empty, t.failed);
        bad |= t.silent + t.failed > 0;
    }
    Ok(if bad { ExitCode::from(1) } else { ExitCode::SUCCESS })
}
