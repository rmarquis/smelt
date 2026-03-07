mod confirm;
mod help;
mod ps;
mod question;
mod resume;
mod rewind;

pub use confirm::ConfirmDialog;
pub use help::HelpDialog;
pub use ps::PsDialog;
pub use question::{parse_questions, Question, QuestionDialog, QuestionOption};
pub use resume::ResumeDialog;
pub use rewind::RewindDialog;

use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::{cursor, style::Print, terminal, QueueableCommand};
use std::io::Write;

use super::{chunk_line, crlf, ConfirmChoice, RenderOut};

pub enum DialogResult {
    Dismissed,
    Confirm {
        choice: ConfirmChoice,
        message: Option<String>,
        tool_name: String,
        request_id: u64,
    },
    Question {
        answer: Option<String>,
        request_id: u64,
    },
    Rewind {
        block_idx: Option<usize>,
        restore_vim_insert: bool,
    },
    Resume {
        session_id: Option<String>,
    },
    PsClosed,
}

pub trait Dialog {
    /// Whether the agent is blocked on a reply for this dialog.
    fn blocks_agent(&self) -> bool {
        false
    }
    fn height(&self) -> u16;
    fn mark_dirty(&mut self);
    fn draw(&mut self, start_row: u16);
    fn handle_resize(&mut self);
    fn anchor_row(&self) -> Option<u16>;
    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<DialogResult>;
}

pub(crate) struct ListState {
    pub selected: usize,
    pub scroll_offset: usize,
    pub max_visible: usize,
    max_height: Option<u16>,
    overhead: u16,
    pub anchor_row: Option<u16>,
    pub dirty: bool,
}

impl ListState {
    pub fn new(item_count: usize, max_height: Option<u16>, overhead: u16) -> Self {
        let max_visible = Self::cap(max_height, overhead, item_count);
        Self {
            selected: 0,
            scroll_offset: 0,
            max_visible,
            max_height,
            overhead,
            anchor_row: None,
            dirty: true,
        }
    }

    fn cap(max_height: Option<u16>, overhead: u16, item_count: usize) -> usize {
        max_height
            .map(|h| (h as usize).saturating_sub(overhead as usize))
            .unwrap_or(usize::MAX)
            .min(item_count)
    }

    pub fn height(&self, item_count: usize) -> u16 {
        let wanted = (item_count as u16).saturating_add(self.overhead);
        if let Some(cap) = self.max_height {
            wanted.min(cap)
        } else {
            wanted
        }
    }

    pub fn set_items(&mut self, count: usize) {
        self.max_visible = Self::cap(self.max_height, self.overhead, count);
        if count == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(count - 1);
            self.scroll_offset = self
                .scroll_offset
                .min(count.saturating_sub(self.max_visible));
        }
        self.dirty = true;
    }

    pub fn handle_resize(&mut self) {
        self.anchor_row = None;
        self.dirty = true;
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
            self.dirty = true;
        }
    }

    pub fn select_next(&mut self, item_count: usize) {
        if self.selected + 1 < item_count {
            self.selected += 1;
            if self.selected >= self.scroll_offset + self.max_visible {
                self.scroll_offset = self.selected + 1 - self.max_visible;
            }
            self.dirty = true;
        }
    }

    pub fn page_up(&mut self) {
        let half = self.max_visible / 2;
        self.selected = self.selected.saturating_sub(half.max(1));
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        self.dirty = true;
    }

    pub fn page_down(&mut self, item_count: usize) {
        let half = self.max_visible / 2;
        self.selected = (self.selected + half.max(1)).min(item_count.saturating_sub(1));
        if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected + 1 - self.max_visible;
        }
        self.dirty = true;
    }

    pub fn begin_draw(
        &mut self,
        start_row: u16,
        item_count: usize,
    ) -> Option<(RenderOut, usize, u16)> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;

        let mut out = RenderOut::scroll();
        let (width, height) = terminal::size().unwrap_or((80, 24));

        let wanted_rows = (item_count as u16).saturating_add(self.overhead);
        let (bar_row, granted) = begin_dialog_draw(
            &mut out,
            start_row,
            wanted_rows,
            height,
            self.max_height,
            &mut self.anchor_row,
        );
        self.max_visible = (granted as usize)
            .saturating_sub(self.overhead as usize)
            .min(item_count);
        if item_count == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(item_count - 1);
            self.scroll_offset = self
                .scroll_offset
                .min(item_count.saturating_sub(self.max_visible));
        }

        Some((out, width as usize, bar_row))
    }

    pub fn visible_range(&self, item_count: usize) -> std::ops::Range<usize> {
        let end = (self.scroll_offset + self.max_visible).min(item_count);
        self.scroll_offset..end
    }
}

pub(crate) struct TextArea {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
}

impl TextArea {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn visual_row_count(&self, wrap_w: usize) -> u16 {
        self.lines
            .iter()
            .map(|l| chunk_line(l, wrap_w).len() as u16)
            .sum()
    }

    pub fn wrap(&self, wrap_w: usize) -> (Vec<String>, (usize, usize)) {
        let mut visual = Vec::new();
        let mut cursor = (0, 0);

        for (li, line) in self.lines.iter().enumerate() {
            let vis_start = visual.len();
            let chunks = chunk_line(line, wrap_w);
            visual.extend(chunks);

            if li == self.row {
                let char_count = line.chars().count();
                let col = self.col.min(char_count);
                if char_count == 0 || wrap_w == 0 {
                    cursor = (vis_start, col);
                } else {
                    let vis_offset = col / wrap_w;
                    let vis_col = col % wrap_w;
                    let num_vis = visual.len() - vis_start;
                    if vis_offset >= num_vis {
                        cursor = (
                            vis_start + num_vis - 1,
                            visual[vis_start + num_vis - 1].chars().count(),
                        );
                    } else {
                        cursor = (vis_start + vis_offset, vis_col);
                    }
                }
            }
        }

        (visual, cursor)
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        let byte = char_to_byte(&self.lines[self.row], self.col);
        self.lines[self.row].insert(byte, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let byte = char_to_byte(&self.lines[self.row], self.col);
        let rest = self.lines[self.row][byte..].to_string();
        self.lines[self.row].truncate(byte);
        self.row += 1;
        self.col = 0;
        self.lines.insert(self.row, rest);
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let byte = char_to_byte(&self.lines[self.row], self.col);
            self.lines[self.row].remove(byte);
        } else if self.row > 0 {
            let removed = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&removed);
        }
    }

    pub fn delete_word_backward(&mut self) {
        if self.col == 0 {
            return;
        }
        let line = &self.lines[self.row];
        let byte_pos = char_to_byte(line, self.col);
        let target = crate::vim::word_backward_pos(line, byte_pos, crate::vim::CharClass::Word);
        let target_col = line[..target].chars().count();
        let end_byte = char_to_byte(&self.lines[self.row], self.col);
        self.lines[self.row].drain(target..end_byte);
        self.col = target_col;
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let len = self.lines[self.row].chars().count();
        if self.col < len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match (code, modifiers) {
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => self.insert_char(c),
            (KeyCode::Enter, _) => self.insert_newline(),
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
            {
                self.delete_word_backward()
            }
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Left, _) => self.move_left(),
            (KeyCode::Right, _) => self.move_right(),
            (KeyCode::Up, _) => self.move_up(),
            (KeyCode::Down, _) => self.move_down(),
            (KeyCode::Home, _) => self.move_home(),
            (KeyCode::End, _) => self.move_end(),
            _ => return false,
        }
        true
    }
}

pub(crate) fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

pub(crate) fn render_inline_textarea(
    out: &mut RenderOut,
    ta: &TextArea,
    editing: bool,
    text_col: u16,
    wrap_w: usize,
    mut row: u16,
) -> (u16, Option<(u16, u16)>) {
    let (vis_lines, vis_cursor) = ta.wrap(wrap_w);
    let pad: String = " ".repeat(text_col as usize);
    let mut cursor_pos = None;
    for (vi, vl) in vis_lines.iter().enumerate() {
        if vi == 0 {
            let _ = out.queue(Print(", "));
        } else {
            let _ = out.queue(Print(&pad));
        }
        let _ = out.queue(Print(vl));
        if editing && vi == vis_cursor.0 {
            cursor_pos = Some((text_col + vis_cursor.1 as u16, row));
        }
        crlf(out);
        row += 1;
    }
    (row, cursor_pos)
}

pub(crate) fn begin_dialog_draw(
    out: &mut RenderOut,
    start_row: u16,
    content_rows: u16,
    height: u16,
    max_rows: Option<u16>,
    anchor_row: &mut Option<u16>,
) -> (u16, u16) {
    let _ = out.queue(terminal::BeginSynchronizedUpdate);
    let _ = out.queue(cursor::Hide);

    let granted = if let Some(cap) = max_rows {
        content_rows.min(cap)
    } else {
        content_rows
    };
    let granted = granted.min(height);

    let bar_row = if let Some(anchor) = *anchor_row {
        anchor
    } else {
        let available = height.saturating_sub(start_row);
        let row = if granted <= available {
            start_row
        } else {
            let deficit = granted.saturating_sub(available);
            let _ = out.queue(terminal::ScrollUp(deficit));
            height.saturating_sub(granted)
        };
        *anchor_row = Some(row);
        row
    };

    let _ = out.queue(cursor::MoveTo(0, bar_row));
    out.row = Some(bar_row);
    (bar_row, granted)
}

pub(crate) fn end_dialog_draw(out: &mut RenderOut) {
    let _ = out.queue(terminal::Clear(terminal::ClearType::FromCursorDown));
    let _ = out.queue(terminal::EndSynchronizedUpdate);
    let _ = out.flush();
}

pub(crate) fn finish_dialog_frame(
    out: &mut RenderOut,
    cursor_pos: Option<(u16, u16)>,
    editing: bool,
) {
    if editing {
        if let Some((col, r)) = cursor_pos {
            let _ = out.queue(cursor::MoveTo(col, r));
        }
        let _ = out.queue(cursor::Show);
    }
    let _ = out.queue(terminal::EndSynchronizedUpdate);
    let _ = out.flush();
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max.saturating_sub(1)).collect();
    truncated.push('\u{2026}');
    truncated
}
