use crate::render::{crlf, draw_bar};
use crate::theme;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute, SetForegroundColor};
use crossterm::{terminal, QueueableCommand};

use super::{end_dialog_draw, DialogResult, ListState};

pub struct HelpDialog {
    list: ListState,
    /// Total content rows (static, computed once).
    total_rows: usize,
}

impl Default for HelpDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpDialog {
    /// Number of content lines in the help sections (3 prefixes + 11 keys + 1 separator).
    const TOTAL_ROWS: usize = 15;

    pub fn new() -> Self {
        Self {
            list: ListState::new(0, None, 3),
            total_rows: Self::TOTAL_ROWS,
        }
    }
}

impl super::Dialog for HelpDialog {
    fn height(&self) -> u16 {
        self.list.height(self.total_rows)
    }

    fn mark_dirty(&mut self) {
        self.list.dirty = true;
    }

    fn anchor_row(&self) -> Option<u16> {
        self.list.anchor_row
    }

    fn handle_resize(&mut self) {
        self.list.handle_resize();
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<DialogResult> {
        if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            return Some(DialogResult::Dismissed);
        }
        match code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => {
                Some(DialogResult::Dismissed)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.list.scroll_offset > 0 {
                    self.list.scroll_offset -= 1;
                    self.list.dirty = true;
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Clamp eagerly; draw() will also clamp but this prevents
                // unbounded accumulation from rapid key presses.
                let max_scroll = self.total_rows.saturating_sub(self.list.max_visible);
                if self.list.scroll_offset < max_scroll {
                    self.list.scroll_offset += 1;
                    self.list.dirty = true;
                }
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self, start_row: u16, sync_started: bool) {
        // Each section renders as a heading row followed by entry rows.
        let sections: &[(&str, &[(&str, &str)])] = &[
            (
                "prefixes",
                &[
                    (
                        "/command",
                        "slash commands  (try /resume, /compact, /fork, /ps, /vim…)",
                    ),
                    ("@<path>", "attach a file or URL"),
                    ("!<cmd>", "run a shell command"),
                ],
            ),
            (
                "keys",
                &[
                    ("enter", "send message"),
                    ("ctrl+j  shift+enter", "insert newline"),
                    ("ctrl+c", "cancel / interrupt"),
                    ("ctrl+r", "search input history"),
                    ("ctrl+t", "cycle reasoning effort"),
                    ("shift+tab", "cycle mode  (normal → plan → apply → yolo)"),
                    ("ctrl+u / ctrl+d", "scroll up / down"),
                    ("ctrl+a / ctrl+e", "line start / end"),
                    ("ctrl+w  alt+bs", "delete word backward"),
                    ("tab", "autocomplete"),
                    ("esc  ctrl+c", "cancel / close dialog"),
                ],
            ),
        ];

        let label_col = sections
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|(k, _)| k.len()))
            .max()
            .unwrap_or(0)
            + 4;

        // Collect content lines for scrolling
        let mut content_lines: Vec<(&str, &str)> = Vec::new();
        for (si, (_, entries)) in sections.iter().enumerate() {
            for &(label, detail) in *entries {
                content_lines.push((label, detail));
            }
            if si + 1 < sections.len() {
                content_lines.push(("", "")); // blank separator
            }
        }
        let total_content = content_lines.len();

        let Some((mut out, w, _)) = self.list.begin_draw(start_row, total_content, sync_started)
        else {
            return;
        };
        let max_visible = self.list.max_visible;

        // Clamp scroll
        let max_scroll = total_content.saturating_sub(max_visible);
        self.list.scroll_offset = self.list.scroll_offset.min(max_scroll);

        draw_bar(&mut out, w, None, None, theme::accent());
        crlf(&mut out);

        let _ = out.queue(SetAttribute(Attribute::Dim));
        let _ = out.queue(Print(" help"));
        let _ = out.queue(SetAttribute(Attribute::Reset));
        crlf(&mut out);
        crlf(&mut out);

        for &(label, detail) in content_lines
            .iter()
            .skip(self.list.scroll_offset)
            .take(max_visible)
        {
            if label.is_empty() && detail.is_empty() {
                crlf(&mut out);
            } else {
                let _ = out.queue(Print("  "));
                let _ = out.queue(SetForegroundColor(theme::MUTED));
                let _ = out.queue(Print(label));
                let _ = out.queue(crossterm::style::ResetColor);
                let padding = " ".repeat(label_col.saturating_sub(label.len()));
                let _ = out.queue(SetAttribute(Attribute::Dim));
                let _ = out.queue(Print(format!("{padding}{detail}")));
                let _ = out.queue(SetAttribute(Attribute::Reset));
                let _ = out.queue(terminal::Clear(terminal::ClearType::UntilNewLine));
                crlf(&mut out);
            }
        }

        end_dialog_draw(&mut out);
    }
}
