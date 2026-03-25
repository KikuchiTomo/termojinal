//! Command execution UI state for the Command Palette.
//!
//! When a user selects an external command from the palette, a
//! [`CommandExecution`] is created to manage the child process and
//! drive the interactive UI (fuzzy lists, text inputs, confirmations, etc.)
//! through the stdio JSON protocol defined in [`termojinal_ipc::command_protocol`].

use std::collections::HashSet;

use termojinal_ipc::command_loader::LoadedCommand;
use termojinal_ipc::command_protocol::{CommandMessage, CommandResponse, FuzzyItem};
use termojinal_ipc::command_runner::{CommandRunner, RunnerStatus};

use winit::keyboard::{Key, NamedKey};

/// The current UI mode for an active command execution.
pub enum CommandUIState {
    /// Waiting for the first message from the command script.
    Loading,
    /// Showing a single-select fuzzy-filter list.
    Fuzzy { prompt: String },
    /// Showing a multi-select fuzzy-filter list.
    Multi { prompt: String },
    /// Showing a yes/no confirmation dialog.
    Confirm { message: String, default: bool },
    /// Showing a free-text input field.
    Text { label: String, placeholder: String },
    /// Showing a progress/information message (non-interactive).
    Info,
    /// The command has finished successfully.
    Done(Option<String>),
    /// The command has encountered an error.
    Error(String),
}

/// Result of handling a key event during command execution.
pub enum CommandKeyResult {
    /// The key was consumed by the command UI.
    Consumed,
    /// The user cancelled the command (Escape).
    Cancelled,
    /// The command has finished — dismiss the palette.
    Dismiss,
}

/// State for an active command execution in the Command Palette.
pub struct CommandExecution {
    pub runner: CommandRunner,
    pub command_name: String,
    /// Current UI state based on the latest CommandMessage.
    pub ui_state: CommandUIState,
    /// User input (for text/fuzzy/multi).
    pub input: String,
    /// Items from fuzzy/multi messages.
    pub items: Vec<FuzzyItem>,
    /// Filtered item indices (indices into `items`).
    pub filtered_items: Vec<usize>,
    /// Selected index in the filtered list.
    pub selected: usize,
    /// Multi-select toggled item indices (indices into `items`).
    pub selected_set: HashSet<usize>,
    /// Info/progress message text.
    pub info_message: String,
    /// IME preedit text (displayed but not committed).
    pub preedit: String,
}

impl CommandExecution {
    /// Start a new command execution from a loaded command definition.
    pub fn new(cmd: &LoadedCommand) -> Result<Self, std::io::Error> {
        let runner = CommandRunner::start(cmd)?;
        Ok(Self {
            runner,
            command_name: cmd.meta.name.clone(),
            ui_state: CommandUIState::Loading,
            input: String::new(),
            items: Vec::new(),
            filtered_items: Vec::new(),
            selected: 0,
            selected_set: HashSet::new(),
            info_message: String::new(),
            preedit: String::new(),
        })
    }

    /// Poll the command runner for new messages and update UI state.
    ///
    /// Returns `true` if a new message was received and state changed.
    pub fn poll(&mut self) -> bool {
        // Don't re-poll if we're already waiting for user input — the runner
        // will keep returning the same message and we'd reset selection state.
        if matches!(
            self.ui_state,
            CommandUIState::Fuzzy { .. }
                | CommandUIState::Multi { .. }
                | CommandUIState::Confirm { .. }
                | CommandUIState::Text { .. }
        ) {
            return false;
        }

        if let Some(msg) = self.runner.poll() {
            let msg = msg.clone();
            match msg {
                CommandMessage::Fuzzy { prompt, items, .. } => {
                    self.items = items;
                    self.input.clear();
                    self.selected = 0;
                    self.selected_set.clear();
                    self.filter_items();
                    self.ui_state = CommandUIState::Fuzzy { prompt };
                }
                CommandMessage::Multi { prompt, items } => {
                    self.items = items;
                    self.input.clear();
                    self.selected = 0;
                    self.selected_set.clear();
                    self.filter_items();
                    self.ui_state = CommandUIState::Multi { prompt };
                }
                CommandMessage::Confirm { message, default } => {
                    self.ui_state = CommandUIState::Confirm { message, default };
                }
                CommandMessage::Text {
                    label,
                    placeholder,
                    default,
                    ..
                } => {
                    self.input = default;
                    self.ui_state = CommandUIState::Text { label, placeholder };
                }
                CommandMessage::Info { message } => {
                    self.info_message = message;
                    self.ui_state = CommandUIState::Info;
                }
                CommandMessage::Done { notify } => {
                    self.ui_state = CommandUIState::Done(notify);
                }
                CommandMessage::Error { message } => {
                    self.ui_state = CommandUIState::Error(message);
                }
            }
            return true;
        }

        // Also check if runner status changed (e.g., EOF without Done).
        match self.runner.status() {
            RunnerStatus::Done(notify) => {
                if !matches!(self.ui_state, CommandUIState::Done(_)) {
                    self.ui_state = CommandUIState::Done(notify.clone());
                    return true;
                }
            }
            RunnerStatus::Error(msg) => {
                if !matches!(self.ui_state, CommandUIState::Error(_)) {
                    self.ui_state = CommandUIState::Error(msg.clone());
                    return true;
                }
            }
            _ => {}
        }

        false
    }

    /// Handle a key event based on the current UI state.
    pub fn handle_key(&mut self, event: &winit::event::KeyEvent) -> CommandKeyResult {
        // Escape always cancels.
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
            match &self.ui_state {
                CommandUIState::Done(_) | CommandUIState::Error(_) => {
                    return CommandKeyResult::Dismiss;
                }
                _ => {
                    self.runner.cancel();
                    return CommandKeyResult::Cancelled;
                }
            }
        }

        match &self.ui_state {
            CommandUIState::Fuzzy { .. } => self.handle_fuzzy_key(event),
            CommandUIState::Multi { .. } => self.handle_multi_key(event),
            CommandUIState::Confirm { .. } => self.handle_confirm_key(event),
            CommandUIState::Text { .. } => self.handle_text_key(event),
            CommandUIState::Done(_) | CommandUIState::Error(_) => {
                // Any key dismisses done/error state.
                CommandKeyResult::Dismiss
            }
            CommandUIState::Loading | CommandUIState::Info => {
                // No interaction during loading/info.
                CommandKeyResult::Consumed
            }
        }
    }

    /// Whether the command execution is finished (done or error).
    pub fn is_done(&self) -> bool {
        matches!(
            self.ui_state,
            CommandUIState::Done(_) | CommandUIState::Error(_)
        )
    }

    /// Update filtered_items based on the current input text.
    pub fn filter_items(&mut self) {
        let query = self.input.to_lowercase();
        if query.is_empty() {
            self.filtered_items = (0..self.items.len()).collect();
        } else {
            self.filtered_items = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let label = item.label.as_deref().unwrap_or(&item.value).to_lowercase();
                    let desc = item.description.as_deref().unwrap_or("").to_lowercase();
                    label.contains(&query) || desc.contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // Clamp selected index.
        if self.filtered_items.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.filtered_items.len() - 1);
        }
    }

    fn select_next_wrap(&mut self) {
        if !self.filtered_items.is_empty() {
            if self.selected + 1 >= self.filtered_items.len() {
                self.selected = 0;
            } else {
                self.selected += 1;
            }
        }
    }

    fn select_prev_wrap(&mut self) {
        if !self.filtered_items.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered_items.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    // ── Private key handlers ──────────────────────────────────────────

    fn handle_fuzzy_key(&mut self, event: &winit::event::KeyEvent) -> CommandKeyResult {
        // Arrow keys and Ctrl+N/P for navigation.
        match &event.logical_key {
            Key::Named(NamedKey::ArrowUp) => {
                self.select_prev_wrap();
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next_wrap();
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(&item_idx) = self.filtered_items.get(self.selected) {
                    let value = self.items[item_idx].value.clone();
                    let resp = CommandResponse::Selected { value };
                    self.send_response(resp);
                }
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::Backspace) => {
                self.input.pop();
                self.filter_items();
                return CommandKeyResult::Consumed;
            }
            _ => {}
        }

        // Text input for fuzzy filtering.
        if let Some(ref text) = event.text {
            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
            if !printable.is_empty() {
                self.input.push_str(&printable);
                self.filter_items();
                return CommandKeyResult::Consumed;
            }
        }
        CommandKeyResult::Consumed
    }

    fn handle_multi_key(&mut self, event: &winit::event::KeyEvent) -> CommandKeyResult {
        match &event.logical_key {
            Key::Named(NamedKey::ArrowUp) => {
                self.select_prev_wrap();
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next_wrap();
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::Enter) => {
                let values: Vec<String> = self
                    .selected_set
                    .iter()
                    .map(|&idx| self.items[idx].value.clone())
                    .collect();
                let resp = CommandResponse::MultiSelected { values };
                self.send_response(resp);
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::Space) => {
                if let Some(&item_idx) = self.filtered_items.get(self.selected) {
                    if self.selected_set.contains(&item_idx) {
                        self.selected_set.remove(&item_idx);
                    } else {
                        self.selected_set.insert(item_idx);
                    }
                }
                return CommandKeyResult::Consumed;
            }
            Key::Named(NamedKey::Backspace) => {
                self.input.pop();
                self.filter_items();
                return CommandKeyResult::Consumed;
            }
            _ => {}
        }

        if let Some(ref text) = event.text {
            let printable: String = text
                .chars()
                .filter(|c| !c.is_control() && *c != ' ')
                .collect();
            if !printable.is_empty() {
                self.input.push_str(&printable);
                self.filter_items();
                return CommandKeyResult::Consumed;
            }
        }
        CommandKeyResult::Consumed
    }

    fn handle_confirm_key(&mut self, event: &winit::event::KeyEvent) -> CommandKeyResult {
        let default_val = match &self.ui_state {
            CommandUIState::Confirm { default, .. } => *default,
            _ => false,
        };

        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                let resp = CommandResponse::Confirmed { yes: default_val };
                self.send_response(resp);
                CommandKeyResult::Consumed
            }
            _ => {
                if let Some(ref text) = event.text {
                    let ch = text.to_lowercase();
                    if ch == "y" {
                        let resp = CommandResponse::Confirmed { yes: true };
                        self.send_response(resp);
                        return CommandKeyResult::Consumed;
                    } else if ch == "n" {
                        let resp = CommandResponse::Confirmed { yes: false };
                        self.send_response(resp);
                        return CommandKeyResult::Consumed;
                    }
                }
                CommandKeyResult::Consumed
            }
        }
    }

    fn handle_text_key(&mut self, event: &winit::event::KeyEvent) -> CommandKeyResult {
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                let value = self.input.clone();
                let resp = CommandResponse::TextInput { value };
                self.send_response(resp);
                CommandKeyResult::Consumed
            }
            Key::Named(NamedKey::Backspace) => {
                self.input.pop();
                CommandKeyResult::Consumed
            }
            _ => {
                if let Some(ref text) = event.text {
                    if !text.is_empty() && !text.contains('\r') {
                        self.input.push_str(text);
                        return CommandKeyResult::Consumed;
                    }
                }
                CommandKeyResult::Consumed
            }
        }
    }

    /// Send a response to the command runner.
    fn send_response(&mut self, response: CommandResponse) {
        if let Err(e) = self.runner.respond(response) {
            // Broken pipe is expected when the command exits after sending Done/Error.
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                log::debug!(
                    "command '{}' pipe closed (expected after completion)",
                    self.command_name
                );
            } else {
                log::error!(
                    "failed to send response to command '{}': {}",
                    self.command_name,
                    e
                );
                self.ui_state = CommandUIState::Error(format!("communication error: {e}"));
                return;
            }
        }
        // After sending a response, go back to Loading so poll() can pick up
        // the next message from the command script.
        self.ui_state = CommandUIState::Loading;
    }
}
