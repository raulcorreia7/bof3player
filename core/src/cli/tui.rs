// SPDX-License-Identifier: MIT
//! The terminal browser: a tree of kinds, families and items on the left, the
//! selected item's details on the right, and a now-playing panel with position,
//! loop window, level meters and a spectrum.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crate::Library;
use crate::catalog::{Filter, Item, Kind};
use crate::player::Loop;
use crate::theme;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph, Wrap};

use crate::catalog::Naming;
use crate::cli::Global;
use crate::cli::audio::Deck;
use crate::cli::commands::{clock, music_length, open_library};
use crate::save::{
    ExportFormat, ExportOptions, Format, OggQuality, Save, Step, absolute, export_failed, saved_message,
};
use crate::spectrum::Spectrum;

/// Frame interval of the extract browser's loop.
pub const TICK: Duration = Duration::from_millis(50);
/// Frame interval of the library browser while something moves: about 60 frames per second.
const FRAME: Duration = Duration::from_millis(16);
/// Redraw interval of the library browser when nothing moves.
const IDLE_FRAME: Duration = Duration::from_millis(100);
/// Seconds a seek key moves.
pub const SEEK_STEP: f64 = 5.0;
/// Volume change per key press.
pub const VOLUME_STEP: f32 = 0.05;
/// Most search hits shown.
const MAX_HITS: usize = 5000;
/// Levels fall this much per tick.
pub const METER_FALL: f32 = 0.06;
/// Levels fall this much per second, the rate `METER_FALL` gives at `TICK`.
const METER_FALL_PER_SECOND: f32 = METER_FALL / TICK.as_secs_f32();
/// Bands of the spectrum analyzer.
pub const SPECTRUM_BANDS: usize = 48;

/// The open export menu: the item, its supported formats, their target files
/// and how many times a looping track plays.
struct ExportMenu {
    item: usize,
    selected: usize,
    rows: Vec<(Format, PathBuf)>,
    loops: Option<u32>,
    quality: OggQuality,
}

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;

#[derive(Clone, PartialEq, Eq)]
enum Row {
    Kind(Kind),
    Family(Kind, String),
    Item(usize, usize),
}

#[derive(Default)]
struct Family {
    items: Vec<usize>,
    playable: usize,
}

struct App {
    library: &'static Library<'static>,
    global: Global,
    /// Set by `E`; the run loop then shows the folder picker.
    choose_folder: bool,
    deck: Deck,
    groups: BTreeMap<Kind, BTreeMap<String, Family>>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    state: ListState,
    query: String,
    typing: bool,
    playable_only: bool,
    playable: usize,
    current: Option<usize>,
    looping: bool,
    continuous: bool,
    message: String,
    help: bool,
    export: Option<ExportMenu>,
    save: Option<Save>,
    meters: (f32, f32),
    spectrum: Spectrum,
    last_tick: Instant,
    quit: bool,
    show_all_dupes: bool,
}

fn kind_key(kind: Kind) -> String {
    kind.name().to_string()
}

fn family_key(kind: Kind, family: &str) -> String {
    format!("{}/{family}", kind.name())
}

/// The heading of a kind with its glyph.
fn label(kind: Kind) -> String {
    let glyph = match kind {
        Kind::Music => "♫",
        Kind::Bank => "▤",
        Kind::Sample => "◈",
        Kind::Xa => "≋",
        Kind::Cdda => "◉",
    };
    format!("{glyph} {}", kind.label())
}

impl App {
    fn new(library: &'static Library<'static>, global: &Global) -> Self {
        let mut groups: BTreeMap<Kind, BTreeMap<String, Family>> = BTreeMap::new();
        for (index, item) in library.catalog().items().iter().enumerate() {
            let family = groups.entry(item.kind).or_default().entry(item.family.clone()).or_default();
            family.items.push(index);
            if item.playable {
                family.playable += 1;
            }
        }
        let mut expanded = HashSet::new();
        if let Some(families) = groups.get(&Kind::Music) {
            expanded.insert(kind_key(Kind::Music));
            for family in families.keys() {
                expanded.insert(family_key(Kind::Music, family));
            }
        }
        let mut app = Self {
            library,
            global: global.clone(),
            choose_folder: false,
            deck: Deck::new(global.mute),
            groups,
            expanded,
            rows: Vec::new(),
            state: ListState::default(),
            query: String::new(),
            typing: false,
            playable_only: true,
            playable: library.catalog().playable_count(),
            current: None,
            looping: true,
            continuous: true,
            message: String::new(),
            help: false,
            export: None,
            save: None,
            meters: (0.0, 0.0),
            spectrum: Spectrum::new(SPECTRUM_BANDS),
            last_tick: Instant::now(),
            quit: false,
            show_all_dupes: false,
        };
        app.rebuild();
        let first = app.rows.iter().position(|row| matches!(row, Row::Item(..))).unwrap_or(0);
        app.state.select(if app.rows.is_empty() { None } else { Some(first) });
        app
    }

    fn item(&self, index: usize) -> &'static Item {
        &self.library.catalog().items()[index]
    }

    fn selected_row(&self) -> Option<&Row> {
        self.state.selected().and_then(|i| self.rows.get(i))
    }

    fn selected_item(&self) -> Option<usize> {
        match self.selected_row() {
            Some(Row::Item(index, _)) => Some(*index),
            _ => None,
        }
    }

    fn visible(&self, item: &Item) -> bool {
        !self.playable_only || item.playable
    }

    fn rebuild(&mut self) {
        let keep = self.selected_row().cloned();
        self.rows.clear();
        let catalog = self.library.catalog();
        let groups = catalog.groups();
        use std::collections::HashSet;
        let mut seen_canon: HashSet<String> = HashSet::new();
        if !self.query.is_empty() {
            let filter = Filter { playable_only: self.playable_only, ..Filter::default() };
            for hit in catalog.search(&self.query, &filter).into_iter().take(MAX_HITS) {
                if let Some(index) = catalog.index_of(&hit.id) {
                    if !self.show_all_dupes {
                        if let Some(canon) = groups.canonical_of(&hit.id) {
                            if canon != hit.id {
                                continue;
                            }
                            if seen_canon.contains(&canon) {
                                continue;
                            }
                            seen_canon.insert(canon);
                        } else if groups.group_members(&hit.id).is_some()
                            && groups.canonical_of(&hit.id).as_deref() == Some(hit.id.as_str())
                        {
                            if seen_canon.contains(&hit.id) {
                                continue;
                            }
                            seen_canon.insert(hit.id.clone());
                        }
                    }
                    self.rows.push(Row::Item(index, 0));
                }
            }
        } else {
            for kind in Kind::LIST {
                let Some(families) = self.groups.get(&kind) else { continue };
                self.rows.push(Row::Kind(kind));
                if !self.expanded.contains(&kind_key(kind)) {
                    continue;
                }
                for (family, group) in families {
                    self.rows.push(Row::Family(kind, family.clone()));
                    if !self.expanded.contains(&family_key(kind, family)) {
                        continue;
                    }
                    for &index in &group.items {
                        let item = self.item(index);
                        if !self.visible(item) {
                            continue;
                        }
                        if !self.show_all_dupes {
                            if let Some(canon) = groups.canonical_of(&item.id) {
                                if canon != item.id {
                                    continue;
                                }
                                if seen_canon.contains(&canon) {
                                    continue;
                                }
                                seen_canon.insert(canon);
                            } else if groups.group_members(&item.id).is_some()
                                && groups.canonical_of(&item.id).as_deref() == Some(item.id.as_str())
                            {
                                if seen_canon.contains(&item.id) {
                                    continue;
                                }
                                seen_canon.insert(item.id.clone());
                            }
                        }
                        match item.id.split_once('@') {
                            None => self.rows.push(Row::Item(index, 0)),
                            Some((parent, _)) if self.expanded.contains(parent) => self.rows.push(Row::Item(index, 1)),
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        let at = keep.and_then(|k| self.rows.iter().position(|r| *r == k)).unwrap_or(0);
        self.state.select(if self.rows.is_empty() { None } else { Some(at.min(self.rows.len() - 1)) });
    }

    fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let at = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((at + delta).clamp(0, self.rows.len() as isize - 1) as usize));
    }

    fn toggle(&mut self, open: Option<bool>) {
        let key = match self.selected_row() {
            Some(Row::Kind(k)) => kind_key(*k),
            Some(Row::Family(k, f)) => family_key(*k, f),
            Some(Row::Item(i, 0)) if self.item(*i).kind == Kind::Xa && !self.item(*i).id.contains('@') => {
                self.item(*i).id.clone()
            }
            _ => return,
        };
        let now = self.expanded.contains(&key);
        match open {
            Some(true) if now => return,
            Some(false) if !now => return,
            _ => {}
        }
        if now {
            self.expanded.remove(&key);
        } else {
            self.expanded.insert(key);
        }
        self.rebuild();
    }

    fn collapse_or_parent(&mut self) {
        let at = self.state.selected().unwrap_or(0);
        let is_open = match self.selected_row() {
            Some(Row::Kind(k)) => self.expanded.contains(&kind_key(*k)),
            Some(Row::Family(k, f)) => self.expanded.contains(&family_key(*k, f)),
            Some(Row::Item(i, 0)) => self.expanded.contains(&self.item(*i).id),
            _ => false,
        };
        if is_open {
            self.toggle(Some(false));
            return;
        }
        if self.query.is_empty()
            && let Some(parent) =
                (0..at).rev().find(|&j| matches!(self.rows[j], Row::Kind(_) | Row::Family(..)) && j < at)
        {
            let want_family = matches!(self.rows[at], Row::Item(..));
            let target = (0..at).rev().find(|&j| {
                if want_family { matches!(self.rows[j], Row::Family(..)) } else { matches!(self.rows[j], Row::Kind(_)) }
            });
            self.state.select(Some(target.unwrap_or(parent)));
        }
    }

    fn play_index(&mut self, index: usize) {
        let item = self.item(index);
        if !item.playable {
            self.message = format!("{} carries no audio: {}", item.id, item.detail);
            return;
        }
        match self.library.player_with(item, self.global.player_options()) {
            Ok(mut player) => {
                if self.looping {
                    player.set_loop(Loop::Forever);
                }
                self.deck.load(player);
                self.current = Some(index);
                self.message = format!("playing {}", item.id);
            }
            Err(e) => self.message = format!("{}: {e}", item.id),
        }
    }

    fn activate(&mut self) {
        match self.selected_row().cloned() {
            Some(Row::Item(index, _)) => self.play_index(index),
            Some(_) => self.toggle(None),
            None => {}
        }
    }

    fn step_item(&mut self, forward: bool) {
        let n = self.rows.len() as isize;
        let from = self
            .current
            .and_then(|c| self.rows.iter().position(|r| matches!(r, Row::Item(i, _) if *i == c)))
            .or(self.state.selected());
        let Some(mut at) = from.map(|a| a as isize) else { return };
        let dir = if forward { 1 } else { -1 };
        loop {
            at += dir;
            if at < 0 || at >= n {
                return;
            }
            if let Row::Item(index, _) = self.rows[at as usize]
                && self.item(index).playable
            {
                self.state.select(Some(at as usize));
                self.play_index(index);
                return;
            }
        }
    }

    fn toggle_loop(&mut self) {
        self.looping = !self.looping;
        let mode = if self.looping { Loop::Forever } else { Loop::Off };
        self.deck.set_loop(mode);
        self.message = format!("loop {}", if self.looping { "on" } else { "off" });
    }

    fn start_save(&mut self, format: ExportFormat, loops: Option<u32>, quality: OggQuality) {
        if self.save.is_some() {
            self.message = "an export is running".to_string();
            return;
        }
        let Some(index) = self.selected_item() else { return };
        let item = self.item(index);
        let directory = self.global.exports();
        let fade = self.global.fade.unwrap_or(10.0);
        let export = ExportOptions { loops, fade, format, quality: quality.quality(), seconds: None };
        match Save::start(self.library, item, self.global.player_options(), &directory, export) {
            Ok(save) => {
                self.message = format!("saving {}", item.id);
                self.save = Some(save);
            }
            Err(error) => self.message = format!("Could not save: {error}. Choose another item or folder."),
        }
    }

    fn export_native(&mut self, sequence: bool) {
        let Some(index) = self.selected_item() else { return };
        let item = self.item(index);
        self.message = crate::save::export_native(self.library, item, &self.global.exports(), sequence);
    }

    fn open_export(&mut self) {
        if self.save.is_some() {
            self.message = "an export is running".to_string();
            return;
        }
        let Some(index) = self.selected_item() else {
            self.message = "select an item to export".to_string();
            return;
        };
        let item = self.item(index);
        let directory = self.global.exports();
        let rows = Format::for_kind(item.kind)
            .iter()
            .map(|&format| (format, crate::save::export_path(&directory, item, format.extension())))
            .collect();
        self.export =
            Some(ExportMenu { item: index, selected: 0, rows, loops: self.global.loops, quality: OggQuality::Normal });
        self.help = false;
    }

    fn move_export(&mut self, delta: isize) {
        if let Some(menu) = &mut self.export {
            let last = menu.rows.len().saturating_sub(1) as isize;
            menu.selected = (menu.selected as isize + delta).clamp(0, last) as usize;
        }
    }

    fn change_quality(&mut self, delta: i32) {
        if let Some(menu) = &mut self.export {
            menu.quality = menu.quality.next(delta);
        }
    }

    fn change_loops(&mut self, delta: i32) {
        if let Some(menu) = &mut self.export {
            let current = menu.loops.unwrap_or(1);
            menu.loops = Some((current as i32 + delta).clamp(1, 99) as u32);
        }
    }

    fn choose_export(&mut self) {
        let Some(menu) = self.export.take() else { return };
        let (format, _) = menu.rows[menu.selected];
        if let Some(export) = format.export_format() {
            self.start_save(export, menu.loops, menu.quality);
            return;
        }
        self.export_native(format == Format::Sequence);
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.typing = false;
                    self.rebuild();
                }
                KeyCode::Enter => self.typing = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.rebuild();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.push(c);
                    self.rebuild();
                }
                _ => {}
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.export.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.export = None;
                    self.message = "export cancelled".to_string();
                }
                KeyCode::Enter => self.choose_export(),
                KeyCode::Char('j') | KeyCode::Down => self.move_export(1),
                KeyCode::Char('k') | KeyCode::Up => self.move_export(-1),
                KeyCode::Char(']') => self.change_loops(1),
                KeyCode::Char('[') => self.change_loops(-1),
                KeyCode::Char('+') | KeyCode::Char('=') => self.change_quality(1),
                KeyCode::Char('-') => self.change_quality(-1),
                _ => {}
            }
            return;
        }
        let page = 15;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown | KeyCode::Char('d') => self.move_by(page),
            KeyCode::PageUp | KeyCode::Char('u') => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.state.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => self.state.select(self.rows.len().checked_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => self.toggle(Some(true)),
            KeyCode::Char('h') | KeyCode::Left => self.collapse_or_parent(),
            KeyCode::Enter => self.activate(),
            KeyCode::Char(' ') => {
                if self.deck.is_loaded() {
                    let paused = self.deck.paused();
                    self.deck.set_paused(!paused);
                } else if let Some(i) = self.selected_item() {
                    self.play_index(i);
                }
            }
            KeyCode::Char('e') => self.open_export(),
            KeyCode::Char('E') => self.choose_folder = true,
            KeyCode::Char('w') => self.start_save(ExportFormat::Wav, self.global.loops, OggQuality::Normal),
            KeyCode::Char('x') => self.export_native(false),
            KeyCode::Char('X') => self.export_native(true),
            KeyCode::Char('s') => {
                self.deck.unload();
                self.current = None;
            }
            KeyCode::Char(']') => {
                let (pos, _) = self.deck.times();
                self.deck.seek(pos + SEEK_STEP);
            }
            KeyCode::Char('[') => {
                let (pos, _) = self.deck.times();
                self.deck.seek((pos - SEEK_STEP).max(0.0));
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.deck.set_volume(self.deck.volume() + VOLUME_STEP),
            KeyCode::Char('-') => self.deck.set_volume(self.deck.volume() - VOLUME_STEP),
            KeyCode::Char('m') => self.toggle_loop(),
            KeyCode::Char('M') => self.deck.set_muted(!self.deck.muted()),
            KeyCode::Char('c') => {
                self.continuous = !self.continuous;
                self.message = format!("play on: {}", if self.continuous { "on" } else { "off" });
            }
            KeyCode::Char('n') | KeyCode::Char('>') => self.step_item(true),
            KeyCode::Char('N') | KeyCode::Char('<') | KeyCode::Char('b') => self.step_item(false),
            KeyCode::Char('a') => {
                self.playable_only = !self.playable_only;
                self.rebuild();
                self.message = (if self.playable_only { "playable items only" } else { "all items" }).to_string();
            }
            KeyCode::Char('D') => {
                self.show_all_dupes = !self.show_all_dupes;
                self.rebuild();
                self.message =
                    (if self.show_all_dupes { "show all duplicates" } else { "group duplicates" }).to_string();
            }
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Esc => {
                if self.save.take().is_some() {
                    self.message = "export cancelled".to_string();
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.rebuild();
                } else {
                    self.deck.unload();
                    self.current = None;
                }
            }
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    /// Whether the next frame differs from this one without any input: audio plays, bars or
    /// meters are still falling, or an export runs.
    fn animating(&self) -> bool {
        self.save.is_some()
            || (self.deck.is_loaded() && !self.deck.paused())
            || self.meters != (0.0, 0.0)
            || self.spectrum.levels.iter().chain(&self.spectrum.caps).any(|&v| v > 0.0)
    }

    /// Advance the meters, the spectrum, exports and the end of the track. The motion follows
    /// the time since the last call, so it looks the same at any frame rate.
    fn tick(&mut self) {
        let now = Instant::now();
        let seconds = now.duration_since(std::mem::replace(&mut self.last_tick, now)).as_secs_f32().min(0.1);
        let fall = METER_FALL_PER_SECOND * seconds;
        let (l, r) = self.deck.peaks();
        self.meters = ((self.meters.0 - fall).max(l), (self.meters.1 - fall).max(r));
        self.deck.analyze(&mut self.spectrum);
        if let Some(error) = self.deck.take_error() {
            self.message = format!("audio stream: {error}");
        }
        let done = self.save.as_mut().and_then(|save| match save.step() {
            Step::Progress => None,
            Step::Done => Some(saved_message(save.path())),
            Step::Failed(error) => Some(export_failed(save.path(), &error)),
        });
        if let Some(message) = done {
            self.save = None;
            self.message = message;
        }
        if self.deck.finished() {
            self.deck.unload();
            if self.continuous && self.current.is_some() {
                self.step_item(true);
            } else {
                self.current = None;
            }
        }
    }
}

fn row_line(app: &App, row: &Row, width: usize) -> Line<'static> {
    match row {
        Row::Kind(k) => {
            let open = app.expanded.contains(&kind_key(*k));
            let total: usize = app.groups[k].values().map(|f| f.items.len()).sum();
            let playable: usize = app.groups[k].values().map(|f| f.playable).sum();
            let count = if app.playable_only { format!("{playable}/{total}") } else { total.to_string() };
            Line::from(vec![
                Span::styled(if open { "▾ " } else { "▸ " }, Style::new().fg(ACCENT)),
                Span::styled(label(*k), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {count}"), Style::new().fg(DIM)),
            ])
        }
        Row::Family(k, f) => {
            let open = app.expanded.contains(&family_key(*k, f));
            let family = &app.groups[k][f];
            let count = if app.playable_only {
                format!("{}/{}", family.playable, family.items.len())
            } else {
                family.items.len().to_string()
            };
            Line::from(vec![
                Span::raw("  "),
                Span::styled(if open { "▾ " } else { "▸ " }, Style::new().fg(ACCENT)),
                Span::raw(f.clone()),
                Span::styled(format!("  {count}"), Style::new().fg(DIM)),
            ])
        }
        Row::Item(index, depth) => {
            let item = app.item(*index);
            let pad = if app.query.is_empty() { 4 + depth * 2 } else { 0 };
            let playing = app.current == Some(*index);
            let style = if !item.playable {
                Style::new().fg(DIM)
            } else if playing {
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let mark = if playing { "♪ " } else { "  " };
            let fold = if item.kind == Kind::Xa && *depth == 0 && !item.id.contains('@') {
                if app.expanded.contains(&item.id) { "▾ " } else { "▸ " }
            } else {
                ""
            };
            let time = music_length(item.duration);
            // Columns: the id, the title filling the rest, then the duration at the right edge.
            let name_width = if width >= 50 { 12 } else { 0 };
            let time_width = 7;
            let fixed = pad + 4 + name_width + time_width;
            let (name, title) = match (item.title.as_str(), name_width) {
                ("", 0) => (String::new(), item.name.clone()),
                ("", _) => (format!("{:<name_width$}", item.name), String::new()),
                (title, 0) => (String::new(), format!("{title} · {}", item.name)),
                (title, _) => (format!("{:<name_width$}", item.name), title.to_string()),
            };
            let room = width.saturating_sub(fixed);
            let title: String = if title.chars().count() > room {
                title.chars().take(room.saturating_sub(1)).chain(['…']).collect()
            } else {
                title
            };
            let used = pad + 4 + name.chars().count() + title.chars().count();
            let gap = width.saturating_sub(used + time_width).max(1);
            Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(mark, Style::new().fg(Color::Green)),
                Span::styled(fold, Style::new().fg(ACCENT)),
                Span::styled(name, style),
                Span::styled(title, style),
                Span::styled(format!("{}{time:>time_width$}", " ".repeat(gap)), Style::new().fg(DIM)),
            ])
        }
    }
}

/// A section heading in the details panel.
fn heading(text: &str) -> Line<'static> {
    Line::styled(text.to_uppercase(), Style::new().fg(DIM).add_modifier(Modifier::BOLD))
}

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let rows = screen_rows(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if area.width < DETAILS_MIN_WIDTH {
            [Constraint::Percentage(100), Constraint::Length(0)]
        } else {
            [Constraint::Percentage(62), Constraint::Fill(1)]
        })
        .split(rows[1]);

    let header = Line::from(vec![
        Span::styled("♫ Breath of Fire III", Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("   ·   {} playable / {} items", app.playable, app.library.catalog().len()),
            Style::new().fg(DIM),
        ),
    ]);
    if roomy(area) {
        frame.render_widget(
            Paragraph::new(header).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(ACCENT))
                    .padding(Padding::horizontal(2)),
            ),
            rows[0],
        );
    } else {
        frame.render_widget(Paragraph::new(header), rows[0]);
    }
    let title = if app.query.is_empty() && !app.typing {
        " Library ".to_string()
    } else {
        format!(" Search: {}{} · {} hits ", app.query, if app.typing { "▏" } else { "" }, app.rows.len())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(padding(area))
        .title(title)
        .border_style(Style::new().fg(if app.typing { Color::Yellow } else { ACCENT }));
    let width = block.inner(columns[0]).width as usize;
    let spaced = roomy(area);
    let items: Vec<ListItem<'_>> = app
        .rows
        .iter()
        .enumerate()
        .map(|(at, r)| {
            let line = row_line(app, r, width);
            // A blank line separates the kind groups.
            if spaced && at > 0 && matches!(r, Row::Kind(_)) {
                ListItem::new(Text::from(vec![Line::raw(""), line]))
            } else {
                ListItem::new(line)
            }
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(Color::Rgb(31, 52, 112)).fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, columns[0], &mut app.state);
    if app.rows.is_empty() {
        let inner = Block::default().borders(Borders::ALL).padding(padding(area)).inner(columns[0]);
        let message = if app.query.is_empty() { "Nothing to list." } else { "No matches. Esc clears the search." };
        frame.render_widget(Paragraph::new(Line::styled(message, Style::new().fg(DIM))), inner);
    }

    let details = match app.selected_row() {
        Some(Row::Item(index, _)) => {
            let item = app.item(*index);
            let catalog = app.library.catalog();
            let mut lines = vec![
                Line::styled(item.id.clone(), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Line::raw(item.name.clone()),
            ];
            if !item.title.is_empty() {
                lines.push(Line::raw(item.title.clone()));
            }
            if let Some(same) = catalog.same_as_text(item) {
                lines.push(Line::raw(format!("Same as   {same}")));
            }
            if let Some(members) = catalog.groups().group_members(&item.id) {
                lines.push(Line::raw(format!("Duplicates: {}", members.len())));
                for m in &members {
                    if let Some(it) = catalog.find(m) {
                        lines.push(Line::styled(format!("  {} {}", it.id, it.name), Style::new().fg(Color::Gray)));
                    } else {
                        lines.push(Line::styled(format!("  {}", m), Style::new().fg(Color::Gray)));
                    }
                }
            }
            lines.push(Line::raw(""));
            lines.push(heading("Properties"));
            lines.push(Line::raw(format!("Family    {}", item.family)));
            lines.push(Line::raw(format!("Length    {}", music_length(item.duration))));
            lines.push(Line::raw(format!("Plays     {}", if item.playable { "yes" } else { "no" })));
            lines.push(Line::raw(format!("File      {}", item.path)));
            lines.push(Line::raw(""));
            lines.push(heading("Description"));
            lines.push(Line::styled(item.detail.clone(), Style::new().fg(Color::Gray)));
            lines
        }
        Some(Row::Family(k, f)) => {
            vec![
                Line::raw(format!("{} / {f}", label(*k))),
                Line::raw(format!("{} items", app.groups[k][f].items.len())),
            ]
        }
        Some(Row::Kind(k)) => vec![Line::raw(label(*k)), Line::raw("Enter or → to open")],
        None => vec![Line::raw("nothing selected")],
    };
    if columns[1].width > 0 {
        let [text, levels] = split_details(columns[1], area);
        frame.render_widget(
            Paragraph::new(details)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).padding(padding(area)).title(" Details ")),
            text,
        );
        if levels.height > 0 {
            draw_levels(frame, app.meters, &app.spectrum, levels);
        }
    }

    draw_now_playing(frame, app, rows[2]);

    let help = help_line(area.width as usize);
    let status = if let Some(save) = &app.save {
        let duration = save.duration();
        let percent = if duration > 0.0 { (save.rendered() / duration * 100.0) as u32 } else { 0 };
        Line::styled(
            format!("saving {}  {} / {}  ({percent}%)", save.id(), clock(save.rendered()), clock(duration)),
            Style::new().fg(Color::Yellow),
        )
    } else if app.message.is_empty() {
        Line::raw("")
    } else {
        Line::styled(app.message.clone(), Style::new().fg(Color::Yellow))
    };
    frame.render_widget(Paragraph::new(status), rows[3]);
    frame.render_widget(Paragraph::new(help), rows[4]);

    if app.help {
        draw_help(frame, area, &app.global.exports());
    }
    if app.export.is_some() {
        draw_export_menu(frame, app, area);
    }
}

/// The key help for the footer, fitted to `width`.
fn help_line(width: usize) -> Line<'static> {
    hints(
        &[
            ("↑↓", "move"),
            ("Enter", "play"),
            ("Space", "pause"),
            ("[ ]", "seek"),
            ("m", "loop"),
            ("n/N", "next/prev"),
            ("e", "export"),
            ("E", "folder"),
            ("/", "search"),
            ("a", "all items"),
            ("+-", "vol"),
            ("D", "dupes"),
            ("?", "help"),
            ("q", "quit"),
        ],
        2,
        width,
    )
}

/// A row of key badges with their actions. Entries drop from the end until the row fits `width`,
/// except the last `keep`, which always stay.
pub fn hints(entries: &[(&str, &str)], keep: usize, width: usize) -> Line<'static> {
    let cost = |(key, action): &(&str, &str)| key.chars().count() + action.chars().count() + 2;
    let split = entries.len().saturating_sub(keep);
    let tail: usize = entries[split..].iter().map(cost).sum();
    let mut used = tail;
    let mut shown = 0;
    for entry in &entries[..split] {
        if used + cost(entry) > width {
            break;
        }
        used += cost(entry);
        shown += 1;
    }
    let spans = entries[..shown]
        .iter()
        .chain(&entries[split..])
        .flat_map(|(name, action)| [key(name), Span::raw(format!(" {action} "))]);
    Line::from(spans.collect::<Vec<_>>())
}

/// Terminals narrower than this hide the details panel and give the list the whole width.
const DETAILS_MIN_WIDTH: u16 = 72;

/// Whether the terminal is big enough for the generous layout: wide padding, a bordered header,
/// a tall now-playing bar with the spectrum. Smaller terminals get the compact layout.
pub fn roomy(area: Rect) -> bool {
    area.height >= 30 && area.width >= 100
}

/// The padding inside panels: 2 cells at the sides and 1 above and below where there is room,
/// otherwise 1 cell at the sides.
pub fn padding(area: Rect) -> Padding {
    if roomy(area) { Padding::new(2, 2, 1, 1) } else { Padding::horizontal(1) }
}

/// The browser's vertical split, top to bottom: the header (3 rows with a border, 1 when compact),
/// the list and details taking the remaining height, the now-playing bar (about 30% of the height
/// when roomy, 5 rows when compact), a row for messages and a row for the key help. Each has its
/// own row, so none hides another. Shared by the library browser and the extract browser.
pub fn screen_rows(area: Rect) -> [Rect; 5] {
    let (header, playbar) = if roomy(area) { (3, (area.height * 3 / 10).clamp(10, 16)) } else { (1, 5) };
    Layout::vertical([
        Constraint::Length(header),
        Constraint::Min(5),
        Constraint::Length(playbar),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area)
}

/// Split the details column: the text above and, in the compact layout when it is tall enough, the
/// levels and spectrum below. The roomy layout shows them in the now-playing bar instead.
pub fn split_details(area: Rect, full: Rect) -> [Rect; 2] {
    let levels = if !roomy(full) && area.height >= 16 { (area.height / 3).clamp(7, 16) } else { 0 };
    Layout::vertical([Constraint::Min(5), Constraint::Length(levels)]).areas(area)
}

/// A popup centered in `area`: `percent` of its width between `min` and `max` cells, `height` rows,
/// both limited to the area.
pub fn popup(area: Rect, percent: u16, min: u16, max: u16, height: u16) -> Rect {
    let width = (area.width * percent / 100).clamp(min, max).min(area.width);
    let height = height.min(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

pub fn key(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::new().fg(Color::Black).bg(ACCENT))
}

fn draw_now_playing(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let now = app.current.filter(|_| app.deck.is_loaded()).map(|index| {
        let item = app.item(index);
        let label = match item.title.as_str() {
            "" => item.name.clone(),
            title => format!("{title} · {}", item.name),
        };
        (item.id.clone(), label)
    });
    draw_playbar(frame, &app.deck, now, app.looping, app.meters, &app.spectrum, area);
}

/// The now-playing bar shared by the library browser and the extract browser: the title (bold)
/// with the time at the right, a progress gauge, the loop and volume state, and, when roomy, the
/// level meters and the spectrum across the full width. `now` is the id and label of what plays
/// (None: nothing). It always has the same rows, so the layout does not jump.
pub fn draw_playbar(
    frame: &mut Frame<'_>,
    deck: &Deck,
    now: Option<(String, String)>,
    looping: bool,
    meters: (f32, f32),
    spectrum: &Spectrum,
    area: Rect,
) {
    let full = frame.area();
    let block = Block::default().borders(Borders::ALL).padding(padding(full)).title(" ♪ Now playing ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let thick = if roomy(full) { 2 } else { 1 };
    let rows =
        Layout::vertical([Constraint::Length(1), Constraint::Length(thick), Constraint::Length(1), Constraint::Min(0)])
            .split(inner);

    let (pos, dur) = if now.is_some() { deck.times() } else { (0.0, 0.0) };
    let title = match &now {
        Some((id, label)) => {
            let state = if deck.paused() { "❚❚ " } else { "▶ " };
            let kind = id.split([':', '/']).next().unwrap_or("");
            Line::from(vec![
                Span::styled(state, Style::new().fg(Color::Green)),
                Span::styled(label.clone(), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(format!("   {kind}"), Style::new().fg(DIM)),
            ])
        }
        None => Line::styled("Nothing playing", Style::new().fg(DIM)),
    };
    let time = format!("{} / {}", clock(pos), clock(dur));
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(time.chars().count() as u16)]).areas(rows[0]);
    frame.render_widget(Paragraph::new(title), left);
    frame.render_widget(Paragraph::new(Line::styled(time, Style::new().add_modifier(Modifier::BOLD))), right);

    let ratio = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
    let window = deck
        .loop_window()
        .filter(|_| looping && dur > 0.0)
        .map(|(a, b)| ((a / dur).clamp(0.0, 1.0), (b / dur).clamp(0.0, 1.0)));
    let bar = progress_line(rows[1].width as usize, ratio, window);
    frame.render_widget(Paragraph::new(vec![bar; usize::from(rows[1].height)]), rows[1]);

    let volume = if deck.muted() {
        Span::styled("muted", Style::new().fg(Color::Yellow))
    } else {
        Span::styled(format!("vol {:>3}%", (deck.volume() * 100.0) as u32), Style::new().fg(DIM))
    };
    let loop_text = match (now.is_some(), deck.loop_window()) {
        (true, Some((a, b))) => format!("loop window {} - {}", clock(a), clock(b)),
        (true, None) => "no loop window".to_string(),
        (false, _) => format!("output: {}", deck.device),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if looping { "⟳ loop on   " } else { "  loop off  " },
                Style::new().fg(if looping { Color::Yellow } else { DIM }),
            ),
            volume,
            Span::styled(format!("   {loop_text}"), Style::new().fg(DIM)),
        ])),
        rows[2],
    );

    if rows[3].height >= 2 {
        let [meter_area, bars] =
            Layout::horizontal([Constraint::Length(16), Constraint::Fill(1)]).spacing(2).areas(rows[3]);
        draw_meters(frame, meters, meter_area);
        let lines = spectrum_lines(spectrum, bars.width as usize, bars.height as usize);
        frame.render_widget(Paragraph::new(lines).style(Style::new().fg(ACCENT)), bars);
    }
}

/// A theme color as a terminal color.
fn themed(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// One row of the progress bar, `width` cells wide: the played part in the accent with a
/// sub-cell edge, and, for a loop `window` (fractions of the track), a shaded span with thin
/// markers at its start and end. The markers and the span show over the played part.
fn progress_line(width: usize, ratio: f64, window: Option<(f64, f64)>) -> Line<'static> {
    const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let band = themed(theme::LOOP_BAND);
    let band_played = themed(theme::LOOP_BAND_PLAYED);
    let cells = width as f64;
    let filled = ratio.clamp(0.0, 1.0) * cells;
    let edges = window.map(|(a, b)| {
        let start = ((a * cells) as usize).min(width.saturating_sub(1));
        let end = ((b * cells).ceil() as usize).clamp(start + 1, width.max(1)) - 1;
        (start, end)
    });
    let spans: Vec<Span<'static>> = (0..width)
        .map(|cell| {
            let inside = edges.is_some_and(|(start, end)| (start..=end).contains(&cell));
            let covered = (filled - cell as f64).clamp(0.0, 1.0);
            let played = covered >= 1.0;
            let ground = match (inside, played) {
                (true, true) => band_played,
                (true, false) => band,
                (false, _) => Color::Black,
            };
            let is_edge = edges.is_some_and(|(start, end)| cell == start || cell == end);
            if is_edge {
                let glyph = if edges.is_some_and(|(start, _)| cell == start) { "▏" } else { "▕" };
                let mark = themed(if covered > 0.0 { theme::LOOP_MARK_PLAYED } else { theme::LOOP_MARK });
                Span::styled(glyph, Style::new().fg(mark).bg(ground).add_modifier(Modifier::BOLD))
            } else if played {
                Span::styled("█", Style::new().fg(if inside { band_played } else { ACCENT }).bg(ground))
            } else if covered > 0.0 {
                let step = ((covered * 8.0) as usize).clamp(1, 7) - 1;
                Span::styled(EIGHTHS[step].to_string(), Style::new().fg(ACCENT).bg(ground))
            } else {
                Span::styled(" ", Style::new().bg(ground))
            }
        })
        .collect();
    Line::from(spans)
}

/// The L and R level meters, one row each: the channel name, then the bar.
fn draw_meters(frame: &mut Frame<'_>, meters: (f32, f32), area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    for (area, level, name) in [(rows[0], meters.0, "L"), (rows[1], meters.1, "R")] {
        let color = if level > 0.9 {
            Color::LightRed
        } else if level > 0.6 {
            Color::Yellow
        } else {
            Color::Green
        };
        let [label, bar] = Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
        frame.render_widget(Paragraph::new(Line::styled(name, Style::new().fg(DIM))), label);
        frame.render_widget(
            Gauge::default().ratio(f64::from(level.clamp(0.0, 1.0))).label("").gauge_style(Style::new().fg(color)),
            bar,
        );
    }
}

/// The level meters and the spectrum in a bordered block: the compact layout's visualizer.
pub fn draw_levels(frame: &mut Frame<'_>, meters: (f32, f32), spectrum: &Spectrum, area: Rect) {
    let block = Block::default().borders(Borders::ALL).padding(Padding::horizontal(1)).title(" Levels ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    draw_meters(frame, meters, rows[0]);
    let bars = spectrum_lines(spectrum, rows[1].width as usize, rows[1].height as usize);
    frame.render_widget(Paragraph::new(bars).style(Style::new().fg(ACCENT)), rows[1]);
}

/// The spectrum as rows of block characters, loudest bands tallest, with a cap above each bar.
fn spectrum_lines(spectrum: &Spectrum, width: usize, height: usize) -> Vec<Line<'static>> {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let bands = spectrum.levels.len();
    if width == 0 || height == 0 || bands == 0 {
        return Vec::new();
    }
    let band_of = |column: usize| (column * bands / width).min(bands - 1);
    (0..height)
        .rev()
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..width)
                .map(|column| {
                    let band = band_of(column);
                    let eighths = (spectrum.levels[band] * (height * 8) as f32).round() as usize;
                    let cap = ((spectrum.caps[band] * (height * 8) as f32).round() as usize).saturating_sub(1) / 8;
                    let fill = eighths.saturating_sub(row * 8).min(8);
                    if fill == 0 && cap == row && spectrum.caps[band] > 0.02 {
                        Span::styled("▔", Style::new().fg(Color::Yellow))
                    } else {
                        Span::raw(BLOCKS[fill].to_string())
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// The longest `text` that fits `width` columns: a long path keeps its end.
fn tail_fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    format!("…{}", text.chars().skip(count - width + 1).collect::<String>())
}

/// A popup as wide as its longest line plus the border and padding, centered in `area`.
fn popup_for(area: Rect, lines: &[Line<'_>]) -> Rect {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (widest + 6).max(40);
    popup(area, 100, width, width, lines.len() as u16 + 4)
}

fn draw_help(frame: &mut Frame<'_>, area: Rect, export_dir: &std::path::Path) {
    let folder = tail_fit(&export_dir.display().to_string(), 44);
    let lines = vec![
        Line::raw("Move     j/k ↑/↓   g/G top/bottom   d/u page"),
        Line::raw("         h/l ←/→ close/open"),
        Line::raw("Play     Enter play   Space pause   s stop"),
        Line::raw("         n/N next/prev   [ ] seek 5 s"),
        Line::raw("         + - volume   M mute   c play on"),
        Line::raw("Loop     m loop on/off (on by default)"),
        Line::raw("Save     e export menu   w quick WAV"),
        Line::raw("         x native package   X sequence package"),
        Line::raw("Formats  music: WAV FLAC Ogg MIDI sequence"),
        Line::raw("         sample: WAV FLAC Ogg VAG   bank: VAB"),
        Line::raw("         XA and CD: WAV FLAC Ogg"),
        Line::raw(format!("Folder   {folder}")),
        Line::raw("Find     / search (Esc clears)   a all items   D toggle dupes"),
        Line::raw("Other    ? this help   Esc clear or stop   q quit"),
    ];
    let rect = popup_for(area, &lines);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).padding(Padding::uniform(1)).title(" Keys ")),
        rect,
    );
}

/// The export menu: the item's name, the export folder and one row per format
/// it supports, with the file each row writes.
fn draw_export_menu(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(menu) = &app.export else { return };
    let item = app.item(menu.item);
    let mut lines = vec![
        Line::styled(
            match item.title.as_str() {
                "" => item.name.clone(),
                title => format!("{title} · {}", item.name),
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!("Folder  {}", tail_fit(&absolute(&app.global.exports()).display().to_string(), 52))),
        Line::raw(format!(
            "Loops   {}   [ ] change, 1 = one play",
            menu.loops.map_or_else(|| "auto (intro + 1 loop)".to_string(), |loops| loops.to_string())
        )),
        Line::raw(format!("Ogg     {}   - + change", menu.quality.label())),
        Line::raw(""),
    ];
    for (index, (format, path)) in menu.rows.iter().enumerate() {
        let selected = index == menu.selected;
        let name =
            path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
        lines.push(Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(format!("{:<19}", format.label()), Style::new().fg(if selected { ACCENT } else { DIM })),
            Span::raw(name),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Enter save   ↑↓ choose   [ ] loops   - + quality   Esc close", Style::new().fg(DIM)));
    let rect = popup_for(area, &lines);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).padding(Padding::uniform(1)).title(" Export ")),
        rect,
    );
}

/// Run the browser until the user quits.
pub fn run(global: &Global, naming: &dyn Naming) -> Result<ExitCode, String> {
    let mut global = global.clone();
    let library = loop {
        match open_library(&global, naming) {
            Ok(library) => break library,
            Err(error) => match super::pick::pick_data(&error)? {
                Some(path) => global.dir = Some(path),
                None => return Ok(ExitCode::SUCCESS),
            },
        }
    };
    let mut app = App::new(library, &global);
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        // Frames are paced from the last draw, so input waits at most until the next frame is due
        // and a key press draws at once. Idle, the loop wakes ten times a second.
        while !app.quit {
            let drawn = Instant::now();
            terminal.draw(|f| draw(f, &mut app))?;
            let frame = if app.animating() { FRAME } else { IDLE_FRAME };
            let mut wait = frame.saturating_sub(drawn.elapsed());
            while event::poll(wait)? {
                if let Event::Key(key) = event::read()? {
                    app.message.clear();
                    app.on_key(key);
                    break;
                }
                wait = frame.saturating_sub(drawn.elapsed());
            }
            if std::mem::take(&mut app.choose_folder) {
                let start = absolute(&app.global.export_dir);
                if let Some(dir) =
                    super::pick::pick_folder(&mut terminal, "Choose the folder exports are written into.", &start)
                        .map_err(std::io::Error::other)?
                {
                    app.global.export_dir = dir;
                }
            }
            app.tick();
        }
        Ok(())
    })();
    ratatui::restore();
    result.map_err(|e| e.to_string())?;
    Ok(ExitCode::SUCCESS)
}
