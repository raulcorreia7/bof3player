// SPDX-License-Identifier: MIT
//! Drawing the window.

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, IntoElement, Render, SharedString, Window, div, prelude::*, px,
    transparent_black, uniform_list,
};

use super::chooser::Target;
use super::icons::icon;
use super::list::{self, Row};
use super::widgets::{Palette, RADIUS, button, chip, field, meter, panel, slider, spectrum, transport};
use super::{ExportMenu, Field, Gui, State, describe};
use crate::catalog::Item;
use crate::deck::clock;
use crate::discover::SourceKind;

/// Height of a list row.
const ROW: f32 = 28.0;

/// The keys the help lists, as in the terminal browser.
const HELP: [(&str, &str); 19] = [
    ("Space", "Play or pause"),
    ("Enter", "Play the item, or open or close a group"),
    ("Up Down  j k", "Move"),
    ("PgUp PgDn  d u", "Move a page"),
    ("g G  Home End", "First or last row"),
    ("Right Left  l h", "Open a group, or close it and move to its parent"),
    ("n N", "Next or previous playable item"),
    ("[ ]", "Seek 5 s back or forward"),
    ("+ -", "Volume"),
    ("m", "Loop the track"),
    ("M", "Mute or unmute"),
    ("c", "Play on when an item ends"),
    ("/", "Filter (Enter keeps it, Esc clears it)"),
    ("a", "All items, or playable items only"),
    ("s  Esc", "Stop"),
    ("w", "Save the item as WAV"),
    ("e", "Export menu: WAV, FLAC, Ogg Vorbis and native files"),
    ("x X", "Save the native file, or the sequence package"),
    ("?  q", "Help, quit"),
];

impl Render for Gui {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::get();
        // The text fields are virtual; the window keeps the key focus.
        if !self.focus.is_focused(window) {
            self.focus.focus(window);
        }
        let header = self.header();
        let source = self.source_row(cx);
        let filter = self.filter_row(cx);
        let list = self.list_panel(cx);
        let player = self.now_playing(cx);
        let status = self.status_row();
        let mut root = div()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(palette.bg)
            .text_color(palette.text)
            .text_sm()
            .child(header)
            .child(source)
            .child(filter)
            .child(list)
            .child(player)
            .child(status);
        if let Some(chooser) = &self.chooser {
            root = root.child(overlay(self.chooser_card(chooser, cx)));
        } else if self.help {
            root = root.child(overlay(Self::help_card()));
        } else if let Some(menu) = &self.export {
            root = root.child(overlay(self.export_card(menu, cx)));
        }
        root
    }
}

fn overlay(card: impl IntoElement) -> Div {
    let palette = Palette::get();
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.scrim)
        .occlude()
        .child(card)
}

impl Gui {
    fn header(&self) -> Div {
        let palette = Palette::get();
        let (counts, kind) = match &self.state {
            State::Ready(browser) => (
                format!("{} playable / {} items", browser.tree.playable, browser.tree.total()),
                match browser.loaded.kind {
                    SourceKind::Disc => "Disc image",
                    SourceKind::Folder => "Data folder",
                },
            ),
            State::Loading(_) => ("Opening game data...".to_string(), ""),
            State::Failed(_) => ("No game data loaded".to_string(), ""),
        };
        let disc = match &self.state {
            State::Ready(browser) => browser
                .loaded
                .path
                .file_name()
                .map_or_else(|| browser.loaded.path.display().to_string(), |name| name.to_string_lossy().into_owned()),
            _ => String::new(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text)
                            .child("Breath of Fire III"),
                    )
                    .child(div().text_color(palette.muted).child("Music player"))
                    .child(div().text_color(palette.muted).child(counts))
                    .child(div().flex_1())
                    .child(div().text_color(palette.text).child(disc))
                    .child(div().text_color(palette.muted).child(kind)),
            )
            .child(div().h(px(1.0)).w_full().bg(palette.border))
    }

    fn source_row(&self, cx: &mut Context<Self>) -> Div {
        let palette = Palette::get();
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().w(px(80.0)).text_color(palette.muted).child("Game data"))
            .child(
                field(
                    "path",
                    &self.path,
                    "path to a .cue disc image or an extracted folder",
                    self.editing == Some(Field::Path),
                )
                .flex_1()
                .on_click(cx.listener(|gui, _, _, cx| {
                    gui.editing = Some(Field::Path);
                    cx.notify();
                })),
            )
            .child(button("browse", Some("folder"), "Browse", true).on_click(cx.listener(|gui, _, _, cx| {
                gui.browse_for(Target::Data);
                cx.notify();
            })))
            .child(button("open", Some("play"), "Open", true).on_click(cx.listener(|gui, _, _, cx| {
                gui.editing = None;
                gui.open();
                cx.notify();
            })))
    }

    fn filter_row(&self, cx: &mut Context<Self>) -> Div {
        let palette = Palette::get();
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().w(px(80.0)).text_color(palette.muted).child("Filter"))
            .child(
                field("filter", &self.filter, "type to search, or press /", self.editing == Some(Field::Filter))
                    .flex_1()
                    .on_click(cx.listener(|gui, _, _, cx| {
                        gui.editing = Some(Field::Filter);
                        cx.notify();
                    })),
            )
            .child(
                chip(
                    "all",
                    "filter",
                    if self.playable_only { "Playable only" } else { "All items" },
                    self.playable_only,
                )
                .on_click(cx.listener(|gui, _, _, cx| {
                    gui.playable_only = !gui.playable_only;
                    gui.rebuild();
                    cx.notify();
                })),
            )
    }

    fn list_panel(&self, cx: &mut Context<Self>) -> Div {
        let body = match &self.state {
            State::Loading(_) => {
                notice("Opening game data...", "", "Reading the catalog. This takes a moment.", None, &[])
            }
            State::Failed(failed) => notice(
                "Open a disc image or folder",
                &failed.message,
                "Pick your Breath of Fire III .cue sheet, disc image, or a folder of extracted files. \
                 You can also start with -i PATH or set BOF3_GAME_DIR.",
                Some(
                    button("browse-empty", Some("folder"), "Browse", true)
                        .on_click(cx.listener(|gui, _, _, cx| {
                            gui.browse_for(Target::Data);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                &failed.probes,
            ),
            State::Ready(browser) if browser.rows.is_empty() => notice(
                "Nothing to list",
                "",
                "No item matches. Clear the filter, or press a to show all items.",
                None,
                &[],
            ),
            State::Ready(browser) => uniform_list(
                "rows",
                browser.rows.len(),
                cx.processor(|gui, range: std::ops::Range<usize>, _, cx| {
                    gui.visible.set((range.start, range.end));
                    let State::Ready(browser) = &gui.state else { return Vec::new() };
                    range
                        .map(|ix| {
                            let row = browser.rows[ix];
                            let open = browser.tree.is_open(&row);
                            let selected = ix == browser.selected;
                            let playing =
                                matches!(row, Row::Item(item) if gui.current.is_some_and(|c| std::ptr::eq(c, item)));
                            row_view(ix, row, open, selected, playing).on_click(cx.listener(
                                move |gui, event: &ClickEvent, _, cx| {
                                    gui.editing = None;
                                    gui.select(ix);
                                    match row {
                                        Row::Item(item) if event.click_count() >= 2 => gui.play(item),
                                        Row::Item(_) => {}
                                        _ => gui.toggle(None),
                                    }
                                    cx.notify();
                                },
                            ))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.scroll.clone())
            .size_full()
            .into_any_element(),
        };
        panel().flex_1().min_h_0().overflow_hidden().py_1().child(body)
    }

    fn now_playing(&self, cx: &mut Context<Self>) -> Div {
        let palette = Palette::get();
        let (position, duration) = self.deck.times();
        let loaded = self.deck.is_loaded();
        let paused = self.deck.paused();
        let shown = self.current.or_else(|| self.selected_item());
        let title = shown.map_or_else(|| "Nothing selected".to_string(), |item| list::label(item).to_string());
        let (state, state_color) = match (self.current, paused) {
            (Some(_), false) => ("Playing", palette.success),
            (Some(_), true) => ("Paused", palette.warning),
            (None, _) => ("Stopped", palette.muted),
        };
        let fraction = if duration > 0.0 { (position / duration) as f32 } else { 0.0 };
        let window = self.deck.loop_window();
        let mark = window
            .filter(|_| duration > 0.0 && self.looping)
            .map(|(a, b)| ((a / duration) as f32, (b / duration) as f32));
        let mut details = shown.map(|item| format!("{}  ·  {}", describe(item), item.id)).unwrap_or_default();
        if let Some((a, b)) = window {
            details.push_str(&format!("  ·  loop {} - {}", clock(a), clock(b)));
        }
        let same_as = match (&self.state, shown) {
            (State::Ready(browser), Some(item)) => browser.loaded.library.catalog().same_as_text(item),
            _ => None,
        }
        .map(|text| format!("Same as {text}"));
        let volume = self.deck.volume();
        let (left, right) = self.meters;
        let level = |label: &'static str, value: f32| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(12.0)).text_color(palette.muted).child(label))
                .child(meter(value).flex_1())
        };
        let controls = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .child(div().text_xl().font_weight(FontWeight::BOLD).text_color(palette.text).child(title))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(state_color).child(state))
                    .child(
                        div().flex_1().overflow_hidden().whitespace_nowrap().text_color(palette.muted).child(details),
                    )
                    .child(div().text_lg().text_color(palette.muted).child(format!(
                        "{} / {}",
                        clock(position),
                        clock(duration)
                    ))),
            )
            .children(same_as.map(|text| div().text_color(palette.muted).child(text)))
            .child(slider("seek", fraction, mark, &self.seek_bounds, cx, |gui, f| gui.seek_to(f)))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .w(px(150.0))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap_2()
                            .child(level("L", left))
                            .child(level("R", right)),
                    )
                    .child(
                        spectrum(self.spectrum.levels.clone(), self.spectrum.caps.clone(), self.spectrum.tint).flex_1(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(transport("prev", "prev", 44.0, true, false).on_click(cx.listener(|gui, _, _, cx| {
                        gui.step_item(false);
                        cx.notify();
                    })))
                    .child(
                        transport("play", if loaded && !paused { "pause" } else { "play" }, 56.0, true, true).on_click(
                            cx.listener(|gui, _, _, cx| {
                                gui.toggle_pause();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(transport("stop", "stop", 44.0, loaded, false).on_click(cx.listener(|gui, _, _, cx| {
                        gui.stop();
                        cx.notify();
                    })))
                    .child(transport("next", "next", 44.0, true, false).on_click(cx.listener(|gui, _, _, cx| {
                        gui.step_item(true);
                        cx.notify();
                    })))
                    .child(div().w(px(8.0)))
                    .child(chip("loop", "loop", "Loop", self.looping).on_click(cx.listener(|gui, _, _, cx| {
                        gui.toggle_loop();
                        cx.notify();
                    })))
                    .child(chip("play-on", "queue", "Play on", self.continuous).on_click(cx.listener(
                        |gui, _, _, cx| {
                            gui.continuous = !gui.continuous;
                            cx.notify();
                        },
                    )))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("mute")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(32.0))
                            .rounded(px(RADIUS))
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .child(
                                icon(if self.deck.muted() { "volume-off" } else { "volume" }, 18.0)
                                    .text_color(if self.deck.muted() { palette.warning } else { palette.muted }),
                            )
                            .on_click(cx.listener(|gui, _, _, cx| {
                                gui.deck.set_muted(!gui.deck.muted());
                                cx.notify();
                            })),
                    )
                    .child(
                        slider("volume", volume / 1.5, None, &self.volume_bounds, cx, |gui, f| {
                            gui.deck.set_volume(f * 1.5);
                        })
                        .w(px(120.0)),
                    )
                    .child(div().w(px(40.0)).child(format!("{}%", (volume * 100.0).round() as u32)))
                    .child(button("export", Some("export"), "Export", self.selected_item().is_some()).on_click(
                        cx.listener(|gui, _, _, cx| {
                            gui.open_export();
                            cx.notify();
                        }),
                    )),
            );
        panel().flex().gap_4().p_4().child(controls)
    }

    fn status_row(&self) -> Div {
        let palette = Palette::get();
        let text: SharedString = if let Some(save) = &self.save {
            let percent = (save.rendered() / save.duration().max(0.001) * 100.0).min(100.0) as u32;
            format!("Saving {}  {} / {}  ({percent}%)", save.id(), clock(save.rendered()), clock(save.duration()))
                .into()
        } else if !self.message.is_empty() {
            self.message.clone().into()
        } else {
            let output = format!("Output: {}", self.deck.device);
            output.into()
        };
        let is_problem = self.save.is_none()
            && (self.message.starts_with("Cannot")
                || self.message.starts_with("Could not")
                || self.message.contains("error"));
        div()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(if is_problem { palette.danger } else { palette.muted })
                    .child(text),
            )
            .child(
                div().text_color(palette.muted).child(
                    "Space play  ·  Enter play  ·  ↑ ↓ move  ·  n b next prev  ·  / filter  ·  e export  ·  ? help",
                ),
            )
    }

    fn help_card() -> Div {
        let palette = Palette::get();
        let rows = HELP.iter().map(|(keys, what)| {
            div()
                .flex()
                .gap_4()
                .child(div().w(px(150.0)).text_color(palette.accent).child(*keys))
                .child(div().child(*what))
        });
        panel()
            .flex()
            .flex_col()
            .gap_1()
            .p_5()
            .w(px(620.0))
            .child(div().pb_2().text_lg().font_weight(FontWeight::BOLD).child("Keys"))
            .children(rows)
            .child(div().pt_3().text_color(palette.muted).child("Press any key to close."))
    }

    fn export_card(&self, menu: &ExportMenu, cx: &mut Context<Self>) -> Div {
        let palette = Palette::get();
        let rows = menu.rows.iter().enumerate().map(|(ix, (format, path))| {
            let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            div()
                .flex()
                .gap_4()
                .px_3()
                .py_1()
                .rounded(px(RADIUS))
                .when(ix == menu.selected, |row| row.bg(palette.selection))
                .child(div().w(px(150.0)).child(format.label()))
                .child(div().text_color(palette.muted).child(name))
        });
        let renders = menu.rows.iter().any(|(format, _)| format.export_format().is_some());
        let settings = if renders {
            format!("[ ] plays of the loop: {}    + - Ogg quality: {}", menu.loops.unwrap_or(1), menu.quality.label())
        } else {
            String::new()
        };
        panel()
            .flex()
            .flex_col()
            .gap_1()
            .p_5()
            .w(px(620.0))
            .child(div().text_lg().font_weight(FontWeight::BOLD).child(format!("Export {}", list::label(menu.item))))
            .child(
                div()
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .text_color(palette.muted)
                            .child(format!("Folder: {}", crate::save::absolute(&self.export_dir).display())),
                    )
                    .child(button("folder", Some("folder"), "Change", true).on_click(cx.listener(|gui, _, _, cx| {
                        gui.browse_for(Target::Export);
                        cx.notify();
                    }))),
            )
            .children(rows)
            .child(div().pt_2().text_color(palette.muted).child(settings))
            .child(
                div()
                    .text_color(palette.muted)
                    .child("Up Down choose  ·  Enter save  ·  f change folder  ·  Esc cancel"),
            )
    }
}

/// An empty or error state: what to do, what failed, an optional action and details.
fn notice(title: &str, problem: &str, next: &str, action: Option<AnyElement>, details: &[String]) -> AnyElement {
    let palette = Palette::get();
    div()
        .size_full()
        .p_8()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_2xl().font_weight(FontWeight::BOLD).text_color(palette.text).child(title.to_string()))
        .when(!problem.is_empty(), |column| column.child(div().text_color(palette.warning).child(problem.to_string())))
        .child(div().text_color(palette.text).child(next.to_string()))
        .children(action)
        .when(!details.is_empty(), |column| {
            column
                .child(div().pt_4().text_color(palette.muted).child(format!("Places tried ({}):", details.len())))
                .children(details.iter().map(|line| div().text_xs().text_color(palette.muted).child(line.clone())))
        })
        .into_any_element()
}

/// One row of the list.
fn row_view(ix: usize, row: Row, open: bool, selected: bool, playing: bool) -> gpui::Stateful<Div> {
    let palette = Palette::get();
    let base = div()
        .id(ix)
        .h(px(ROW))
        .w_full()
        .px_3()
        .rounded(px(RADIUS))
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .border_1()
        .border_color(if selected { palette.focus_ring } else { transparent_black() })
        .when(selected, |row| row.bg(palette.selection))
        .when(!selected, |row| row.hover(|style| style.bg(palette.hover)));
    let chevron = |color| icon(if open { "chevron-down" } else { "chevron-right" }, 14.0).text_color(color);
    match row {
        Row::Kind(kind, count) => base
            .font_weight(FontWeight::BOLD)
            .child(chevron(palette.accent))
            .child(icon(kind_icon(kind), 16.0).text_color(palette.accent))
            .child(div().text_color(palette.text).child(kind.label()))
            .child(div().text_color(palette.muted).font_weight(FontWeight::NORMAL).child(count.to_string())),
        Row::Family(_, family, count) => base
            .pl(px(34.0))
            .child(chevron(palette.muted))
            .child(div().child(family))
            .child(div().text_color(palette.muted).child(count.to_string())),
        Row::Item(item) => item_row(base, item, playing),
    }
}

fn item_row(base: gpui::Stateful<Div>, item: &'static Item, playing: bool) -> gpui::Stateful<Div> {
    let palette = Palette::get();
    let length = item.duration.map_or_else(String::new, clock);
    let color = match (playing, item.playable) {
        (true, _) => palette.success,
        (false, true) => palette.text,
        (false, false) => palette.disabled,
    };
    base.pl(px(56.0))
        .child(div().w(px(14.0)).child(icon("play", 12.0).text_color(if playing {
            palette.success
        } else {
            transparent_black()
        })))
        .child(
            div()
                .w(px(280.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(color)
                .child(list::label(item).to_string()),
        )
        .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(palette.muted).child(if item.playable {
            item.id.clone()
        } else {
            format!("{}  ·  no audio", item.id)
        }))
        .child(div().text_color(palette.muted).child(length))
}

/// The icon of a kind.
fn kind_icon(kind: crate::catalog::Kind) -> &'static str {
    use crate::catalog::Kind;
    match kind {
        Kind::Music => "music",
        Kind::Sample => "volume",
        Kind::Xa => "wave",
        Kind::Cdda => "disc",
        Kind::Bank => "box",
    }
}
