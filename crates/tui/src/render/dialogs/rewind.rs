use crate::render::{crlf, draw_bar};
use crate::theme;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::{Attribute, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::QueueableCommand;

use super::{end_dialog_draw, truncate_str, DialogResult, ListState};

pub struct RewindDialog {
    turns: Vec<(usize, String)>,
    list: ListState,
    restore_vim_insert: bool,
}

impl RewindDialog {
    pub fn new(
        turns: Vec<(usize, String)>,
        restore_vim_insert: bool,
        max_height: Option<u16>,
    ) -> Self {
        let mut list = ListState::new(turns.len(), max_height, 4);
        list.selected = turns.len().saturating_sub(1);
        list.scroll_offset = turns.len().saturating_sub(list.max_visible);
        Self {
            turns,
            list,
            restore_vim_insert,
        }
    }
}

impl super::Dialog for RewindDialog {
    fn height(&self) -> u16 {
        self.list.height(self.turns.len())
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
        match code {
            KeyCode::Enter => {
                return Some(DialogResult::Rewind {
                    block_idx: Some(self.turns[self.list.selected].0),
                    restore_vim_insert: self.restore_vim_insert,
                })
            }
            KeyCode::Esc => {
                return Some(DialogResult::Rewind {
                    block_idx: None,
                    restore_vim_insert: self.restore_vim_insert,
                })
            }
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
                return Some(DialogResult::Rewind {
                    block_idx: None,
                    restore_vim_insert: self.restore_vim_insert,
                })
            }
            KeyCode::Up | KeyCode::Char('k') => self.list.select_prev(),
            KeyCode::Down | KeyCode::Char('j') => self.list.select_next(self.turns.len()),
            _ => {}
        }
        None
    }

    fn draw(&mut self, start_row: u16) {
        let Some((mut out, w, _)) = self.list.begin_draw(start_row, self.turns.len()) else {
            return;
        };

        draw_bar(&mut out, w, None, None, theme::accent());
        crlf(&mut out);

        let _ = out.queue(SetAttribute(Attribute::Dim));
        let _ = out.queue(Print(" Rewind to:"));
        let _ = out.queue(SetAttribute(Attribute::Reset));
        crlf(&mut out);

        for (i, (_, ref full_text)) in self
            .turns
            .iter()
            .enumerate()
            .take(self.list.visible_range(self.turns.len()).end)
            .skip(self.list.visible_range(self.turns.len()).start)
        {
            let label = full_text.lines().next().unwrap_or("");
            let num = i + 1;
            let max_label = w.saturating_sub(8);
            let truncated = truncate_str(label, max_label);
            let _ = out.queue(Print("  "));
            if i == self.list.selected {
                let _ = out.queue(SetAttribute(Attribute::Dim));
                let _ = out.queue(Print(format!("{}.", num)));
                let _ = out.queue(SetAttribute(Attribute::Reset));
                let _ = out.queue(Print(" "));
                let _ = out.queue(SetForegroundColor(theme::accent()));
                let _ = out.queue(Print(&truncated));
                let _ = out.queue(ResetColor);
            } else {
                let _ = out.queue(SetAttribute(Attribute::Dim));
                let _ = out.queue(Print(format!("{}. ", num)));
                let _ = out.queue(SetAttribute(Attribute::Reset));
                let _ = out.queue(Print(&truncated));
            }
            crlf(&mut out);
        }

        crlf(&mut out);
        let _ = out.queue(SetAttribute(Attribute::Dim));
        let _ = out.queue(Print(" enter: select  esc: cancel"));
        let _ = out.queue(SetAttribute(Attribute::Reset));
        end_dialog_draw(&mut out);
    }
}
