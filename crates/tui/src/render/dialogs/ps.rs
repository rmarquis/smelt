use crate::render::{crlf, draw_bar};
use crate::{theme, utils::format_duration};
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::{Attribute, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::QueueableCommand;
use engine::tools::ProcessInfo;

use super::{end_dialog_draw, truncate_str, DialogResult, ListState};

pub struct PsDialog {
    registry: engine::tools::ProcessRegistry,
    procs: Vec<ProcessInfo>,
    list: ListState,
    killed: Vec<String>,
}

impl PsDialog {
    pub fn new(registry: engine::tools::ProcessRegistry, max_height: Option<u16>) -> Self {
        let procs = Self::fetch_procs(&registry, &[]);
        let list = ListState::new(procs.len().max(1), max_height, 4);
        Self {
            registry,
            procs,
            list,
            killed: Vec::new(),
        }
    }

    fn fetch_procs(
        registry: &engine::tools::ProcessRegistry,
        killed: &[String],
    ) -> Vec<ProcessInfo> {
        registry
            .list()
            .into_iter()
            .filter(|p| !killed.contains(&p.id))
            .collect()
    }
}

impl super::Dialog for PsDialog {
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
        match (code, mods) {
            (KeyCode::Esc, _) => return Some(DialogResult::PsClosed),
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                return Some(DialogResult::PsClosed)
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.list.select_prev();
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.list.select_next(self.procs.len());
            }
            (KeyCode::Backspace, _) => {
                if let Some(p) = self.procs.get(self.list.selected) {
                    self.killed.push(p.id.clone());
                    self.procs = Self::fetch_procs(&self.registry, &self.killed);
                    self.list.set_items(self.procs.len().max(1));
                }
            }
            _ => {}
        }
        None
    }

    fn draw(&mut self, start_row: u16) {
        let fresh = Self::fetch_procs(&self.registry, &self.killed);
        if fresh.len() != self.procs.len() {
            self.list.set_items(fresh.len().max(1));
        }
        self.procs = fresh;

        let Some((mut out, w, _)) = self.list.begin_draw(start_row, self.procs.len().max(1))
        else {
            return;
        };
        let now = std::time::Instant::now();

        draw_bar(&mut out, w, None, None, theme::accent());
        crlf(&mut out);

        let _ = out.queue(SetAttribute(Attribute::Dim));
        let _ = out.queue(Print(" Background Processes"));
        let _ = out.queue(SetAttribute(Attribute::Reset));
        crlf(&mut out);

        if self.procs.is_empty() {
            let _ = out.queue(SetAttribute(Attribute::Dim));
            let _ = out.queue(Print("  No processes"));
            let _ = out.queue(SetAttribute(Attribute::Reset));
            crlf(&mut out);
        } else {
            let range = self.list.visible_range(self.procs.len());
            for (i, proc) in self
                .procs
                .iter()
                .enumerate()
                .take(range.end)
                .skip(range.start)
            {
                let time = format_duration(now.duration_since(proc.started_at).as_secs());
                let meta = format!(" {time} {}", proc.id);
                let meta_len = meta.chars().count() + 1;
                let max_cmd = w.saturating_sub(meta_len + 4);
                let cmd_display = truncate_str(&proc.command, max_cmd);
                if i == self.list.selected {
                    let _ = out.queue(Print("  "));
                    let _ = out.queue(SetForegroundColor(theme::accent()));
                    let _ = out.queue(Print(&cmd_display));
                    let _ = out.queue(ResetColor);
                } else {
                    let _ = out.queue(Print("  "));
                    let _ = out.queue(Print(&cmd_display));
                }
                let _ = out.queue(Print(" "));
                let _ = out.queue(SetAttribute(Attribute::Dim));
                let _ = out.queue(Print(format!("{time} {}", proc.id)));
                let _ = out.queue(SetAttribute(Attribute::Reset));
                crlf(&mut out);
            }
        }

        crlf(&mut out);
        let _ = out.queue(SetAttribute(Attribute::Dim));
        let _ = out.queue(Print(" esc: close  backspace: kill selected"));
        let _ = out.queue(SetAttribute(Attribute::Reset));
        end_dialog_draw(&mut out);
    }
}
