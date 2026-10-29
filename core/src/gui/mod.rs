// SPDX-License-Identifier: MIT
//! `bof3player-gui`: a window to browse and play the game's audio.
//!
//! It offers the terminal browser's features through the same library calls: `discover` and
//! `Library` open and list the game data, `Deck` plays it and `save` writes files. This module
//! holds the state and the key map; `view` draws it with `gpui`.

mod chooser;
mod icons;
mod list;
mod load;
mod view;
mod widgets;

use std::cell::Cell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, Application, Bounds, Context, FocusHandle, Focusable, KeyDownEvent, Keystroke, Pixels,
    ScrollStrategy, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowOptions, px, size,
};

use clap::Parser;

use crate::catalog::{Item, Kind};
use crate::deck::Deck;
use crate::options::Global;
use crate::player::Loop;
use crate::save::{
    ExportFormat, ExportOptions, Format, OggQuality, Save, Step, export_failed, export_path, saved_message,
};
use crate::spectrum::Spectrum;
use chooser::{Chooser, Target};
use list::{Row, Tree};
use load::{Failed, Loaded, Loading};

/// Seconds a seek key moves.
const SEEK_STEP: f64 = 5.0;
/// Volume change per key press.
const VOLUME_STEP: f32 = 0.05;
/// Rows a page key moves.
const PAGE: isize = 15;
/// Levels fall this much per tick.
const METER_FALL: f32 = 0.02;
/// Bands of the spectrum analyzer.
const SPECTRUM_BANDS: usize = 64;
/// Pace of the window refresh.
const TICK: Duration = Duration::from_millis(16);

/// The window's command line: the same switches as the terminal player.
#[derive(Parser)]
#[command(name = "bof3player-gui", version, about = "Browse and play the audio of Breath of Fire III in a window")]
struct Cli {
    #[command(flatten)]
    global: Global,
}

/// Start the window. `args` are the command-line arguments without the program name.
pub fn main(args: Vec<OsString>) -> ExitCode {
    let cli = Cli::parse_from(std::iter::once(OsString::from("bof3player-gui")).chain(args));
    let global = cli.global;
    let failed = Rc::new(Cell::new(false));
    let opened = Rc::clone(&failed);
    Application::new().with_assets(icons::Icons).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.0), px(720.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("bof3player".into()), ..Default::default() }),
            ..Default::default()
        };
        let window = cx.open_window(options, |window, cx| {
            let gui = cx.new(|cx| Gui::new(global, cx));
            gui.read(cx).focus.focus(window);
            gui
        });
        match window {
            Ok(_) => cx.activate(true),
            Err(error) => {
                eprintln!("bof3player-gui: cannot open a window: {error}");
                opened.set(true);
                cx.quit();
            }
        }
    });
    if failed.get() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// What the window shows in place of the list.
enum State {
    Loading(Loading),
    Failed(Failed),
    Ready(Browser),
}

/// An opened catalog and its list.
struct Browser {
    loaded: Loaded,
    tree: Tree,
    rows: Vec<Row>,
    selected: usize,
}

/// A text field that takes the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Filter,
    Path,
}

/// The open export menu: the item, its formats, their target files and the render settings.
struct ExportMenu {
    item: &'static Item,
    selected: usize,
    rows: Vec<(Format, PathBuf)>,
    loops: Option<u32>,
    quality: OggQuality,
}

struct Gui {
    state: State,
    /// The switches the window started with.
    global: Global,
    deck: Deck,
    /// The item the deck holds.
    current: Option<&'static Item>,
    filter: String,
    /// Text of the path field.
    path: String,
    editing: Option<Field>,
    looping: bool,
    continuous: bool,
    playable_only: bool,
    show_all_dupes: bool,
    message: String,
    help: bool,
    chooser: Option<Chooser>,
    export: Option<ExportMenu>,
    save: Option<Save>,
    export_dir: PathBuf,
    meters: (f32, f32),
    spectrum: Spectrum,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The first and one past the last row drawn.
    visible: Rc<Cell<(usize, usize)>>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Gui {
    fn new(global: Global, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |gui, cx| gui.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            state: State::Loading(Loading::start(global.dir.as_deref(), global.names.as_deref())),
            deck: Deck::new(global.mute),
            current: None,
            filter: String::new(),
            path: global.dir.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
            editing: None,
            looping: true,
            continuous: true,
            playable_only: true,
            show_all_dupes: false,
            message: String::new(),
            help: false,
            chooser: None,
            export: None,
            save: None,
            export_dir: global.export_dir.clone(),
            global,
            meters: (0.0, 0.0),
            spectrum: Spectrum::new(SPECTRUM_BANDS),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            visible: Rc::default(),
            seek_bounds: Rc::default(),
            volume_bounds: Rc::default(),
        }
    }

    /// Advance what moves without input, and redraw while something does.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let busy = matches!(self.state, State::Loading(_)) || self.save.is_some();
        self.poll_loading();
        let (left, right) = self.deck.peaks();
        self.meters = ((self.meters.0 - METER_FALL).max(left), (self.meters.1 - METER_FALL).max(right));
        self.deck.analyze(&mut self.spectrum);
        if let Some(error) = self.deck.take_error() {
            self.message = format!("Audio output error: {error}. Check the output device.");
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
        let moving = self.deck.is_loaded() && !self.deck.paused()
            || self.meters != (0.0, 0.0)
            || self.spectrum.levels.iter().any(|&level| level > 0.0);
        if busy || moving {
            cx.notify();
        }
    }

    fn open(&mut self) {
        self.deck.unload();
        self.current = None;
        let path = self.path.trim();
        let hint = (!path.is_empty()).then(|| PathBuf::from(path));
        self.state = State::Loading(Loading::start(hint.as_deref(), self.global.names.as_deref()));
    }

    fn poll_loading(&mut self) {
        let State::Loading(loading) = &self.state else { return };
        let Some(result) = loading.poll() else { return };
        self.state = match result {
            Ok(loaded) => {
                self.path = loaded.path.display().to_string();
                if let Some(notice) = &loaded.notice {
                    self.message.clone_from(notice);
                }
                let tree = Tree::new(loaded.library);
                let rows = tree.rows(&self.filter, self.playable_only);
                let selected = rows.iter().position(|row| matches!(row, Row::Item(_))).unwrap_or(0);
                State::Ready(Browser { loaded, tree, rows, selected })
            }
            Err(failed) => {
                if self.chooser.is_none() {
                    self.browse_for(Target::Data);
                }
                State::Failed(failed)
            }
        };
    }

    /// Rebuild the rows and keep the selection on the same row when it is still listed.
    fn rebuild(&mut self) {
        let State::Ready(browser) = &mut self.state else { return };
        let keep = browser.rows.get(browser.selected).map(Row::key);
        let mut rows = browser.tree.rows(&self.filter, self.playable_only);
        if !self.show_all_dupes {
            let catalog = browser.loaded.library.catalog();
            let groups = catalog.groups();
            use std::collections::HashSet;
            let mut seen: HashSet<String> = HashSet::new();
            rows.retain(|row| {
                if let Row::Item(item) = row {
                    if let Some(canon) = groups.canonical_of(&item.id) {
                        if canon != item.id {
                            return false;
                        }
                        if seen.contains(&canon) {
                            return false;
                        }
                        seen.insert(canon);
                        return true;
                    } else if groups.group_members(&item.id).is_some()
                        && groups.canonical_of(&item.id).as_deref() == Some(item.id.as_str())
                    {
                        if seen.contains(&item.id) {
                            return false;
                        }
                        seen.insert(item.id.clone());
                        return true;
                    }
                }
                true
            });
        }
        browser.rows = rows;
        browser.selected = keep
            .and_then(|key| browser.rows.iter().position(|row| row.key() == key))
            .unwrap_or_else(|| browser.rows.iter().position(|row| matches!(row, Row::Item(_))).unwrap_or(0));
        self.reveal();
    }

    fn selected_row(&self) -> Option<Row> {
        match &self.state {
            State::Ready(browser) => browser.rows.get(browser.selected).copied(),
            _ => None,
        }
    }

    fn selected_item(&self) -> Option<&'static Item> {
        match self.selected_row() {
            Some(Row::Item(item)) => Some(item),
            _ => None,
        }
    }

    /// Scroll just far enough to show the selected row.
    fn reveal(&self) {
        let State::Ready(browser) = &self.state else { return };
        let (first, end) = self.visible.get();
        let at = browser.selected;
        if at <= first {
            self.scroll.scroll_to_item_strict(at, ScrollStrategy::Top);
        } else if at + 1 >= end {
            self.scroll.scroll_to_item_strict(at, ScrollStrategy::Bottom);
        }
    }

    fn select(&mut self, index: usize) {
        if let State::Ready(browser) = &mut self.state {
            browser.selected = index.min(browser.rows.len().saturating_sub(1));
        }
        self.reveal();
    }

    fn move_by(&mut self, delta: isize) {
        if let State::Ready(browser) = &self.state {
            self.select(browser.selected.saturating_add_signed(delta));
        }
    }

    fn toggle(&mut self, open: Option<bool>) {
        if let State::Ready(browser) = &mut self.state
            && let Some(row) = browser.rows.get(browser.selected).copied()
        {
            browser.tree.set_open(&row, open);
            self.rebuild();
        }
    }

    /// Close the selected group, or move to the group an item sits in.
    fn collapse_or_parent(&mut self) {
        let State::Ready(browser) = &mut self.state else { return };
        let Some(row) = browser.rows.get(browser.selected).copied() else { return };
        if !matches!(row, Row::Item(_)) && browser.tree.is_open(&row) {
            browser.tree.set_open(&row, Some(false));
            self.rebuild();
            return;
        }
        let above = &browser.rows[..browser.selected];
        let parent = match row {
            Row::Item(_) => above.iter().rposition(|row| matches!(row, Row::Family(..))),
            _ => None,
        }
        .or_else(|| above.iter().rposition(|row| matches!(row, Row::Kind(..))));
        if let Some(parent) = parent {
            self.select(parent);
        }
    }

    fn activate(&mut self) {
        match self.selected_row() {
            Some(Row::Item(item)) => self.play(item),
            Some(_) => self.toggle(None),
            None => {}
        }
    }

    fn play(&mut self, item: &'static Item) {
        let State::Ready(browser) = &self.state else { return };
        if !item.playable {
            self.message = format!("{} carries no audio: {}", item.id, item.detail);
            return;
        }
        match browser.loaded.library.player_with(item, self.global.player_options()) {
            Ok(mut player) => {
                if self.looping {
                    player.set_loop(Loop::Forever);
                }
                self.deck.load(player);
                self.current = Some(item);
                self.message.clear();
            }
            Err(error) => self.message = format!("Cannot play {}: {error}. Pick another item.", item.id),
        }
    }

    fn toggle_pause(&mut self) {
        if self.deck.is_loaded() {
            self.deck.set_paused(!self.deck.paused());
        } else if let Some(item) = self.selected_item() {
            self.play(item);
        }
    }

    fn stop(&mut self) {
        self.deck.unload();
        self.current = None;
    }

    /// Play the next or previous playable item after or before the playing one.
    fn step_item(&mut self, forward: bool) {
        let State::Ready(browser) = &self.state else { return };
        let from = self
            .current
            .and_then(|now| {
                browser.rows.iter().position(|row| matches!(row, Row::Item(item) if std::ptr::eq(*item, now)))
            })
            .unwrap_or(browser.selected);
        let playable = |at: &usize| matches!(browser.rows[*at], Row::Item(item) if item.playable);
        let next = if forward { (from + 1..browser.rows.len()).find(playable) } else { (0..from).rev().find(playable) };
        match next {
            Some(at) => {
                self.select(at);
                if let Some(item) = self.selected_item() {
                    self.play(item);
                }
            }
            None if forward => self.stop(),
            None => self.message = "This is the first item.".to_string(),
        }
    }

    fn toggle_loop(&mut self) {
        self.looping = !self.looping;
        self.deck.set_loop(if self.looping { Loop::Forever } else { Loop::Off });
    }

    fn seek_to(&mut self, fraction: f32) {
        let (_, duration) = self.deck.times();
        if duration > 0.0 {
            self.deck.seek(duration * f64::from(fraction));
        }
    }

    fn seek_by(&mut self, delta: f64) {
        let (position, duration) = self.deck.times();
        if duration > 0.0 {
            self.deck.seek((position + delta).clamp(0.0, duration));
        }
    }

    fn start_save(&mut self, format: ExportFormat, loops: Option<u32>, quality: OggQuality) {
        if self.save.is_some() {
            self.message = "An export is running.".to_string();
            return;
        }
        let (State::Ready(browser), Some(item)) = (&self.state, self.selected_item()) else { return };
        let export = ExportOptions {
            loops,
            fade: self.global.fade.unwrap_or(10.0),
            format,
            quality: quality.quality(),
            seconds: None,
        };
        match Save::start(browser.loaded.library, item, self.global.player_options(), &self.export_dir, export) {
            Ok(save) => {
                self.message.clear();
                self.save = Some(save);
            }
            Err(error) => self.message = format!("Could not save: {error}. Choose another item or folder."),
        }
    }

    fn export_native(&mut self, sequence: bool) {
        let (State::Ready(browser), Some(item)) = (&self.state, self.selected_item()) else { return };
        self.message = crate::save::export_native(browser.loaded.library, item, &self.export_dir, sequence);
    }

    fn open_export(&mut self) {
        if self.save.is_some() {
            self.message = "An export is running.".to_string();
            return;
        }
        let Some(item) = self.selected_item() else {
            self.message = "Select an item to export.".to_string();
            return;
        };
        let rows = Format::for_kind(item.kind)
            .iter()
            .map(|&format| (format, export_path(&self.export_dir, item, format.extension())))
            .collect();
        self.export =
            Some(ExportMenu { item, selected: 0, rows, loops: self.global.loops, quality: OggQuality::Normal });
        self.help = false;
    }

    fn choose_export(&mut self) {
        let Some(menu) = self.export.take() else { return };
        let (format, _) = menu.rows[menu.selected];
        if let Some(export) = format.export_format() {
            self.start_save(export, menu.loops, menu.quality);
        } else {
            self.export_native(format == Format::Sequence);
        }
    }

    fn export_key(&mut self, name: &str) {
        let Some(menu) = &mut self.export else { return };
        match name {
            "escape" | "q" => {
                self.export = None;
                self.message = "Export cancelled.".to_string();
            }
            "enter" => self.choose_export(),
            "f" => self.browse_for(Target::Export),
            "j" | "down" => menu.selected = (menu.selected + 1).min(menu.rows.len() - 1),
            "k" | "up" => menu.selected = menu.selected.saturating_sub(1),
            "]" => menu.loops = Some((menu.loops.unwrap_or(1) + 1).min(99)),
            "[" => menu.loops = Some(menu.loops.unwrap_or(1).saturating_sub(1).max(1)),
            "+" | "=" => menu.quality = menu.quality.next(1),
            "-" => menu.quality = menu.quality.next(-1),
            _ => {}
        }
    }

    /// The key map. While a text field has the keyboard, keys type into it.
    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let name = key_name(keystroke);
        if command && name == "q" {
            cx.quit();
        } else if let Some(field) = self.editing {
            self.edit_key(field, keystroke, command, cx);
        } else if self.chooser.is_some() {
            self.chooser_key(name);
        } else if self.help {
            self.help = false;
        } else if self.export.is_some() {
            self.export_key(name);
        } else if !self.browse_key_with_event(name, keystroke, cx) {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn edit_key(&mut self, field: Field, keystroke: &Keystroke, command: bool, cx: &mut Context<Self>) {
        let text = match field {
            Field::Filter => &mut self.filter,
            Field::Path => &mut self.path,
        };
        match keystroke.key.as_str() {
            "escape" => {
                if field == Field::Filter {
                    text.clear();
                }
                self.editing = None;
            }
            "enter" => {
                self.editing = None;
                if field == Field::Path {
                    self.open();
                }
            }
            "backspace" => {
                text.pop();
            }
            "down" => self.move_by(1),
            "up" => self.move_by(-1),
            "v" if command => {
                if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    text.push_str(pasted.lines().next().unwrap_or_default());
                }
            }
            _ if !command => {
                if let Some(typed) = keystroke.key_char.as_deref().filter(|typed| !typed.chars().any(char::is_control))
                {
                    text.push_str(typed);
                }
            }
            _ => {}
        }
        if field == Field::Filter {
            self.rebuild();
        }
    }

    /// The browser's keys, as in the terminal browser. Returns whether the key was used.
    fn browse_key_with_event(&mut self, name: &str, _keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        match name {
            "D" => {
                self.show_all_dupes = !self.show_all_dupes;
                self.rebuild();
                self.message = if self.show_all_dupes {
                    "Show all duplicates".to_string()
                } else {
                    "Group duplicates".to_string()
                };
                return true;
            }
            "q" => cx.quit(),
            "j" | "down" => self.move_by(1),
            "k" | "up" => self.move_by(-1),
            "pagedown" | "d" => self.move_by(PAGE),
            "pageup" | "u" => self.move_by(-PAGE),
            "g" | "home" => self.select(0),
            "G" | "end" => self.select(usize::MAX),
            "l" | "right" => self.toggle(Some(true)),
            "h" | "left" => self.collapse_or_parent(),
            "enter" => self.activate(),
            "space" => self.toggle_pause(),
            "e" => self.open_export(),
            "w" => self.start_save(ExportFormat::Wav, None, OggQuality::Normal),
            "x" => self.export_native(false),
            "X" => self.export_native(true),
            "s" => self.stop(),
            "]" => self.seek_by(SEEK_STEP),
            "[" => self.seek_by(-SEEK_STEP),
            "+" | "=" => self.deck.set_volume(self.deck.volume() + VOLUME_STEP),
            "-" => self.deck.set_volume(self.deck.volume() - VOLUME_STEP),
            "M" => self.deck.set_muted(!self.deck.muted()),
            "m" => self.toggle_loop(),
            "c" => self.continuous = !self.continuous,
            "n" | ">" => self.step_item(true),
            "N" | "<" | "b" => self.step_item(false),
            "a" => {
                self.playable_only = !self.playable_only;
                self.rebuild();
            }
            "/" => self.editing = Some(Field::Filter),
            "escape" => {
                if self.save.take().is_some() {
                    self.message = "Export cancelled.".to_string();
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild();
                } else {
                    self.stop();
                }
            }
            "?" => self.help = true,
            _ => return false,
        }
        true
    }
}

/// The kind and family of an item, for the now-playing line.
fn describe(item: &Item) -> String {
    let kind = match item.kind {
        Kind::Music => "Music",
        Kind::Sample => "Sound effect",
        Kind::Xa => "XA",
        Kind::Cdda => "CD audio",
        Kind::Bank => "Sound bank",
    };
    format!("{kind} / {}", item.family)
}

/// The name a key map matches: the typed character, else the key's name.
fn key_name(keystroke: &Keystroke) -> &str {
    let command = keystroke.modifiers.control || keystroke.modifiers.platform;
    keystroke
        .key_char
        .as_deref()
        .filter(|typed| !command && !typed.is_empty() && typed.chars().all(|c| !c.is_control() && !c.is_whitespace()))
        .unwrap_or(&keystroke.key)
}

impl Focusable for Gui {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
