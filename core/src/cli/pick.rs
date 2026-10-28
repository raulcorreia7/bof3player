// SPDX-License-Identifier: MIT
//! A folder browser for choosing game data when none was found, so the player needs no switches.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::tui::{ACCENT, hints};
use crate::discover::{self, Entry};

struct Picker {
    dir: PathBuf,
    entries: Vec<Entry>,
    error: Option<String>,
    state: ListState,
    folders_only: bool,
}

impl Picker {
    fn new(dir: PathBuf, folders_only: bool) -> Self {
        let mut picker =
            Self { dir: PathBuf::new(), entries: Vec::new(), error: None, state: ListState::default(), folders_only };
        picker.go(dir);
        picker
    }

    fn go(&mut self, dir: PathBuf) {
        match discover::browse(&dir) {
            Ok(entries) => {
                self.entries = entries.into_iter().filter(|entry| entry.is_dir || !self.folders_only).collect();
                self.error = None;
                self.dir = dir;
            }
            Err(error) => self.error = Some(format!("Cannot open {}: {error}.", dir.display())),
        }
        self.state.select(if self.entries.is_empty() { None } else { Some(0) });
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let came_from = self.dir.file_name().map(|name| name.to_string_lossy().into_owned());
            self.go(parent);
            if let Some(at) = came_from.and_then(|name| self.entries.iter().position(|entry| entry.name == name)) {
                self.state.select(Some(at));
            }
        }
    }

    fn selected(&self) -> Option<&Entry> {
        self.state.selected().and_then(|at| self.entries.get(at))
    }

    fn step(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1) as isize;
        if let Some(at) = self.state.selected() {
            self.state.select(Some((at as isize + delta).clamp(0, last) as usize));
        }
    }

    fn draw(&mut self, frame: &mut Frame<'_>, message: &str) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(3), Constraint::Length(1)])
            .split(frame.area());
        let head = vec![
            Line::styled("♫ Breath of Fire III: choose your game data", Style::new().add_modifier(Modifier::BOLD)),
            Line::styled(message.to_string(), Style::new().fg(Color::Yellow)),
            Line::styled(self.dir.display().to_string(), Style::new().fg(ACCENT)),
        ];
        frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: true }), rows[0]);
        let items: Vec<ListItem<'_>> = self
            .entries
            .iter()
            .map(|entry| {
                let (mark, style) = if entry.is_dir {
                    ("▸ ", Style::new().fg(ACCENT))
                } else {
                    ("◉ ", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
                };
                ListItem::new(Line::from(vec![Span::styled(mark, style), Span::styled(entry.name.clone(), style)]))
            })
            .collect();
        let title = self.error.clone().unwrap_or_else(|| " Folders, .cue and disc images ".to_string());
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, rows[1], &mut self.state);
        let help = hints(
            &[("j/k", "move"), ("Enter", "open"), ("h", "up"), ("o", "use this folder"), ("q", "quit")],
            2,
            frame.area().width as usize,
        );
        frame.render_widget(Paragraph::new(help), rows[2]);
    }
}

/// Let the user walk the folders and pick a disc image, a cue sheet or a game folder. `message`
/// says why. Returns `None` when the user quits.
pub fn pick_data(message: &str) -> Result<Option<PathBuf>, String> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, message, discover::start_dir(), false);
    ratatui::restore();
    result
}

/// Let the user choose a folder, starting at `start`, on an already running terminal.
pub fn pick_folder(terminal: &mut DefaultTerminal, message: &str, start: &Path) -> Result<Option<PathBuf>, String> {
    run(terminal, message, start.to_path_buf(), true)
}

fn run(
    terminal: &mut DefaultTerminal,
    message: &str,
    start: PathBuf,
    folders_only: bool,
) -> Result<Option<PathBuf>, String> {
    let mut picker = Picker::new(start, folders_only);
    let result = (|| -> std::io::Result<Option<PathBuf>> {
        loop {
            terminal.draw(|frame| picker.draw(frame, message))?;
            let Event::Key(key) = event::read()? else { continue };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                KeyCode::Char('j') | KeyCode::Down => picker.step(1),
                KeyCode::Char('k') | KeyCode::Up => picker.step(-1),
                KeyCode::PageDown => picker.step(15),
                KeyCode::PageUp => picker.step(-15),
                KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => picker.up(),
                KeyCode::Char('o') => return Ok(Some(picker.dir.clone())),
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                    if let Some(entry) = picker.selected().cloned() {
                        if entry.is_dir {
                            picker.go(entry.path);
                        } else {
                            return Ok(Some(entry.path));
                        }
                    }
                }
                _ => {}
            }
        }
    })();
    result.map_err(|error| error.to_string())
}
