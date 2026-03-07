use crate::render::{crlf, draw_bar, ResumeEntry};
use crate::{session, theme};
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::{Attribute, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::QueueableCommand;
use std::time::Instant;

use super::{end_dialog_draw, truncate_str, DialogResult, ListState};

pub struct ResumeDialog {
    entries: Vec<ResumeEntry>,
    current_cwd: String,
    query: String,
    workspace_only: bool,
    filtered: Vec<ResumeEntry>,
    list: ListState,
    pending_d: bool,
    last_drawn: Instant,
}

impl ResumeDialog {
    pub fn new(entries: Vec<ResumeEntry>, current_cwd: String, max_height: Option<u16>) -> Self {
        let filtered = filter_resume_entries(&entries, "", true, &current_cwd);
        let list = ListState::new(filtered.len().max(1), max_height, 4);
        Self {
            entries,
            current_cwd,
            query: String::new(),
            workspace_only: true,
            filtered,
            list,
            pending_d: false,
            last_drawn: Instant::now(),
        }
    }

    fn refilter(&mut self) {
        self.filtered = filter_resume_entries(
            &self.entries,
            &self.query,
            self.workspace_only,
            &self.current_cwd,
        );
        self.list.set_items(self.filtered.len().max(1));
    }

    fn delete_selected(&mut self) {
        if let Some(entry) = self.filtered.get(self.list.selected) {
            let id = entry.id.clone();
            session::delete(&id);
            self.entries.retain(|e| e.id != id);
            self.refilter();
        }
    }
}

impl super::Dialog for ResumeDialog {
    fn mark_dirty(&mut self) {
        self.list.dirty = true;
    }

    fn anchor_row(&self) -> Option<u16> {
        self.list.anchor_row
    }

    fn handle_resize(&mut self) {
        self.list.handle_resize();
        self.refilter();
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<DialogResult> {
        // Check for DD completion before anything else.
        if self.pending_d {
            self.pending_d = false;
            if code == KeyCode::Char('d') && mods == KeyModifiers::NONE {
                self.delete_selected();
                return None;
            }
            // 'd' followed by something else: insert both as query chars.
            self.query.push('d');
            // Fall through to handle the current key normally.
        }

        match (code, mods) {
            (KeyCode::Enter, _) => {
                return Some(DialogResult::Resume {
                    session_id: self.filtered.get(self.list.selected).map(|e| e.id.clone()),
                });
            }
            (KeyCode::Esc, _) => {
                return Some(DialogResult::Resume { session_id: None });
            }
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                return Some(DialogResult::Resume { session_id: None });
            }
            (KeyCode::Char('w'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.workspace_only = !self.workspace_only;
                self.refilter();
            }
            (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                if !self.filtered.is_empty() {
                    self.list.page_up();
                }
            }
            (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                if !self.filtered.is_empty() {
                    self.list.page_down(self.filtered.len());
                }
            }
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
            {
                if self.query.is_empty() {
                    self.delete_selected();
                } else {
                    let len = self.query.len();
                    let target = crate::vim::word_backward_pos(
                        &self.query,
                        len,
                        crate::vim::CharClass::Word,
                    );
                    self.query.truncate(target);
                    self.refilter();
                }
            }
            (KeyCode::Backspace, _) => {
                if self.query.is_empty() {
                    self.delete_selected();
                } else {
                    self.query.pop();
                    self.refilter();
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.list.select_prev();
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.list.select_next(self.filtered.len());
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) if self.query.is_empty() => {
                self.pending_d = true;
                self.list.dirty = true;
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.query.push(c);
                self.refilter();
            }
            _ => {}
        }
        None
    }

    fn draw(&mut self, start_row: u16) {
        if !self.list.dirty {
            let freshest = self.filtered.iter().map(resume_ts).max().unwrap_or(0);
            let age_s = session::now_ms().saturating_sub(freshest) / 1000;
            let interval = if age_s < 60 {
                1
            } else if age_s < 3600 {
                30
            } else {
                60
            };
            if self.last_drawn.elapsed().as_secs() >= interval {
                self.list.dirty = true;
            }
        }
        if !self.list.dirty {
            return;
        }
        self.last_drawn = Instant::now();

        let Some((mut out, w, _)) = self.list.begin_draw(start_row, self.filtered.len().max(1))
        else {
            return;
        };

        let now_ms = session::now_ms();

        draw_bar(&mut out, w, None, None, theme::accent());
        crlf(&mut out);

        let _ = out.queue(SetAttribute(Attribute::Dim));
        if self.workspace_only {
            let _ = out.queue(Print(" Resume (workspace):"));
        } else {
            let _ = out.queue(Print(" Resume (all):"));
        }
        let _ = out.queue(SetAttribute(Attribute::Reset));
        let _ = out.queue(Print(" "));
        let _ = out.queue(Print(&self.query));
        crlf(&mut out);

        if self.filtered.is_empty() {
            let _ = out.queue(SetAttribute(Attribute::Dim));
            let _ = out.queue(Print("  No matches"));
            let _ = out.queue(SetAttribute(Attribute::Reset));
            crlf(&mut out);
        } else {
            let range = self.list.visible_range(self.filtered.len());
            for (i, entry) in self
                .filtered
                .iter()
                .enumerate()
                .take(range.end)
                .skip(range.start)
            {
                let title = resume_title(entry);
                let time_ago = session::time_ago(resume_ts(entry), now_ms);
                let time_len = time_ago.chars().count() + 1;
                let indent = 2 + entry.depth * 2;
                let indent_str = " ".repeat(indent);
                let max_label = w.saturating_sub(time_len + indent + 2);
                let truncated = truncate_str(&title, max_label);
                if i == self.list.selected {
                    let _ = out.queue(Print(&indent_str));
                    let _ = out.queue(SetForegroundColor(theme::accent()));
                    let _ = out.queue(Print(&truncated));
                    let _ = out.queue(ResetColor);
                } else {
                    let _ = out.queue(Print(&indent_str));
                    let _ = out.queue(Print(&truncated));
                }
                let _ = out.queue(Print(" "));
                let _ = out.queue(SetAttribute(Attribute::Dim));
                let _ = out.queue(Print(&time_ago));
                let _ = out.queue(SetAttribute(Attribute::Reset));
                crlf(&mut out);
            }
        }

        crlf(&mut out);
        let _ = out.queue(SetAttribute(Attribute::Dim));
        if self.workspace_only {
            let _ = out.queue(Print(
                " enter: select  dd/bs: delete  esc: cancel  ctrl+w: all sessions",
            ));
        } else {
            let _ = out.queue(Print(
                " enter: select  dd/bs: delete  esc: cancel  ctrl+w: this workspace",
            ));
        }
        let _ = out.queue(SetAttribute(Attribute::Reset));
        end_dialog_draw(&mut out);
    }
}

fn is_junk_title(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case("untitled")
        || t.starts_with('/')
        || t.starts_with('\x00')
}

fn resume_title(entry: &ResumeEntry) -> String {
    let raw = if !is_junk_title(&entry.title) {
        &entry.title
    } else if let Some(ref sub) = entry.subtitle {
        if !is_junk_title(sub) {
            sub
        } else {
            return "Untitled".into();
        }
    } else {
        return "Untitled".into();
    };
    raw.lines().next().unwrap_or("Untitled").trim().to_string()
}

fn resume_ts(entry: &ResumeEntry) -> u64 {
    if entry.updated_at_ms > 0 {
        entry.updated_at_ms
    } else {
        entry.created_at_ms
    }
}

fn filter_resume_entries(
    entries: &[ResumeEntry],
    query: &str,
    workspace_only: bool,
    current_cwd: &str,
) -> Vec<ResumeEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            if workspace_only {
                match e.cwd {
                    Some(ref cwd) => cwd == current_cwd,
                    None => false,
                }
            } else {
                true
            }
        })
        .filter(|e| {
            if q.is_empty() {
                return true;
            }
            let mut hay = resume_title(e);
            if let Some(ref subtitle) = e.subtitle {
                hay.push(' ');
                hay.push_str(subtitle);
            }
            crate::fuzzy::fuzzy_match(&hay, &q)
        })
        .cloned()
        .collect()
}
