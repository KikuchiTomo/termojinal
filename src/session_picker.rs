//! Session Picker overlay — shown when creating a new tab/pane and
//! unattached daemon sessions are available for the current workspace.

use crate::workspace::DaemonSessionInfo;
use termojinal_ipc::keybinding::Action;

/// A single entry in the session picker.
#[derive(Clone)]
pub(crate) struct SessionPickerEntry {
    /// `None` for the "New Session" entry.
    pub(crate) session_id: Option<String>,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) shell: String,
    #[allow(dead_code)]
    pub(crate) cwd: String,
    pub(crate) pid: Option<i32>,
    #[allow(dead_code)]
    pub(crate) created_at: String,
}

/// State for the Session Picker overlay.
pub(crate) struct SessionPicker {
    pub(crate) visible: bool,
    pub(crate) input: String,
    pub(crate) entries: Vec<SessionPickerEntry>,
    pub(crate) filtered: Vec<usize>,
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
    /// The action that triggered the picker (NewTab / SplitRight / SplitDown).
    pub(crate) pending_action: Action,
}

impl SessionPicker {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            input: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            pending_action: Action::NewTab,
        }
    }

    /// Open the picker with unattached sessions for the given workspace.
    pub(crate) fn open(
        &mut self,
        unattached: &[DaemonSessionInfo],
        action: Action,
    ) {
        self.visible = true;
        self.input.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.pending_action = action;

        self.entries.clear();

        // First entry: create a new session.
        self.entries.push(SessionPickerEntry {
            session_id: None,
            label: "New Session".to_string(),
            detail: "Create a fresh terminal session".to_string(),
            shell: String::new(),
            cwd: String::new(),
            pid: None,
            created_at: String::new(),
        });

        // Add unattached sessions.
        for s in unattached {
            let shell_name = s
                .shell
                .rsplit('/')
                .next()
                .unwrap_or(&s.shell)
                .to_string();
            let cwd_short = shorten_home(&s.cwd);
            let detail = if let Some(pid) = s.pid {
                format!("{shell_name} \u{2022} {cwd_short} \u{2022} PID {pid}")
            } else {
                format!("{shell_name} \u{2022} {cwd_short}")
            };
            self.entries.push(SessionPickerEntry {
                session_id: Some(s.id.clone()),
                label: s.name.clone(),
                detail,
                shell: s.shell.clone(),
                cwd: s.cwd.clone(),
                pid: s.pid,
                created_at: s.created_at.clone(),
            });
        }

        self.update_filter();
    }

    pub(crate) fn dismiss(&mut self) {
        self.visible = false;
    }

    pub(crate) fn update_filter(&mut self) {
        let query = self.input.to_lowercase();
        if query.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.label.to_lowercase().contains(&query)
                        || e.detail.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = if self.selected + 1 >= self.filtered.len() {
                0
            } else {
                self.selected + 1
            };
        }
    }

    pub(crate) fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = if self.selected == 0 {
                self.filtered.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    pub(crate) fn ensure_visible(&mut self, max_visible: usize) {
        if max_visible == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_visible {
            self.scroll_offset = self.selected + 1 - max_visible;
        }
    }

    pub(crate) fn selected_entry(&self) -> Option<&SessionPickerEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
    }
}

/// Shorten a path by replacing the home directory with `~`.
fn shorten_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}
