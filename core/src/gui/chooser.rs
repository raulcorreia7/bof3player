// SPDX-License-Identifier: MIT
//! A folder and file chooser for the game data and the export folder, so the window needs no
//! switches.

use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder;
use gpui::{
    ClickEvent, Context, Div, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div,
    px, transparent_black,
};

use super::Gui;
use super::icons::icon;
use super::widgets::{Palette, RADIUS, button, panel};
use crate::discover::{self, Entry};

/// What the chooser sets when the user picks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The game data: a folder, a cue sheet or a disc image.
    Data,
    /// The folder exports are written into.
    Export,
}

/// The folder being browsed.
pub struct Chooser {
    target: Target,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: usize,
    error: Option<String>,
}

impl Chooser {
    pub fn new(target: Target, start: &Path) -> Self {
        let mut chooser = Self { target, dir: PathBuf::new(), entries: Vec::new(), selected: 0, error: None };
        chooser.go(start.to_path_buf());
        chooser
    }

    fn go(&mut self, dir: PathBuf) {
        match discover::browse(&dir) {
            Ok(entries) => {
                self.entries = entries.into_iter().filter(|e| e.is_dir || self.target == Target::Data).collect();
                self.error = None;
                self.dir = dir;
            }
            Err(error) => self.error = Some(format!("Cannot open {}: {error}.", dir.display())),
        }
        self.selected = 0;
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let came_from = self.dir.file_name().map(|name| name.to_string_lossy().into_owned());
            self.go(parent);
            if let Some(at) = came_from.and_then(|name| self.entries.iter().position(|e| e.name == name)) {
                self.selected = at;
            }
        }
    }
}

impl Gui {
    /// Open the chooser for `target`, starting near the current choice.
    pub(super) fn browse_for(&mut self, target: Target) {
        let start = match target {
            Target::Export => crate::save::absolute(&self.export_dir),
            Target::Data => {
                let typed = PathBuf::from(self.path.trim());
                let near = if typed.is_dir() { Some(typed) } else { typed.parent().map(Path::to_path_buf) };
                near.filter(|dir| dir.is_dir()).unwrap_or_else(discover::start_dir)
            }
        };
        self.editing = None;
        self.chooser = Some(Chooser::new(target, &start));
    }

    /// Apply the choice and close the chooser.
    fn finish_choice(&mut self, path: PathBuf) {
        let Some(chooser) = self.chooser.take() else { return };
        match chooser.target {
            Target::Data => {
                self.path = path.display().to_string();
                self.open();
            }
            Target::Export => {
                self.message = format!("Exports go to {}.", path.display());
                self.export_dir = path;
            }
        }
    }

    fn enter_entry(&mut self, index: usize) {
        let Some(chooser) = &mut self.chooser else { return };
        let Some(entry) = chooser.entries.get(index).cloned() else { return };
        if entry.is_dir {
            chooser.go(entry.path);
        } else {
            self.finish_choice(entry.path);
        }
    }

    pub(super) fn chooser_key(&mut self, name: &str) {
        let Some(chooser) = &mut self.chooser else { return };
        let last = chooser.entries.len().saturating_sub(1);
        match name {
            "escape" | "q" => self.chooser = None,
            "j" | "down" => chooser.selected = (chooser.selected + 1).min(last),
            "k" | "up" => chooser.selected = chooser.selected.saturating_sub(1),
            "h" | "left" | "backspace" => chooser.up(),
            "o" => {
                let dir = chooser.dir.clone();
                self.finish_choice(dir);
            }
            "enter" | "l" | "right" => {
                let at = chooser.selected;
                self.enter_entry(at);
            }
            _ => {}
        }
    }

    pub(super) fn chooser_card(&self, chooser: &Chooser, cx: &mut Context<Self>) -> Div {
        let palette = Palette::get();
        let title = match chooser.target {
            Target::Data => "Choose game data",
            Target::Export => "Choose the export folder",
        };
        let hint = match chooser.target {
            Target::Data => "Open a .cue sheet or a disc image, or use this folder if it holds the extracted files.",
            Target::Export => "Open folders to find the one exports are written into.",
        };
        let rows = chooser.entries.iter().enumerate().map(|(ix, entry)| {
            let (name, tint) = if entry.is_dir { ("folder", palette.accent) } else { ("disc", palette.success) };
            let is_dir = entry.is_dir;
            div()
                .id(ix)
                .h(px(30.0))
                .w_full()
                .px_3()
                .rounded(px(RADIUS))
                .flex()
                .items_center()
                .gap_3()
                .cursor_pointer()
                .border_1()
                .border_color(if ix == chooser.selected { palette.focus_ring } else { transparent_black() })
                .when(ix == chooser.selected, |row| row.bg(palette.selection))
                .when(ix != chooser.selected, |row| row.hover(|style| style.bg(palette.hover)))
                .child(icon(name, 16.0).text_color(tint))
                .child(div().child(entry.name.clone()))
                .on_click(cx.listener(move |gui, event: &ClickEvent, _, cx| {
                    if let Some(chooser) = &mut gui.chooser {
                        chooser.selected = ix;
                    }
                    if event.click_count() >= 2 || !is_dir {
                        gui.enter_entry(ix);
                    }
                    cx.notify();
                }))
        });
        let empty = chooser.entries.is_empty().then(|| {
            div().p_3().text_color(palette.muted).child(match chooser.target {
                Target::Data => "No folders or disc images here.",
                Target::Export => "No folders here.",
            })
        });
        panel()
            .flex()
            .flex_col()
            .gap_2()
            .p_5()
            .w(px(640.0))
            .h(px(520.0))
            .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
            .child(div().text_color(palette.muted).child(hint))
            .child(div().text_color(palette.accent).child(chooser.dir.display().to_string()))
            .when_some(chooser.error.clone(), |card, error| card.child(div().text_color(palette.warning).child(error)))
            .child(
                div()
                    .id("chooser-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows)
                    .children(empty),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_2()
                    .child(button("up", None, "Up", true).on_click(cx.listener(|gui, _, _, cx| {
                        if let Some(chooser) = &mut gui.chooser {
                            chooser.up();
                        }
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .child(button("cancel", None, "Cancel", true).on_click(cx.listener(|gui, _, _, cx| {
                        gui.chooser = None;
                        cx.notify();
                    })))
                    .child(button("use", Some("folder"), "Use this folder", true).on_click(cx.listener(
                        |gui, _, _, cx| {
                            if let Some(dir) = gui.chooser.as_ref().map(|c| c.dir.clone()) {
                                gui.finish_choice(dir);
                            }
                            cx.notify();
                        },
                    ))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette.muted)
                    .child("Up Down choose  ·  Enter open  ·  Backspace up  ·  o use this folder  ·  Esc cancel"),
            )
    }
}
