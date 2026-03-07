use super::*;

impl App {
    // ── Commands ─────────────────────────────────────────────────────────

    pub(super) fn handle_command(&mut self, input: &str) -> CommandAction {
        match input {
            "/exit" | "/quit" | ":q" | ":qa" | ":wq" | ":wqa" => CommandAction::Quit,
            "/clear" | "/new" => {
                self.reset_session();
                CommandAction::CancelAndClear
            }
            "/compact" => CommandAction::Compact,
            "/resume" => {
                let entries = self.resume_entries();
                if entries.is_empty() {
                    self.screen.push(Block::Error {
                        message: "no saved sessions".into(),
                    });
                    self.screen.flush_blocks();
                    CommandAction::Continue
                } else {
                    let cwd = std::env::current_dir()
                        .ok()
                        .and_then(|p| p.to_str().map(String::from))
                        .unwrap_or_default();
                    CommandAction::OpenDialog(Box::new(render::ResumeDialog::new(
                        entries,
                        cwd,
                        Some(terminal::size().map(|(_, h)| h / 2).unwrap_or(12)),
                    )))
                }
            }
            "/vim" => {
                let enabled = !self.input.vim_enabled();
                self.input.set_vim_enabled(enabled);
                state::set_vim_enabled(enabled);
                CommandAction::Continue
            }
            "/export" => {
                self.export_to_clipboard();
                CommandAction::Continue
            }
            "/ps" => {
                if self.engine.processes.list().is_empty() {
                    self.screen.push(Block::Error {
                        message: "no background processes".into(),
                    });
                    self.screen.flush_blocks();
                    CommandAction::Continue
                } else {
                    CommandAction::OpenDialog(Box::new(render::PsDialog::new(
                        self.engine.processes.clone(),
                        None,
                    )))
                }
            }
            "/fork" => {
                self.fork_session();
                CommandAction::Continue
            }
            _ if input.starts_with('!') && !self.input.skip_shell_escape() => {
                self.run_shell_escape(&input[1..]);
                CommandAction::Continue
            }
            _ => CommandAction::Continue,
        }
    }

    /// Execute a command while the agent is running.
    /// Returns the `EventOutcome` to use, or `None` to queue as a message.
    pub(super) fn try_command_while_running(&mut self, input: &str) -> Option<EventOutcome> {
        // Not a command — will be queued as a user message.
        // Skip shell escape check for pasted content
        let is_from_paste = self.input.skip_shell_escape();
        if !input.starts_with('/')
            && (!input.starts_with('!') || is_from_paste)
            && !matches!(input, ":q" | ":qa" | ":wq" | ":wqa")
        {
            return None;
        }
        if input.starts_with('/') && !crate::completer::Completer::is_command(input) {
            return None;
        }

        // Access control: some commands are blocked while running.
        if let Err(reason) = is_allowed_while_running(input) {
            self.screen.push(Block::Error { message: reason });
            self.screen.flush_blocks();
            return Some(EventOutcome::Noop);
        }

        // Delegate to the unified handler.
        match self.handle_command(input) {
            CommandAction::Quit => Some(EventOutcome::Quit),
            CommandAction::CancelAndClear => Some(EventOutcome::CancelAndClear),
            CommandAction::OpenDialog(dlg) => Some(EventOutcome::OpenDialog(dlg)),
            CommandAction::Continue => Some(EventOutcome::Noop),
            CommandAction::Compact => unreachable!(), // blocked above
        }
    }

    pub(super) fn run_shell_escape(&mut self, raw: &str) {
        let cmd = raw.trim();
        if cmd.is_empty() {
            return;
        }
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map(|o| {
                let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr);
                if !stderr.is_empty() {
                    if !s.is_empty() {
                        s.push('\n');
                    }
                    s.push_str(&stderr);
                }
                s.truncate(s.trim_end().len());
                s
            })
            .unwrap_or_else(|e| format!("error: {}", e));
        self.screen.push(Block::Exec {
            command: cmd.to_string(),
            output,
        });
        self.screen.flush_blocks();
    }

    pub(super) fn toggle_mode(&mut self) {
        self.mode = self.mode.toggle();
        state::set_mode(self.mode);
        self.engine.send(UiCommand::SetMode { mode: self.mode });
        self.screen.mark_dirty();
    }

    pub(super) fn set_reasoning_effort(&mut self, effort: ReasoningEffort) {
        self.reasoning_effort = effort;
        self.screen.set_reasoning_effort(effort);
        state::set_reasoning_effort(effort);
        self.engine.send(UiCommand::SetReasoningEffort { effort });
    }

    pub(super) fn export_to_clipboard(&mut self) {
        let text = self.format_conversation_text();
        if text.is_empty() {
            self.screen.push(Block::Error {
                message: "nothing to export".into(),
            });
            self.screen.flush_blocks();
            return;
        }
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text)) {
            Ok(()) => {
                self.screen.push(Block::Text {
                    content: "conversation copied to clipboard".into(),
                });
                self.screen.flush_blocks();
            }
            Err(e) => {
                self.screen.push(Block::Error {
                    message: format!("clipboard error: {}", e),
                });
                self.screen.flush_blocks();
            }
        }
    }

    pub(super) fn format_conversation_text(&self) -> String {
        let mut out = String::new();
        for msg in &self.history {
            match msg.role {
                Role::System | Role::Tool => continue,
                Role::User => {
                    if let Some(c) = &msg.content {
                        out.push_str("User: ");
                        out.push_str(c.as_text());
                        out.push_str("\n\n");
                    }
                }
                Role::Assistant => {
                    if let Some(c) = &msg.content {
                        if !c.is_empty() {
                            out.push_str("Assistant: ");
                            out.push_str(c.as_text());
                            out.push_str("\n\n");
                        }
                    }
                    if let Some(calls) = &msg.tool_calls {
                        for tc in calls {
                            out.push_str(&format!("[Tool call: {}]\n\n", tc.function.name));
                        }
                    }
                }
            }
        }
        out.trim_end().to_string()
    }
}
