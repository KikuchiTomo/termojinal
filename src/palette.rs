//! Command palette, file finder, Quick Launch, and update checker.

use std::sync::{Arc, Mutex};

use termojinal_ipc::keybinding::Action;
use termojinal_layout::PaneId;
use winit::keyboard::{Key, NamedKey};

use crate::workspace::WorkspaceInfo;
use crate::Workspace;

pub(crate) struct PaletteCommand {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) action: Action,
    pub(crate) kind: CommandKind,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CommandKind {
    Builtin,        // Built-in termojinal command
    Plugin,         // External command (unsigned/unverified)
    PluginVerified, // External command (signed & verified)
}

pub(crate) enum PaletteResult {
    /// Key was handled by palette.
    Consumed,
    /// User selected a command — execute the action.
    Execute(Action),
    /// User pressed Escape — dismiss palette.
    Dismiss,
    /// Key not handled by palette.
    Pass,
    /// Open a file in the editor.
    OpenInEditor(String),
    /// cd to a directory.
    CdToDirectory(String),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PaletteMode {
    /// File finder (default when palette opens).
    FileFinder,
    /// Command mode (activated by typing `>`).
    Command,
}

/// A file/directory entry in the file finder palette.
pub(crate) struct FileFinderEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) is_dir: bool,
}

/// State for the file finder within the command palette.
pub(crate) struct FileFinderState {
    /// Current search root directory.
    pub(crate) search_root: String,
    /// All entries at the current directory level.
    pub(crate) entries: Vec<FileFinderEntry>,
    /// Filtered entry indices (into `entries`).
    pub(crate) filtered: Vec<usize>,
    /// Currently selected index into `filtered`.
    pub(crate) selected: usize,
    /// Scroll offset for the results list.
    pub(crate) scroll_offset: usize,
}

impl FileFinderState {
    pub(crate) fn new() -> Self {
        Self {
            search_root: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Load entries from the given directory.
    pub(crate) fn load_entries(&mut self, root: &str) {
        self.search_root = root.to_string();
        self.entries.clear();
        self.filtered.clear();
        self.selected = 0;
        self.scroll_offset = 0;

        if let Ok(read_dir) = std::fs::read_dir(root) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let ffe = FileFinderEntry { name, path, is_dir };
                if is_dir {
                    dirs.push(ffe);
                } else {
                    files.push(ffe);
                }
            }
            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            self.entries.extend(dirs);
            self.entries.extend(files);
        }

        // Initially show all entries.
        self.filtered = (0..self.entries.len()).collect();
    }

    /// Filter entries by prefix match against the query.
    pub(crate) fn update_filter(&mut self, query: &str) {
        let q = query.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().starts_with(&q))
            .map(|(i, _)| i)
            .collect();
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

    pub(crate) fn selected_entry(&self) -> Option<&FileFinderEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
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
}

pub(crate) struct CommandPalette {
    pub(crate) visible: bool,
    pub(crate) mode: PaletteMode,
    pub(crate) input: String,
    pub(crate) preedit: String, // IME preedit text (displayed but not committed)
    pub(crate) commands: Vec<PaletteCommand>,
    pub(crate) filtered: Vec<usize>, // Indices into commands
    pub(crate) selected: usize,      // Index into filtered
    pub(crate) scroll_offset: usize, // First visible item index (for scrolling)
    pub(crate) file_finder: FileFinderState,
    /// Instant when error flash was triggered (orange border). None = no flash.
    pub(crate) error_flash: Option<std::time::Instant>,
}

impl CommandPalette {
    pub(crate) fn new() -> Self {
        let commands = vec![
            PaletteCommand {
                name: "Split Right".to_string(),
                description: "Split pane horizontally".to_string(),
                action: Action::SplitRight,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Split Down".to_string(),
                description: "Split pane vertically".to_string(),
                action: Action::SplitDown,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Close Pane".to_string(),
                description: "Close the focused pane".to_string(),
                action: Action::CloseTab,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "New Tab".to_string(),
                description: "Open a new tab".to_string(),
                action: Action::NewTab,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Zoom Pane".to_string(),
                description: "Toggle pane zoom".to_string(),
                action: Action::ZoomPane,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Next Pane".to_string(),
                description: "Focus next pane".to_string(),
                action: Action::NextPane,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Previous Pane".to_string(),
                description: "Focus previous pane".to_string(),
                action: Action::PrevPane,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "New Workspace".to_string(),
                description: "Create a new workspace".to_string(),
                action: Action::NewWorkspace,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Next Tab".to_string(),
                description: "Switch to next tab".to_string(),
                action: Action::NextTab,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Previous Tab".to_string(),
                description: "Switch to previous tab".to_string(),
                action: Action::PrevTab,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Toggle Sidebar".to_string(),
                description: "Show/hide sidebar".to_string(),
                action: Action::ToggleSidebar,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Toggle Directory Tree".to_string(),
                description: "Show/hide file tree in sidebar".to_string(),
                action: Action::ToggleDirectoryTree,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Copy".to_string(),
                description: "Copy selection to clipboard".to_string(),
                action: Action::Copy,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Paste".to_string(),
                description: "Paste from clipboard".to_string(),
                action: Action::Paste,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Search".to_string(),
                description: "Find in terminal".to_string(),
                action: Action::Search,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Allow Flow Panel".to_string(),
                description: "Toggle AI permission panel".to_string(),
                action: Action::AllowFlowPanel,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "About Termojinal".to_string(),
                description: "License, credits, and version info".to_string(),
                action: Action::About,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Quick Launch".to_string(),
                description: "Fuzzy search tabs, panes, and workspaces (Cmd+O)".to_string(),
                action: Action::QuickLaunch,
                kind: CommandKind::Builtin,
            },
            PaletteCommand {
                name: "Claudes Dashboard".to_string(),
                description: "Multi-agent Claude Code dashboard".to_string(),
                action: Action::ClaudesDashboard,
                kind: CommandKind::Builtin,
            },
        ];
        let filtered: Vec<usize> = (0..commands.len()).collect();
        Self {
            visible: false,
            mode: PaletteMode::FileFinder,
            input: String::new(),
            preedit: String::new(),
            commands,
            filtered,
            selected: 0,
            scroll_offset: 0,
            file_finder: FileFinderState::new(),
            error_flash: None,
        }
    }

    pub(crate) fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            // Reset state when opening — always start in file finder mode.
            self.mode = PaletteMode::FileFinder;
            self.input.clear();
            self.update_filter();
        }
    }

    /// Initialize file finder from a CWD path. Called when palette opens.
    pub(crate) fn init_file_finder(&mut self, cwd: &str) {
        self.file_finder.load_entries(cwd);
    }

    pub(crate) fn update_filter(&mut self) {
        let query = self.input.to_lowercase();
        self.filtered = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, cmd)| cmd.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected + 1 >= self.filtered.len() {
                self.selected = 0; // wrap to top
            } else {
                self.selected += 1;
            }
        }
    }

    pub(crate) fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1; // wrap to bottom
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Ensure selected item is visible within the scroll viewport.
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

    pub(crate) fn execute(&mut self) -> Option<Action> {
        if let Some(&idx) = self.filtered.get(self.selected) {
            Some(self.commands[idx].action.clone())
        } else {
            None
        }
    }

    pub(crate) fn handle_key(&mut self, event: &winit::event::KeyEvent) -> PaletteResult {
        match self.mode {
            PaletteMode::Command => self.handle_key_command(event),
            PaletteMode::FileFinder => self.handle_key_file_finder(event),
        }
    }

    /// Key handling for command mode (existing behavior).
    fn handle_key_command(&mut self, event: &winit::event::KeyEvent) -> PaletteResult {
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => PaletteResult::Dismiss,
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = self.execute() {
                    PaletteResult::Execute(action)
                } else {
                    PaletteResult::Consumed
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.select_prev();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::Backspace) => {
                if self.input.is_empty() {
                    // No text left in command mode -> switch back to file finder.
                    self.mode = PaletteMode::FileFinder;
                    self.file_finder.update_filter("");
                } else {
                    self.input.pop();
                    self.update_filter();
                }
                PaletteResult::Consumed
            }
            _ => {
                if let Some(ref text) = event.text {
                    if !text.is_empty() && !text.contains('\r') {
                        self.input.push_str(text);
                        self.update_filter();
                        return PaletteResult::Consumed;
                    }
                }
                PaletteResult::Pass
            }
        }
    }

    /// Key handling for file finder mode.
    fn handle_key_file_finder(&mut self, event: &winit::event::KeyEvent) -> PaletteResult {
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => PaletteResult::Dismiss,
            Key::Named(NamedKey::ArrowUp) => {
                self.file_finder.select_prev();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.file_finder.select_next();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::Tab) => {
                // Autocomplete: fill input with selected entry name.
                if let Some(entry) = self.file_finder.selected_entry() {
                    let name = entry.name.clone();
                    let is_dir = entry.is_dir;
                    // If the input has a path prefix (e.g., "src/"), keep it.
                    let prefix = if let Some(slash_pos) = self.input.rfind('/') {
                        &self.input[..=slash_pos]
                    } else {
                        ""
                    };
                    self.input = format!("{}{}{}", prefix, name, if is_dir { "/" } else { "" });
                    self.handle_file_finder_input_change();
                }
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(entry) = self.file_finder.selected_entry() {
                    let path = entry.path.clone();
                    let is_dir = entry.is_dir;
                    if is_dir {
                        // Navigate into the directory.
                        self.file_finder.load_entries(&path);
                        self.input.clear();
                        PaletteResult::Consumed
                    } else {
                        PaletteResult::OpenInEditor(path)
                    }
                } else {
                    PaletteResult::Consumed
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.input.is_empty() {
                    // Empty input + Backspace -> navigate to parent directory.
                    // If already at filesystem root, dismiss the palette.
                    let root = &self.file_finder.search_root;
                    let root_path = std::path::Path::new(root);
                    if let Some(parent) = root_path
                        .parent()
                        .filter(|p| *p != root_path)
                        .map(|p| p.to_string_lossy().to_string())
                    {
                        self.file_finder.load_entries(&parent);
                        PaletteResult::Consumed
                    } else {
                        PaletteResult::Dismiss
                    }
                } else {
                    self.input.pop();
                    self.handle_file_finder_input_change();
                    PaletteResult::Consumed
                }
            }
            _ => {
                if let Some(ref text) = event.text {
                    if !text.is_empty() && !text.contains('\r') {
                        // Check for `>` as first character -> switch to command mode.
                        if self.input.is_empty() && text.as_str() == ">" {
                            self.mode = PaletteMode::Command;
                            self.input.clear();
                            self.update_filter();
                            return PaletteResult::Consumed;
                        }
                        // `v` -> open in editor: use typed path if input is non-empty,
                        // otherwise use selected entry.
                        if text.as_str() == "v" {
                            let path = if !self.input.is_empty() {
                                self.resolve_input_path()
                            } else {
                                self.file_finder.selected_entry().map(|e| e.path.clone())
                            };
                            if let Some(p) = path {
                                self.error_flash = None;
                                return PaletteResult::OpenInEditor(p);
                            } else {
                                self.error_flash = Some(std::time::Instant::now());
                                return PaletteResult::Consumed;
                            }
                        }
                        // `e` -> cd to directory: use typed path if input is non-empty,
                        // otherwise use selected entry.
                        if text.as_str() == "e" {
                            let path = if !self.input.is_empty() {
                                // Resolve the typed path and extract a directory.
                                self.resolve_input_path().and_then(|p| {
                                    let pp = std::path::Path::new(&p);
                                    if pp.is_dir() {
                                        Some(p)
                                    } else {
                                        pp.parent().map(|d| d.to_string_lossy().to_string())
                                    }
                                })
                            } else {
                                self.file_finder
                                    .selected_entry()
                                    .map(|e| {
                                        if e.is_dir {
                                            e.path.clone()
                                        } else {
                                            std::path::Path::new(&e.path)
                                                .parent()
                                                .map(|p| p.to_string_lossy().to_string())
                                                .unwrap_or_default()
                                        }
                                    })
                                    .filter(|p| !p.is_empty())
                            };
                            if let Some(p) = path {
                                self.error_flash = None;
                                return PaletteResult::CdToDirectory(p);
                            } else {
                                self.error_flash = Some(std::time::Instant::now());
                                return PaletteResult::Consumed;
                            }
                        }
                        self.input.push_str(text);
                        self.handle_file_finder_input_change();
                        return PaletteResult::Consumed;
                    }
                }
                PaletteResult::Pass
            }
        }
    }

    /// Process input changes in file finder mode -- handle `/` path navigation
    /// and `..` parent directory, then filter entries.
    fn handle_file_finder_input_change(&mut self) {
        // `..` (with or without trailing `/`) -> navigate to parent directory.
        if self.input == ".." || self.input == "../" {
            if let Some(parent) = std::path::Path::new(&self.file_finder.search_root)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
            {
                self.file_finder.load_entries(&parent);
            }
            self.input.clear();
            return;
        }

        // Path with `/` -> split into directory navigation + filter.
        if self.input.contains('/') {
            if let Some(slash_pos) = self.input.rfind('/') {
                let dir_part = &self.input[..slash_pos];
                let filter_part = self.input[slash_pos + 1..].to_string();

                // Resolve the directory relative to search_root.
                let target_dir = if dir_part == ".." {
                    std::path::Path::new(&self.file_finder.search_root)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.file_finder.search_root.clone())
                } else if dir_part.starts_with('/') {
                    dir_part.to_string()
                } else {
                    format!("{}/{}", self.file_finder.search_root, dir_part)
                };

                // Only reload if directory changed.
                let canonical = std::fs::canonicalize(&target_dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or(target_dir.clone());
                let current_canonical = std::fs::canonicalize(&self.file_finder.search_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or(self.file_finder.search_root.clone());

                if canonical != current_canonical && std::path::Path::new(&canonical).is_dir() {
                    self.file_finder.load_entries(&canonical);
                    self.input = filter_part.clone();
                }
                self.file_finder.update_filter(&filter_part);
                return;
            }
        }

        // Single `.` -> don't filter yet (user might be typing `..`).
        if self.input == "." {
            return;
        }

        self.file_finder.update_filter(&self.input);
    }

    /// Resolve the current file finder input to an absolute path.
    /// Tries: (1) search_root + input, (2) input as absolute path.
    /// Returns `None` if nothing valid can be resolved.
    fn resolve_input_path(&self) -> Option<String> {
        if self.input.is_empty() {
            return None;
        }
        let candidate = if self.input.starts_with('/') {
            self.input.clone()
        } else {
            format!("{}/{}", self.file_finder.search_root, self.input)
        };
        let p = std::path::Path::new(&candidate);
        if p.exists() {
            std::fs::canonicalize(p)
                .map(|c| c.to_string_lossy().to_string())
                .ok()
        } else {
            // Try partial: parent dir exists and we have a name prefix that
            // matches a single entry.
            if let Some(entry) = self.file_finder.selected_entry() {
                let ep = std::path::Path::new(&entry.path);
                if ep.exists() {
                    return Some(entry.path.clone());
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Quick Launch -- fuzzy search overlay for tabs, panes, and workspaces
// ---------------------------------------------------------------------------

/// The kind of target a Quick Launch entry refers to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum QuickLaunchKind {
    Workspace,
    Tab,
    Pane,
}

/// A single entry shown in the Quick Launch overlay.
#[derive(Clone)]
pub(crate) struct QuickLaunchEntry {
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) kind: QuickLaunchKind,
    pub(crate) workspace_idx: usize,
    pub(crate) tab_idx: usize,
    pub(crate) pane_id: Option<PaneId>,
}

/// State for the Quick Launch overlay.
pub(crate) struct QuickLaunchState {
    pub(crate) visible: bool,
    pub(crate) input: String,
    pub(crate) entries: Vec<QuickLaunchEntry>,
    pub(crate) filtered: Vec<usize>,
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
}

impl QuickLaunchState {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            input: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        }
    }

    pub(crate) fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.input.clear();
            self.selected = 0;
            self.scroll_offset = 0;
        }
    }

    /// Rebuild the entry list from current workspaces.
    pub(crate) fn rebuild_entries(&mut self, workspaces: &[Workspace], workspace_infos: &[WorkspaceInfo]) {
        self.entries.clear();
        for (wi, ws) in workspaces.iter().enumerate() {
            let ws_name = workspace_infos
                .get(wi)
                .filter(|inf| !inf.name.is_empty())
                .map(|inf| inf.name.clone())
                .unwrap_or_else(|| ws.name.clone());
            // Workspace entry.
            self.entries.push(QuickLaunchEntry {
                label: ws_name.clone(),
                detail: format!("Workspace {} \u{2022} {} tab(s)", wi + 1, ws.tabs.len()),
                kind: QuickLaunchKind::Workspace,
                workspace_idx: wi,
                tab_idx: 0,
                pane_id: None,
            });
            // Tab entries.
            for (ti, tab) in ws.tabs.iter().enumerate() {
                let tab_label = if tab.display_title.is_empty() {
                    tab.name.clone()
                } else {
                    tab.display_title.clone()
                };
                self.entries.push(QuickLaunchEntry {
                    label: tab_label,
                    detail: format!(
                        "{} \u{203A} Tab {} \u{2022} {} pane(s)",
                        ws_name,
                        ti + 1,
                        tab.panes.len()
                    ),
                    kind: QuickLaunchKind::Tab,
                    workspace_idx: wi,
                    tab_idx: ti,
                    pane_id: None,
                });
                // Pane entries (only if multiple panes).
                if tab.panes.len() > 1 {
                    for (&pid, pane) in &tab.panes {
                        let pane_title = if !pane.terminal.osc.title.is_empty() {
                            pane.terminal.osc.title.clone()
                        } else if !pane.terminal.osc.cwd.is_empty() {
                            std::path::Path::new(&pane.terminal.osc.cwd)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("pane")
                                .to_string()
                        } else {
                            format!("Pane {pid}")
                        };
                        self.entries.push(QuickLaunchEntry {
                            label: pane_title,
                            detail: format!(
                                "{} \u{203A} Tab {} \u{203A} Pane {}",
                                ws_name,
                                ti + 1,
                                pid
                            ),
                            kind: QuickLaunchKind::Pane,
                            workspace_idx: wi,
                            tab_idx: ti,
                            pane_id: Some(pid),
                        });
                    }
                }
            }
        }
        self.update_filter();
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

    pub(crate) fn selected_entry(&self) -> Option<&QuickLaunchEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
    }
}

// ---------------------------------------------------------------------------
// Homebrew update checker
// ---------------------------------------------------------------------------

/// State for the background Homebrew update check.
pub(crate) struct UpdateChecker {
    /// If an update is available, contains the latest version string.
    pub(crate) available_version: Option<String>,
    /// Whether the check has been initiated.
    pub(crate) checked: bool,
}

impl UpdateChecker {
    pub(crate) fn new() -> Self {
        Self {
            available_version: None,
            checked: false,
        }
    }

    /// Spawn a background thread to check for updates via Homebrew.
    /// Non-blocking: the result is polled later via a shared Arc<Mutex>.
    pub(crate) fn start_check(result: Arc<Mutex<Option<String>>>) {
        std::thread::Builder::new()
            .name("brew-update-check".into())
            .spawn(move || {
                // Try `brew info --json=v2 termojinal` first (formula).
                let version = Self::check_brew_formula().or_else(Self::check_brew_cask);
                if let Some(latest) = version {
                    let current = env!("CARGO_PKG_VERSION");
                    // Simple string comparison: if latest != current, update available.
                    // Strip leading 'v' if present for comparison.
                    let latest_clean = latest.trim_start_matches('v');
                    if latest_clean != current && !latest_clean.is_empty() {
                        if let Ok(mut guard) = result.lock() {
                            *guard = Some(latest_clean.to_string());
                        }
                    }
                }
            })
            .ok();
    }

    fn check_brew_formula() -> Option<String> {
        let output = std::process::Command::new("brew")
            .args(["info", "--json=v2", "termojinal"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        json["formulae"].as_array()?.first()?["versions"]["stable"]
            .as_str()
            .map(|s| s.to_string())
    }

    fn check_brew_cask() -> Option<String> {
        let output = std::process::Command::new("brew")
            .args(["info", "--json=v2", "--cask", "termojinal-app"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        json["casks"].as_array()?.first()?["version"]
            .as_str()
            .map(|s| s.to_string())
    }
}
