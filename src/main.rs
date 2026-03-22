//! termojinal — GPU-accelerated multi-pane terminal emulator.

mod allow_flow;
mod appearance;
mod command_ui;
mod config;
mod notification;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

use config::{color_or, format_tab_title, load_config, parse_hex_color, resolve_theme, TermojinalConfig};

use serde_json::json;
use termojinal_ipc::app_protocol::{AppIpcRequest, AppIpcResponse};
use termojinal_ipc::command_loader::{self, LoadedCommand};
use termojinal_ipc::keybinding::{Action, KeybindingConfig};

use command_ui::{CommandExecution, CommandKeyResult, CommandUIState};
use termojinal_layout::{Direction, LayoutTree, PaneId, SplitDirection};
use termojinal_pty::{Pty, PtyConfig, PtySize};
use termojinal_render::{FontConfig, Renderer, RoundedRect, ThemePalette};
use termojinal_vt::{ClipboardEvent, MouseMode, Terminal};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

// ---------------------------------------------------------------------------
// Command Palette
// ---------------------------------------------------------------------------

struct PaletteCommand {
    name: String,
    description: String,
    action: Action,
    kind: CommandKind,
}

#[derive(Clone, Copy, PartialEq)]
enum CommandKind {
    Builtin,          // Built-in termojinal command
    Plugin,           // External command (unsigned/unverified)
    PluginVerified,   // External command (signed & verified)
}

enum PaletteResult {
    /// Key was handled by palette.
    Consumed,
    /// User selected a command — execute the action.
    Execute(Action),
    /// User pressed Escape — dismiss palette.
    Dismiss,
    /// Key not handled by palette.
    Pass,
}

struct CommandPalette {
    visible: bool,
    input: String,
    preedit: String,       // IME preedit text (displayed but not committed)
    commands: Vec<PaletteCommand>,
    filtered: Vec<usize>, // Indices into commands
    selected: usize,      // Index into filtered
    scroll_offset: usize, // First visible item index (for scrolling)
}

impl CommandPalette {
    fn new() -> Self {
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
        ];
        let filtered: Vec<usize> = (0..commands.len()).collect();
        Self {
            visible: false,
            input: String::new(),
            preedit: String::new(),
            commands,
            filtered,
            selected: 0,
            scroll_offset: 0,
        }
    }

    fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            // Reset state when opening.
            self.input.clear();
            self.update_filter();
        }
    }

    fn update_filter(&mut self) {
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

    fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected + 1 >= self.filtered.len() {
                self.selected = 0; // wrap to top
            } else {
                self.selected += 1;
            }
        }
    }

    fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1; // wrap to bottom
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Ensure selected item is visible within the scroll viewport.
    fn ensure_visible(&mut self, max_visible: usize) {
        if max_visible == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_visible {
            self.scroll_offset = self.selected + 1 - max_visible;
        }
    }

    fn execute(&mut self) -> Option<Action> {
        if let Some(&idx) = self.filtered.get(self.selected) {
            Some(self.commands[idx].action.clone())
        } else {
            None
        }
    }

    fn handle_key(&mut self, event: &winit::event::KeyEvent) -> PaletteResult {
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
                self.input.pop();
                self.update_filter();
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
}

// ---------------------------------------------------------------------------
// User events from PTY reader threads
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum UserEvent {
    PtyOutput(PaneId),
    PtyExited(PaneId),
    StatusUpdate,
    ToggleQuickTerminal,
    AppIpc {
        request: AppIpcRequest,
        response_tx: std_mpsc::Sender<AppIpcResponse>,
        /// Connection alive flag — set to `false` when the IPC client disconnects.
        /// `None` for non-PermissionRequest messages.
        connection_alive: Option<Arc<AtomicBool>>,
    },
    /// An IPC client disconnected — trigger cleanup of stale pending requests
    /// and agent state.
    IpcClientDisconnected,
}

// ---------------------------------------------------------------------------
// Selection state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridPos {
    col: usize,
    row: usize,
}

#[derive(Debug, Clone)]
struct Selection {
    start: GridPos,
    end: GridPos,
    active: bool, // mouse is still held
    /// The terminal scroll_offset at the time each endpoint was recorded.
    /// Stored so we can convert screen-relative rows to absolute positions
    /// that survive scrolling. Absolute row = screen_row - scroll_offset
    /// (invariant under scrolling). At render time we convert back with
    /// screen_row = abs_row + current_scroll_offset.
    scroll_offset_at_start: usize,
    scroll_offset_at_end: usize,
}

impl Selection {
    /// Absolute row for an endpoint: invariant under scrolling.
    /// Computed as screen_row - scroll_offset (can be negative).
    fn abs_row_start(&self) -> isize {
        self.start.row as isize - self.scroll_offset_at_start as isize
    }
    fn abs_row_end(&self) -> isize {
        self.end.row as isize - self.scroll_offset_at_end as isize
    }

    /// Normalize so start <= end in reading order, using absolute rows.
    /// Returns ((col, abs_row), (col, abs_row)).
    fn ordered_abs(&self) -> ((usize, isize), (usize, isize)) {
        let abs_s = self.abs_row_start();
        let abs_e = self.abs_row_end();
        if abs_s < abs_e || (abs_s == abs_e && self.start.col <= self.end.col) {
            ((self.start.col, abs_s), (self.end.col, abs_e))
        } else {
            ((self.end.col, abs_e), (self.start.col, abs_s))
        }
    }

    /// Whether start and end refer to the same cell (using absolute coords).
    fn is_empty(&self) -> bool {
        self.abs_row_start() == self.abs_row_end() && self.start.col == self.end.col
    }

    /// Extract selected text from the terminal.
    /// Uses absolute row coordinates to read the correct cells even when
    /// the selection spans scrollback content.
    fn text(&self, terminal: &termojinal_vt::Terminal) -> String {
        let ((sc, abs_sr), (ec, abs_er)) = self.ordered_abs();
        let grid = terminal.grid();
        let cols = grid.cols();
        let grid_rows = grid.rows() as isize;
        let mut result = String::new();

        for abs_row in abs_sr..=abs_er {
            // Determine which cell source to read from.
            // Absolute row 0 corresponds to grid row 0 at scroll_offset 0.
            // Negative absolute rows are in scrollback.
            // abs_row < 0 => scrollback. scrollback_row index = (-abs_row - 1).
            // abs_row >= 0 => grid row.
            let col_start = if abs_row == abs_sr { sc } else { 0 };
            let col_end = if abs_row == abs_er { ec + 1 } else { cols };

            if abs_row < 0 {
                // Scrollback content.
                let sb_idx = (-abs_row - 1) as usize;
                if let Some(cells) = terminal.scrollback_row(sb_idx) {
                    for col in col_start..col_end.min(cells.len()) {
                        let cell = &cells[col];
                        if cell.width > 0 && cell.c != '\0' {
                            result.push(cell.c);
                        }
                    }
                }
            } else if abs_row < grid_rows {
                // Grid content.
                let grow = abs_row as usize;
                for col in col_start..col_end.min(cols) {
                    let cell = grid.cell(col, grow);
                    if cell.width > 0 && cell.c != '\0' {
                        result.push(cell.c);
                    }
                }
            } else {
                // Beyond the grid; nothing to read.
                break;
            }

            if abs_row != abs_er {
                let trimmed = result.trim_end().len();
                result.truncate(trimmed);
                result.push('\n');
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Search state (Feature 5: Cmd+F)
// ---------------------------------------------------------------------------

struct SearchState {
    query: String,
    /// Matches: (row, col_start, col_end) - col_end is inclusive.
    matches: Vec<(usize, usize, usize)>,
    /// Index into matches for the current match.
    current: usize,
}

impl SearchState {
    fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        }
    }

    #[allow(dead_code)]
    fn search(&mut self, grid: &termojinal_vt::Grid) {
        self.matches.clear();
        self.current = 0;
        if self.query.is_empty() {
            return;
        }
        let query_lower = self.query.to_lowercase();
        let query_chars: Vec<char> = query_lower.chars().collect();
        let qlen = query_chars.len();
        if qlen == 0 {
            return;
        }
        for row in 0..grid.rows() {
            // Build a string for this row.
            let mut row_chars = Vec::with_capacity(grid.cols());
            for col in 0..grid.cols() {
                let cell = grid.cell(col, row);
                let c = if cell.c == '\0' { ' ' } else { cell.c };
                row_chars.push(c);
            }
            // Simple substring search (case-insensitive).
            let row_lower: Vec<char> = row_chars.iter().map(|c| {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.to_lowercase().chars().next().unwrap_or(*c)
            }).collect();
            for start_col in 0..row_lower.len() {
                if start_col + qlen > row_lower.len() {
                    break;
                }
                let mut found = true;
                for (qi, qc) in query_chars.iter().enumerate() {
                    if row_lower[start_col + qi] != *qc {
                        found = false;
                        break;
                    }
                }
                if found {
                    self.matches.push((row, start_col, start_col + qlen - 1));
                }
            }
        }
    }

    fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + 1) % self.matches.len();
        }
    }

    fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace info for rich sidebar (Feature 3)
// ---------------------------------------------------------------------------

struct WorkspaceInfo {
    name: String,
    git_branch: Option<String>,
    git_dirty: usize,
    git_untracked: usize,
    git_ahead: usize,
    git_behind: usize,
    #[allow(dead_code)]
    ports: Vec<u16>,
    last_updated: Instant,
    has_unread: bool,
}

impl WorkspaceInfo {
    fn new() -> Self {
        Self {
            name: String::new(),
            git_branch: None,
            git_dirty: 0,
            git_untracked: 0,
            git_ahead: 0,
            git_behind: 0,
            ports: Vec::new(),
            last_updated: Instant::now(),
            has_unread: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent session info for sidebar AI status display
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum AgentState {
    Running,
    Idle,
    WaitingForPermission,
    Inactive,
}

#[derive(Debug, Clone)]
struct AgentSessionInfo {
    active: bool,
    session_id: Option<String>,
    subagent_count: usize,
    summary: String,
    state: AgentState,
    last_updated: std::time::Instant,
}

impl Default for AgentSessionInfo {
    fn default() -> Self {
        Self {
            active: false,
            session_id: None,
            subagent_count: 0,
            summary: String::new(),
            state: AgentState::Inactive,
            last_updated: std::time::Instant::now(),
        }
    }
}

/// Rotating palette for workspace indicator dots (Arc browser inspired).
const WORKSPACE_COLORS: [[f32; 4]; 6] = [
    [0.29, 0.62, 1.0, 1.0],   // blue
    [0.55, 0.82, 0.33, 1.0],  // green
    [1.0, 0.58, 0.26, 1.0],   // orange
    [0.87, 0.44, 0.85, 1.0],  // purple
    [1.0, 0.42, 0.42, 1.0],   // red
    [0.36, 0.84, 0.77, 1.0],  // teal
];

/// Refresh workspace info by running git commands.
fn refresh_workspace_info(info: &mut WorkspaceInfo, cwd: &str) {
    if cwd.is_empty() {
        info.name = String::new();
        info.git_branch = None;
        info.git_dirty = 0;
        info.git_untracked = 0;
        info.git_ahead = 0;
        info.git_behind = 0;
        return;
    }
    // Name = basename of CWD.
    info.name = std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    // Git branch.
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
    {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.git_branch = if branch.is_empty() { None } else { Some(branch) };
        } else {
            info.git_branch = None;
        }
    }

    // Git dirty and untracked counts via porcelain status.
    info.git_dirty = 0;
    info.git_untracked = 0;
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().filter(|l| !l.is_empty()) {
                if line.starts_with("??") {
                    info.git_untracked += 1;
                } else {
                    info.git_dirty += 1;
                }
            }
        }
    }

    // Git ahead/behind counts.
    info.git_ahead = 0;
    info.git_behind = 0;
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", cwd, "rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let parts: Vec<&str> = text.split_whitespace().collect();
            if parts.len() == 2 {
                info.git_ahead = parts[0].parse().unwrap_or(0);
                info.git_behind = parts[1].parse().unwrap_or(0);
            }
        }
    }

    info.last_updated = Instant::now();
}

// ---------------------------------------------------------------------------
// Per-pane git info cache — refreshed every few seconds per CWD
// ---------------------------------------------------------------------------

/// Snapshot of status info collected by the background thread.
#[derive(Clone, Default)]
struct StatusSnapshot {
    cwd: String,
    git_branch: String,
    git_worktree: String,
    git_stash: usize,
    git_ahead: usize,
    git_behind: usize,
    git_dirty: usize,
    git_untracked: usize,
    git_remote: String,
    ssh_user: String,
    ssh_host: String,
}

/// Async status info collector. Runs heavy commands (lsof, git, ps, ssh -G)
/// on a background thread so the render loop is never blocked.
struct AsyncStatusCollector {
    /// Latest snapshot, read by the render thread.
    snapshot: Arc<Mutex<StatusSnapshot>>,
    /// PID + OSC CWD to request from the background thread.
    request: Arc<Mutex<(i32, String)>>,
    /// Notify the background thread to wake up early (e.g., after PTY output).
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl AsyncStatusCollector {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let snapshot = Arc::new(Mutex::new(StatusSnapshot::default()));
        let request = Arc::new(Mutex::new((0i32, String::new())));
        let notify = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

        let snap = Arc::clone(&snapshot);
        let req = Arc::clone(&request);
        let wake = Arc::clone(&notify);
        std::thread::Builder::new()
            .name("status-collector".into())
            .spawn(move || {
                loop {
                    // Wait up to 2 seconds, or wake immediately if nudged.
                    {
                        let (lock, cvar) = &*wake;
                        let mut nudged = lock.lock().unwrap();
                        if !*nudged {
                            let (mut g, _) = cvar.wait_timeout(nudged, std::time::Duration::from_secs(2)).unwrap();
                            *g = false;
                        } else {
                            *nudged = false;
                        }
                    }

                    let (pid, osc_cwd) = {
                        let r = req.lock().unwrap();
                        (r.0, r.1.clone())
                    };
                    if pid == 0 { continue; }

                    // Resolve CWD: prefer OSC 7, fallback to lsof.
                    let cwd = if !osc_cwd.is_empty() {
                        osc_cwd
                    } else {
                        get_child_cwd(pid).unwrap_or_default()
                    };

                    let mut s = StatusSnapshot::default();
                    s.cwd = cwd.clone();

                    // Always collect git info (branch may change even if CWD doesn't).
                    if !cwd.is_empty() {
                        Self::collect_git(&cwd, &mut s);
                    }

                    // Always detect SSH (connection may start/stop).
                    if let Some((user, host)) = detect_ssh_from_pid(pid) {
                        s.ssh_user = user.unwrap_or_default();
                        s.ssh_host = host;
                    }

                    // Update snapshot and trigger redraw.
                    let changed = {
                        let mut current = snap.lock().unwrap();
                        let changed = current.cwd != s.cwd
                            || current.git_branch != s.git_branch
                            || current.git_dirty != s.git_dirty
                            || current.ssh_host != s.ssh_host;
                        *current = s;
                        changed
                    };
                    if changed {
                        let _ = proxy.send_event(UserEvent::StatusUpdate);
                    }
                }
            })
            .expect("failed to spawn status collector thread");

        Self { snapshot, request, notify }
    }

    /// Update the request (called from render thread — non-blocking).
    fn update_request(&self, pid: i32, osc_cwd: &str) {
        if let Ok(mut r) = self.request.try_lock() {
            r.0 = pid;
            r.1 = osc_cwd.to_string();
        }
    }

    /// Wake the background thread immediately (e.g., after PTY output).
    fn nudge(&self) {
        let (lock, cvar) = &*self.notify;
        if let Ok(mut nudged) = lock.try_lock() {
            *nudged = true;
            cvar.notify_one();
        }
    }

    /// Get the latest snapshot (called from render thread — non-blocking).
    fn get(&self) -> StatusSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    fn collect_git(cwd: &str, s: &mut StatusSnapshot) {
        // Branch.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if out.status.success() {
                let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if b != "HEAD" { s.git_branch = b; }
            }
        }
        // Worktree.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "rev-parse", "--show-toplevel"])
            .output()
        {
            if out.status.success() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                s.git_worktree = std::path::Path::new(&p)
                    .file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            }
        }
        // Stash.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "stash", "list"])
            .output()
        {
            if out.status.success() {
                s.git_stash = String::from_utf8_lossy(&out.stdout)
                    .lines().filter(|l| !l.is_empty()).count();
            }
        }
        // Ahead/behind.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .output()
        {
            if out.status.success() {
                let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let parts: Vec<&str> = t.split_whitespace().collect();
                if parts.len() == 2 {
                    s.git_ahead = parts[0].parse().unwrap_or(0);
                    s.git_behind = parts[1].parse().unwrap_or(0);
                }
            }
        }
        // Dirty/untracked.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "status", "--porcelain"])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()) {
                    if line.starts_with("??") { s.git_untracked += 1; } else { s.git_dirty += 1; }
                }
            }
        }
        // Remote URL.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "remote", "get-url", "origin"])
            .output()
        {
            if out.status.success() {
                s.git_remote = String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
        }
    }
}

struct PaneGitCache {
    /// The CWD that was used to compute this cache.
    cwd: String,
    git_branch: String,
    git_worktree: String,
    git_stash: usize,
    git_ahead: usize,
    git_behind: usize,
    git_dirty: usize,
    git_untracked: usize,
    git_remote: String,
    ssh_user: String,
    ssh_host: String,
    last_updated: Instant,
}

impl PaneGitCache {
    fn new() -> Self {
        Self {
            cwd: String::new(),
            git_branch: String::new(),
            git_worktree: String::new(),
            git_stash: 0,
            git_ahead: 0,
            git_behind: 0,
            git_dirty: 0,
            git_untracked: 0,
            git_remote: String::new(),
            ssh_user: String::new(),
            ssh_host: String::new(),
            last_updated: Instant::now() - std::time::Duration::from_secs(999),
        }
    }

    /// Update from async snapshot.
    fn update_from_snapshot(&mut self, snap: &StatusSnapshot) {
        self.cwd = snap.cwd.clone();
        self.git_branch = snap.git_branch.clone();
        self.git_worktree = snap.git_worktree.clone();
        self.git_stash = snap.git_stash;
        self.git_ahead = snap.git_ahead;
        self.git_behind = snap.git_behind;
        self.git_dirty = snap.git_dirty;
        self.git_untracked = snap.git_untracked;
        self.git_remote = snap.git_remote.clone();
        self.ssh_user = snap.ssh_user.clone();
        self.ssh_host = snap.ssh_host.clone();
        self.last_updated = Instant::now();
    }
}

// ---------------------------------------------------------------------------
// Status bar context — resolved variable values, collected once per frame
// ---------------------------------------------------------------------------

struct StatusContext {
    user: String,
    host: String,
    cwd: String,
    cwd_short: String,
    git_branch: String,
    git_status: String,
    git_remote: String,
    git_worktree: String,
    git_stash: String,
    git_ahead: String,
    git_behind: String,
    git_dirty: String,
    git_untracked: String,
    ports: String,
    shell: String,
    pid: String,
    pane_size: String,
    font_size: String,
    workspace: String,
    workspace_index: String,
    tab: String,
    tab_index: String,
    time: String,
    date: String,
}

/// Cached values that rarely change (user, host, shell).
struct StatusCache {
    user: String,
    host: String,
    shell: String,
    /// Last second for which time/date were computed.
    last_time_secs: u64,
    cached_time: String,
    cached_date: String,
}

impl StatusCache {
    fn new() -> Self {
        let user = std::env::var("USER").unwrap_or_default();
        let host = gethostname::gethostname()
            .to_string_lossy()
            .to_string();
        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::Path::new(&s)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        Self {
            user,
            host,
            shell,
            last_time_secs: 0,
            cached_time: String::new(),
            cached_date: String::new(),
        }
    }

    /// Update time/date cache if the second has changed. Returns (time, date).
    fn time_date(&mut self) -> (&str, &str) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if secs != self.last_time_secs {
            self.last_time_secs = secs;
            // Compute HH:MM and YYYY-MM-DD from unix timestamp (UTC-local approximation
            // via the `time` crate is unavailable, so we shell out or use a simple approach).
            // For simplicity, use chrono-free manual UTC conversion; the status bar will show
            // local time if we use libc localtime.
            #[cfg(unix)]
            {
                let t = secs as i64;
                let mut tm: libc::tm = unsafe { std::mem::zeroed() };
                unsafe { libc::localtime_r(&t as *const i64, &mut tm) };
                self.cached_time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
                self.cached_date = format!(
                    "{:04}-{:02}-{:02}",
                    tm.tm_year + 1900,
                    tm.tm_mon + 1,
                    tm.tm_mday
                );
            }
            #[cfg(not(unix))]
            {
                // Fallback: leave empty on non-unix.
                let _ = secs;
            }
        }
        (&self.cached_time, &self.cached_date)
    }
}

/// Expand `{variable}` placeholders in a status segment content string.
fn expand_status_variables(template: &str, ctx: &StatusContext) -> String {
    template
        .replace("{user}", &ctx.user)
        .replace("{host}", &ctx.host)
        .replace("{cwd_short}", &ctx.cwd_short)
        .replace("{cwd}", &ctx.cwd)
        .replace("{git_branch}", &ctx.git_branch)
        .replace("{git_status}", &ctx.git_status)
        .replace("{git_remote}", &ctx.git_remote)
        .replace("{git_worktree}", &ctx.git_worktree)
        .replace("{git_stash}", &ctx.git_stash)
        .replace("{git_ahead}", &ctx.git_ahead)
        .replace("{git_behind}", &ctx.git_behind)
        .replace("{git_dirty}", &ctx.git_dirty)
        .replace("{git_untracked}", &ctx.git_untracked)
        .replace("{ports}", &ctx.ports)
        .replace("{shell}", &ctx.shell)
        .replace("{pid}", &ctx.pid)
        .replace("{pane_size}", &ctx.pane_size)
        .replace("{font_size}", &ctx.font_size)
        .replace("{workspace}", &ctx.workspace)
        .replace("{workspace_index}", &ctx.workspace_index)
        .replace("{tab}", &ctx.tab)
        .replace("{tab_index}", &ctx.tab_index)
        .replace("{time}", &ctx.time)
        .replace("{date}", &ctx.date)
}

/// Check if an expanded segment is "empty" — only whitespace after variable expansion.
fn segment_is_empty(expanded: &str) -> bool {
    expanded.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Drag-resize state for split pane separators
// ---------------------------------------------------------------------------

struct DragResize {
    /// Which direction the separator runs (Horizontal = vertical separator, etc.)
    direction: SplitDirection,
    /// The pane on the "first" side of the separator (used for resize calculation)
    pane_id: PaneId,
    /// Last mouse position along the drag axis
    last_pos: f64,
}

// ---------------------------------------------------------------------------
// Tab bar drag state (Feature 4: tab reordering)
// ---------------------------------------------------------------------------

struct TabDrag {
    /// Index of the tab being dragged.
    tab_idx: usize,
    /// Mouse x position when drag started.
    start_x: f64,
}

// ---------------------------------------------------------------------------
// Pane — holds per-pane terminal + PTY state
// ---------------------------------------------------------------------------

struct Pane {
    #[allow(dead_code)]
    id: PaneId,
    terminal: Terminal,
    vt_parser: vte::Parser,
    pty: Pty,
    /// The shell command used to spawn this pane's PTY (e.g. `/bin/zsh`).
    shell: String,
    selection: Option<Selection>,
    preedit: Option<String>,
}

// ---------------------------------------------------------------------------
// Tab — a single tab within a workspace, containing a layout tree + panes
// ---------------------------------------------------------------------------

struct Tab {
    layout: LayoutTree,
    panes: HashMap<PaneId, Pane>,
    #[allow(dead_code)]
    name: String,
    /// Computed display title (from OSC title, CWD, or fallback).
    display_title: String,
}

// ---------------------------------------------------------------------------
// Workspace — contains multiple tabs
// ---------------------------------------------------------------------------

struct Workspace {
    tabs: Vec<Tab>,
    active_tab: usize,
    name: String,
}

// ---------------------------------------------------------------------------
// Sidebar / tab bar constants
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Quick Terminal runtime state
// ---------------------------------------------------------------------------

/// Runtime state for the Quick Terminal feature.
#[allow(dead_code)]
struct QuickTerminalState {
    /// Quick Terminal mode is active (has been initialized).
    active: bool,
    /// Window is currently visible/shown.
    visible: bool,
    /// In-progress animation (None when idle).
    animation: Option<QuickTerminalAnimation>,
    /// Dedicated workspace index (if own_workspace = true).
    workspace_idx: Option<usize>,
}

#[allow(dead_code)]
struct QuickTerminalAnimation {
    start_time: std::time::Instant,
    duration: std::time::Duration,
    kind: AnimationKind,
}

#[allow(dead_code)]
enum AnimationKind {
    SlideDown { from_y: f64, to_y: f64 },
    SlideUp { from_y: f64, to_y: f64 },
    FadeIn,
    FadeOut,
}

impl QuickTerminalState {
    fn new() -> Self {
        Self {
            active: false,
            visible: false,
            animation: None,
            workspace_idx: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Quick Terminal show/hide/toggle logic
// ---------------------------------------------------------------------------

/// Toggle the Quick Terminal: show if hidden, hide if visible.
fn toggle_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;
    if !qtc.enabled {
        return;
    }

    if !state.quick_terminal.active {
        // First activation: mark as active.
        state.quick_terminal.active = true;
        show_quick_terminal(state);
    } else if state.quick_terminal.visible {
        hide_quick_terminal(state);
    } else {
        show_quick_terminal(state);
    }
}

/// Show the Quick Terminal window with optional slide-down animation.
fn show_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;

    // Get screen dimensions.
    let window_size = state.window.inner_size();
    let screen_h = window_size.height as f64 / state.scale_factor;

    // Target height based on config.
    let target_h = (screen_h * qtc.height_ratio as f64).round();

    // Make window visible and bring to front.
    state.window.set_visible(true);
    state.window.focus_window();

    // Start slide-down animation if configured.
    match qtc.animation.as_str() {
        "slide_down" => {
            state.quick_terminal.animation = Some(QuickTerminalAnimation {
                start_time: std::time::Instant::now(),
                duration: std::time::Duration::from_millis(qtc.animation_duration_ms as u64),
                kind: AnimationKind::SlideDown {
                    from_y: -target_h,
                    to_y: 0.0,
                },
            });
        }
        _ => {
            // No animation, just show immediately.
        }
    }

    state.quick_terminal.visible = true;

    // Switch to Quick Terminal workspace if it has a dedicated one.
    if let Some(ws_idx) = state.quick_terminal.workspace_idx {
        if ws_idx < state.workspaces.len() {
            state.active_workspace = ws_idx;
        }
    }

    // Bring app to front (macOS specific).
    #[cfg(target_os = "macos")]
    {
        activate_app();
    }

    state.window.request_redraw();
}

/// Hide the Quick Terminal window with optional slide-up animation.
fn hide_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;

    match qtc.animation.as_str() {
        "slide_down" => {
            let window_size = state.window.inner_size();
            let screen_h = window_size.height as f64 / state.scale_factor;
            let target_h = (screen_h * qtc.height_ratio as f64).round();
            state.quick_terminal.animation = Some(QuickTerminalAnimation {
                start_time: std::time::Instant::now(),
                duration: std::time::Duration::from_millis(qtc.animation_duration_ms as u64),
                kind: AnimationKind::SlideUp {
                    from_y: 0.0,
                    to_y: -target_h,
                },
            });
            // Don't hide the window yet — animation completion will hide it.
        }
        _ => {
            // No animation, hide immediately.
            state.window.set_visible(false);
            state.quick_terminal.visible = false;
        }
    }

    state.window.request_redraw();
}

/// Tick the Quick Terminal animation. Returns `true` if an animation is active
/// and we should keep requesting redraws.
fn tick_quick_terminal_animation(state: &mut AppState) -> bool {
    let anim = match state.quick_terminal.animation.as_ref() {
        Some(a) => a,
        None => return false,
    };

    let elapsed = anim.start_time.elapsed();
    let t = (elapsed.as_secs_f64() / anim.duration.as_secs_f64()).min(1.0);
    // Ease-out cubic: 1 - (1 - t)^3
    let eased = 1.0 - (1.0 - t).powi(3);

    let current_x = state
        .window
        .outer_position()
        .map(|p| p.x as f64)
        .unwrap_or(0.0);

    match &anim.kind {
        AnimationKind::SlideDown { from_y, to_y } | AnimationKind::SlideUp { from_y, to_y } => {
            let current_y = from_y + (to_y - from_y) * eased;
            let pos = winit::dpi::LogicalPosition::new(current_x, current_y);
            state.window.set_outer_position(pos);
        }
        AnimationKind::FadeIn | AnimationKind::FadeOut => {
            // Fade not yet implemented — would need NSWindow alpha.
        }
    }

    if t >= 1.0 {
        // Animation complete.
        let was_hiding = matches!(
            anim.kind,
            AnimationKind::SlideUp { .. } | AnimationKind::FadeOut
        );
        state.quick_terminal.animation = None;

        if was_hiding {
            state.window.set_visible(false);
            state.quick_terminal.visible = false;
        }
        false
    } else {
        // Keep animating.
        true
    }
}

/// Bring the application to the front on macOS.
#[cfg(target_os = "macos")]
fn activate_app() {
    use objc2::{class, msg_send, msg_send_id};
    use objc2::rc::Id;
    use objc2::runtime::NSObject;

    unsafe {
        let cls = class!(NSApplication);
        let app: Id<NSObject> = msg_send_id![cls, sharedApplication];
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
}

// ---------------------------------------------------------------------------
// AppState — the full multi-pane application state
// ---------------------------------------------------------------------------

struct AppState {
    window: Arc<Window>,
    renderer: Renderer,
    workspaces: Vec<Workspace>,
    active_workspace: usize,
    keybindings: KeybindingConfig,
    modifiers: ModifiersState,
    cursor_pos: (f64, f64),
    drag_resize: Option<DragResize>,
    next_pane_id: PaneId,
    sidebar_visible: bool,
    sidebar_width: f32,
    sidebar_drag: bool,
    command_palette: CommandPalette,
    font_size: f32,
    search: Option<SearchState>,
    workspace_infos: Vec<WorkspaceInfo>,
    /// Per-workspace AI agent session info for sidebar display.
    agent_infos: Vec<AgentSessionInfo>,
    /// Application start time for animation calculations.
    app_start_time: std::time::Instant,
    tab_drag: Option<TabDrag>,
    config: TermojinalConfig,
    status_cache: StatusCache,
    /// Per-pane git info cache (updated from async collector).
    pane_git_cache: PaneGitCache,
    /// Background thread that collects git/SSH/CWD info without blocking render.
    status_collector: AsyncStatusCollector,
    /// Current display scale factor (e.g. 2.0 for Retina, 1.0 for FHD).
    scale_factor: f64,
    /// Allow Flow UI state for AI agent permission management.
    allow_flow: allow_flow::AllowFlowUI,
    /// Deferred IPC responses for PermissionRequest hooks.
    /// Maps AllowFlow request ID → sender to reply when user decides.
    pending_ipc_responses: HashMap<u64, (std_mpsc::Sender<AppIpcResponse>, Arc<AtomicBool>)>,
    /// Maps Claude Code session IDs to workspace indices so that IPC requests
    /// are routed to the correct workspace even when the user has switched away.
    session_to_workspace: HashMap<String, usize>,
    /// Active command execution (None when showing the palette action list).
    command_execution: Option<CommandExecution>,
    /// Loaded external commands (cached at startup).
    external_commands: Vec<LoadedCommand>,
    /// Quick Terminal runtime state.
    quick_terminal: QuickTerminalState,
    /// Whether the "About Termojinal" overlay is visible.
    about_visible: bool,
    /// Scroll offset for the about overlay content.
    about_scroll: usize,
}

/// Update session_to_workspace mapping after a workspace at `removed_idx` is removed.
/// Removes entries pointing to the removed workspace and decrements indices above it.
fn cleanup_session_to_workspace(state: &mut AppState, removed_idx: usize) {
    state.session_to_workspace.retain(|_, idx| *idx != removed_idx);
    for idx in state.session_to_workspace.values_mut() {
        if *idx > removed_idx {
            *idx -= 1;
        }
    }
}

/// Helper to access the active workspace immutably.
fn active_ws(state: &AppState) -> &Workspace {
    &state.workspaces[state.active_workspace]
}

/// Helper to access the active workspace mutably.
fn active_ws_mut(state: &mut AppState) -> &mut Workspace {
    &mut state.workspaces[state.active_workspace]
}

/// Helper to access the active tab of the active workspace immutably.
fn active_tab(state: &AppState) -> &Tab {
    let ws = active_ws(state);
    &ws.tabs[ws.active_tab]
}

/// Helper to access the active tab of the active workspace mutably.
fn active_tab_mut(state: &mut AppState) -> &mut Tab {
    let ws = active_ws_mut(state);
    let idx = ws.active_tab;
    &mut ws.tabs[idx]
}

/// Whether the tab bar should be visible for the active workspace.
fn tab_bar_visible(state: &AppState) -> bool {
    let ws = active_ws(state);
    state.config.tab_bar.always_show || ws.tabs.len() > 1
}

/// Update the display title of a tab based on the focused pane's state.
/// `fallback_cwd` is the CWD from lsof (used when OSC 7 is unavailable).
fn update_tab_title(tab: &mut Tab, format: &str, tab_index: usize, fallback_cwd: &str) {
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get(&focused_id) {
        let title = &pane.terminal.osc.title;
        let cwd = if pane.terminal.osc.cwd.is_empty() {
            fallback_cwd
        } else {
            &pane.terminal.osc.cwd
        };
        let new_title = format_tab_title(format, title, cwd, tab_index);
        if new_title != tab.display_title {
            tab.display_title = new_title;
        }
    }
}

/// Abbreviate the home directory prefix in a path with `~`.
fn abbreviate_home(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if path.starts_with(&home) {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

/// Update the window title to reflect the focused pane's current state.
/// Format: "{title} — termojinal" or just "termojinal" if no info is available.
fn update_window_title(state: &AppState) {
    if state.workspaces.is_empty() {
        state.window.set_title("termojinal");
        return;
    }
    let ws = &state.workspaces[state.active_workspace];
    if ws.tabs.is_empty() {
        state.window.set_title("termojinal");
        return;
    }
    let tab = &ws.tabs[ws.active_tab];
    let focused_id = tab.layout.focused();

    let title = if let Some(pane) = tab.panes.get(&focused_id) {
        let osc_title = &pane.terminal.osc.title;
        let osc_cwd = &pane.terminal.osc.cwd;
        if !osc_title.is_empty() {
            format!("{osc_title} \u{2014} termojinal")
        } else if !osc_cwd.is_empty() {
            let display_cwd = abbreviate_home(osc_cwd);
            format!("{display_cwd} \u{2014} termojinal")
        } else {
            // Try fallback CWD from status collector cache.
            let fallback = &state.pane_git_cache.cwd;
            if !fallback.is_empty() {
                let display_cwd = abbreviate_home(fallback);
                format!("{display_cwd} \u{2014} termojinal")
            } else {
                "termojinal".to_string()
            }
        }
    } else {
        "termojinal".to_string()
    };

    state.window.set_title(&title);
}

/// Compute the content area that excludes the tab bar, sidebar, and status bar.
/// Effective status bar height (accounts for cell height minimum).
fn effective_status_bar_height(state: &AppState) -> f32 {
    if !state.config.status_bar.enabled {
        return 0.0;
    }
    let cell_h = state.renderer.cell_size().height;
    let bar_pad = 4.0_f32;
    state.config.status_bar.height.max(cell_h + bar_pad * 2.0)
}

/// Returns (content_x, content_y, content_w, content_h) in physical pixels.
fn content_area(state: &AppState, phys_w: f32, phys_h: f32) -> (f32, f32, f32, f32) {
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let tab_bar_h = if tab_bar_visible(state) { state.config.tab_bar.height } else { 0.0 };
    let status_bar_h = effective_status_bar_height(state);
    let content_x = sidebar_w;
    let content_y = tab_bar_h;
    let content_w = (phys_w - sidebar_w).max(1.0);
    let content_h = (phys_h - tab_bar_h - status_bar_h).max(1.0);
    (content_x, content_y, content_w, content_h)
}

/// Get pane rects for the active tab of the active workspace, offset by tab bar + sidebar.
fn active_pane_rects(state: &AppState) -> Vec<(PaneId, termojinal_layout::Rect)> {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let (cx, cy, cw, ch) = content_area(state, phys_w, phys_h);
    let tab = active_tab(state);
    let mut rects = tab.layout.panes(cw, ch);
    for (_, rect) in &mut rects {
        rect.x += cx;
        rect.y += cy;
    }
    rects
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
    pty_buffers: Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    config: Option<TermojinalConfig>,
    /// Whether `--quick-terminal` was passed on the command line.
    quick_terminal_mode: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, config: TermojinalConfig) -> Self {
        Self {
            state: None,
            proxy,
            pty_buffers: Arc::new(Mutex::new(HashMap::new())),
            config: Some(config),
            quick_terminal_mode: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Directory resolution helpers
// ---------------------------------------------------------------------------

/// Get the working directory of the focused pane in the active tab.
fn focused_pane_cwd(state: &AppState) -> Option<String> {
    let tab = active_tab(state);
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get(&focused_id) {
        let osc_cwd = &pane.terminal.osc.cwd;
        if !osc_cwd.is_empty() {
            return Some(osc_cwd.clone());
        }
    }
    // Fallback to git cache cwd
    let cwd = &state.pane_git_cache.cwd;
    if !cwd.is_empty() {
        Some(cwd.clone())
    } else {
        None
    }
}

/// Expand `~` at the start of a path to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return path.replacen('~', &home.to_string_lossy(), 1);
        }
    }
    path.to_string()
}

/// Validate and expand a configured directory path.
/// Returns `None` if the directory is empty or does not exist.
fn validate_dir(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let expanded = expand_tilde(path);
    if std::path::Path::new(&expanded).is_dir() {
        Some(expanded)
    } else {
        log::warn!("configured directory does not exist: {expanded}");
        None
    }
}

/// Determine the working directory for a new pane/tab based on config.
fn resolve_new_pane_cwd(state: &AppState) -> Option<String> {
    match state.config.pane.working_directory {
        config::PaneWorkingDirectory::Inherit => focused_pane_cwd(state),
        config::PaneWorkingDirectory::Home => {
            std::env::var("HOME").ok().or_else(|| {
                dirs::home_dir().map(|p| p.to_string_lossy().to_string())
            })
        }
        config::PaneWorkingDirectory::Fixed => {
            validate_dir(&state.config.pane.fixed_directory)
        }
    }
}

/// Determine the working directory for the initial pane on startup.
fn resolve_startup_cwd(config: &config::TermojinalConfig) -> Option<String> {
    match config.startup.mode {
        config::StartupMode::Default => None,
        config::StartupMode::Fixed => {
            validate_dir(&config.startup.directory)
        }
        config::StartupMode::Restore => {
            load_last_cwd()
        }
    }
}

/// State file path for persisting last CWD.
fn last_cwd_path() -> std::path::PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir.join("termojinal").join("last_cwd.txt")
}

/// Load the last saved CWD from disk.
fn load_last_cwd() -> Option<String> {
    let path = last_cwd_path();
    std::fs::read_to_string(&path).ok().and_then(|s| {
        let s = s.trim().to_string();
        if s.is_empty() || !std::path::Path::new(&s).is_dir() {
            None
        } else {
            Some(s)
        }
    })
}

/// Save the current CWD to disk for restore on next startup.
fn save_last_cwd(cwd: &str) {
    let path = last_cwd_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, cwd);
}

// ---------------------------------------------------------------------------
// Pane spawning helper
// ---------------------------------------------------------------------------

fn spawn_pane(
    id: PaneId,
    cols: u16,
    rows: u16,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    cwd: Option<String>,
) -> Result<Pane, termojinal_pty::PtyError> {
    let config = PtyConfig {
        size: PtySize { cols, rows },
        working_dir: cwd,
        ..PtyConfig::default()
    };
    let pty = Pty::spawn(&config)?;
    log::info!("pane {id}: shell={}, pid={}", config.shell, pty.pid());

    let terminal = Terminal::new(cols as usize, rows as usize);
    let vt_parser = vte::Parser::new();

    // Insert buffer for this pane.
    buffers.lock().unwrap().insert(id, VecDeque::new());

    // Spawn PTY reader thread.
    let master_fd = pty.master_fd();
    let proxy = proxy.clone();
    let buffers = buffers.clone();
    std::thread::Builder::new()
        .name(format!("pty-reader-{id}"))
        .spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match nix::unistd::read(master_fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut lock) = buffers.lock() {
                            if let Some(q) = lock.get_mut(&id) {
                                q.push_back(buf[..n].to_vec());
                            }
                        }
                        if proxy.send_event(UserEvent::PtyOutput(id)).is_err() {
                            break;
                        }
                    }
                    Err(nix::errno::Errno::EIO | nix::errno::Errno::EBADF) => break,
                    Err(e) => {
                        log::error!("pane {id}: PTY read error: {e}");
                        break;
                    }
                }
            }
            let _ = proxy.send_event(UserEvent::PtyExited(id));
        })
        .expect("failed to spawn pty-reader thread");

    // Register this pane with the session daemon (fire-and-forget).
    register_pane_with_daemon(
        id,
        pty.pid().as_raw(),
        &config.shell,
        config.working_dir.as_deref().unwrap_or("."),
        cols,
        rows,
    );

    Ok(Pane {
        id,
        terminal,
        vt_parser,
        pty,
        shell: config.shell.clone(),
        selection: None,
        preedit: None,
    })
}

/// Fire-and-forget: register a UI-spawned pane with the session daemon so
/// that `tm list` can report it.
fn register_pane_with_daemon(pane_id: PaneId, pid: i32, shell: &str, cwd: &str, cols: u16, rows: u16) {
    let shell = shell.to_string();
    let cwd = cwd.to_string();
    std::thread::Builder::new()
        .name(format!("daemon-register-{pane_id}"))
        .spawn(move || {
            use std::io::{BufRead, Write};
            use std::os::unix::net::UnixStream;

            let sock_path = daemon_socket_path();
            let Ok(mut stream) = UnixStream::connect(&sock_path) else {
                log::debug!("daemon not running, skipping pane registration for {pane_id}");
                return;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok();
            let req = json!({
                "type": "register_session",
                "pane_id": pane_id,
                "pid": pid,
                "shell": shell,
                "cwd": cwd,
                "cols": cols,
                "rows": rows,
            });
            let msg = format!("{}\n", req);
            if stream.write_all(msg.as_bytes()).is_err() {
                return;
            }
            // Drain the response.
            let mut line = String::new();
            let _ = std::io::BufReader::new(&stream).read_line(&mut line);
            log::debug!("daemon register response for pane {pane_id}: {}", line.trim());
        })
        .ok();
}

/// Fire-and-forget: unregister a pane from the session daemon.
fn unregister_pane_from_daemon(pane_id: PaneId) {
    std::thread::Builder::new()
        .name(format!("daemon-unregister-{pane_id}"))
        .spawn(move || {
            use std::io::{BufRead, Write};
            use std::os::unix::net::UnixStream;

            let sock_path = daemon_socket_path();
            let Ok(mut stream) = UnixStream::connect(&sock_path) else {
                return;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok();
            let req = json!({
                "type": "unregister_session",
                "pane_id": pane_id,
            });
            let msg = format!("{}\n", req);
            if stream.write_all(msg.as_bytes()).is_err() {
                return;
            }
            let mut line = String::new();
            let _ = std::io::BufReader::new(&stream).read_line(&mut line);
        })
        .ok();
}

/// Get the Unix socket path for the termojinald daemon.
/// Mirrors `termojinal_session::daemon::socket_path()`.
fn daemon_socket_path() -> String {
    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir
        .join("termojinal")
        .join("termojinald.sock")
        .to_string_lossy()
        .to_string()
}

// ---------------------------------------------------------------------------
// Keybinding string conversion
// ---------------------------------------------------------------------------

/// Convert a winit KeyEvent + modifiers into the keybinding string format
/// used by termojinal-ipc (e.g., "cmd+d", "ctrl+c", "cmd+shift+enter").
fn key_to_binding_string(event: &winit::event::KeyEvent, modifiers: ModifiersState) -> Option<String> {
    let mut parts = Vec::new();

    if modifiers.super_key() {
        parts.push("cmd");
    }
    if modifiers.control_key() {
        parts.push("ctrl");
    }
    if modifiers.alt_key() {
        parts.push("alt");
    }
    if modifiers.shift_key() {
        parts.push("shift");
    }

    let key_name = match &event.logical_key {
        Key::Character(c) => {
            // Return lowercase character as the key name.
            let s = c.to_lowercase();
            Some(s)
        }
        Key::Named(named) => {
            let name = match named {
                NamedKey::Enter => "enter",
                NamedKey::Tab => "tab",
                NamedKey::Space => "space",
                NamedKey::Escape => "escape",
                NamedKey::Backspace => "backspace",
                NamedKey::Delete => "delete",
                NamedKey::ArrowUp => "up",
                NamedKey::ArrowDown => "down",
                NamedKey::ArrowLeft => "left",
                NamedKey::ArrowRight => "right",
                NamedKey::Home => "home",
                NamedKey::End => "end",
                NamedKey::PageUp => "pageup",
                NamedKey::PageDown => "pagedown",
                NamedKey::Insert => "insert",
                NamedKey::F1 => "f1",
                NamedKey::F2 => "f2",
                NamedKey::F3 => "f3",
                NamedKey::F4 => "f4",
                NamedKey::F5 => "f5",
                NamedKey::F6 => "f6",
                NamedKey::F7 => "f7",
                NamedKey::F8 => "f8",
                NamedKey::F9 => "f9",
                NamedKey::F10 => "f10",
                NamedKey::F11 => "f11",
                NamedKey::F12 => "f12",
                _ => return None, // Modifier-only key or unknown
            };
            Some(name.to_string())
        }
        _ => None,
    };

    let key_name = key_name?;
    if parts.is_empty() && key_name.len() == 1 {
        // Single character with no modifier — not a binding, let key_to_bytes handle it.
        return None;
    }
    parts.push(&key_name);
    // Only produce a binding string if there is at least one modifier.
    if !modifiers.is_empty() {
        Some(parts.join("+"))
    } else {
        // Named keys without modifiers (e.g., "enter", "escape") — not bindings.
        None
    }
}

// ---------------------------------------------------------------------------
// Keyboard → PTY byte translation
// ---------------------------------------------------------------------------

fn key_to_bytes(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> Option<Vec<u8>> {
    // Ctrl+key → control codes.
    if modifiers.control_key() {
        if let Key::Character(ref c) = event.logical_key {
            let ch = c.chars().next()?;
            match ch.to_ascii_lowercase() {
                'a'..='z' => return Some(vec![ch.to_ascii_lowercase() as u8 - b'a' + 1]),
                '[' => return Some(vec![0x1B]),
                '\\' => return Some(vec![0x1C]),
                ']' => return Some(vec![0x1D]),
                '^' | '6' => return Some(vec![0x1E]),
                '_' | '7' => return Some(vec![0x1F]),
                '@' | '2' | ' ' => return Some(vec![0x00]),
                _ => {}
            }
        }
    }

    // Named keys → escape sequences.
    if let Key::Named(ref named) = event.logical_key {
        match named {
            NamedKey::Enter => {
                // Shift+Enter sends LF (\n) — used by Claude Code for newline input.
                if modifiers.shift_key() {
                    return Some(b"\n".to_vec());
                }
                return Some(b"\r".to_vec());
            }
            NamedKey::Backspace => return Some(vec![0x7F]),
            NamedKey::Tab => return Some(b"\t".to_vec()),
            NamedKey::Space => return Some(b" ".to_vec()),
            NamedKey::Escape => return Some(vec![0x1B]),
            NamedKey::ArrowUp => return Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => return Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => return Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => return Some(b"\x1b[D".to_vec()),
            NamedKey::Home => return Some(b"\x1b[H".to_vec()),
            NamedKey::End => return Some(b"\x1b[F".to_vec()),
            NamedKey::PageUp => return Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => return Some(b"\x1b[6~".to_vec()),
            NamedKey::Delete => return Some(b"\x1b[3~".to_vec()),
            NamedKey::Insert => return Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => return Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => return Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => return Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => return Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => return Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => return Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => return Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => return Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => return Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => return Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => return Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => return Some(b"\x1b[24~".to_vec()),
            _ => {}
        }
    }

    // Regular text input.
    if let Some(ref text) = event.text {
        if !text.is_empty() {
            return Some(text.as_bytes().to_vec());
        }
    }

    // Fallback: character key.
    if let Key::Character(ref c) = event.logical_key {
        return Some(c.as_bytes().to_vec());
    }

    None
}

// ---------------------------------------------------------------------------
// Mouse → escape sequence encoding (SGR format)
// ---------------------------------------------------------------------------

/// Encode a mouse event as an SGR escape sequence to forward to the PTY.
/// `btn`: 0=left, 1=middle, 2=right; add 32 for motion, 64 for scroll
/// `col`, `row`: 1-based grid coordinates
/// `pressed`: true for press (M), false for release (m)
fn encode_mouse_sgr(btn: u8, col: usize, row: usize, pressed: bool) -> Vec<u8> {
    let suffix = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{btn};{};{}{suffix}", col + 1, row + 1).into_bytes()
}

// ---------------------------------------------------------------------------
// winit ApplicationHandler
// ---------------------------------------------------------------------------

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let opacity = self.config.as_ref().map_or(1.0, |c| c.window.opacity);
        let transparent = opacity < 1.0;
        log::info!("config present={}, opacity={opacity}, transparent={transparent}",
                   self.config.is_some());
        if let Some(c) = &self.config {
            log::info!("config font.size={}, window={}x{}", c.font.size, c.window.width, c.window.height);
        }

        let attrs = WindowAttributes::default()
            .with_title("termojinal")
            .with_inner_size(LogicalSize::new(
                self.config.as_ref().map_or(960, |c| c.window.width),
                self.config.as_ref().map_or(640, |c| c.window.height),
            ))
            .with_transparent(transparent);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let cfg = self.config.as_ref().map(|c| c.clone()).unwrap_or_default();
        let initial_scale_factor = window.scale_factor();
        let font_config = FontConfig {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
            line_height: cfg.font.line_height,
        };
        let mut renderer = match pollster::block_on(Renderer::new(window.clone(), &font_config)) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        // Resolve theme (with auto-switch if enabled).
        let current_appearance = appearance::detect_macos_appearance();
        let effective_theme = resolve_theme(&cfg, current_appearance);

        // Build theme palette from effective theme and apply to renderer.
        let palette = ThemePalette::from_theme_colors(
            &effective_theme.background,
            &effective_theme.foreground,
            &effective_theme.black,
            &effective_theme.red,
            &effective_theme.green,
            &effective_theme.yellow,
            &effective_theme.blue,
            &effective_theme.magenta,
            &effective_theme.cyan,
            &effective_theme.white,
            &effective_theme.bright_black,
            &effective_theme.bright_red,
            &effective_theme.bright_green,
            &effective_theme.bright_yellow,
            &effective_theme.bright_blue,
            &effective_theme.bright_magenta,
            &effective_theme.bright_cyan,
            &effective_theme.bright_white,
        );
        renderer.set_theme(palette);

        // Set renderer fields from config.
        renderer.default_bg = color_or(&effective_theme.background, [0.067, 0.067, 0.09, 1.0]);
        renderer.bg_opacity = opacity;
        renderer.preedit_bg = color_or(&effective_theme.preedit_bg, [0.15, 0.15, 0.20, 1.0]);
        renderer.scrollbar_thumb_opacity = cfg.pane.scrollbar_thumb_opacity;
        renderer.scrollbar_track_opacity = cfg.pane.scrollbar_track_opacity;

        let size = window.inner_size();
        let phys_w = size.width as f32;
        let phys_h = size.height as f32;
        // Compute the initial content area matching what resize_all_panes will
        // use, so the PTY is spawned with the exact grid size the shell will
        // see — preventing a SIGWINCH resize (which causes an extra newline
        // and the zsh `%` marker on startup).
        let initial_sidebar_w = cfg.sidebar.width;
        let initial_tab_bar_h = if cfg.tab_bar.always_show { cfg.tab_bar.height } else { 0.0 };
        let cell_h = renderer.cell_size().height;
        let bar_pad = 4.0_f32;
        let initial_status_bar_h = if cfg.status_bar.enabled {
            cfg.status_bar.height.max(cell_h + bar_pad * 2.0)
        } else {
            0.0
        };
        let content_w = (phys_w - initial_sidebar_w).max(1.0);
        let content_h = (phys_h - initial_tab_bar_h - initial_status_bar_h).max(1.0);
        let (cols, rows) = renderer.grid_size_raw(content_w as u32, content_h as u32);
        log::info!("window {}x{} -> grid {cols}x{rows}", size.width, size.height);

        // Create the initial pane (id 0) in the first workspace, first tab.
        let initial_id: PaneId = 0;
        let layout = LayoutTree::new(initial_id);

        let startup_cwd = resolve_startup_cwd(&cfg);
        let pane = match spawn_pane(initial_id, cols, rows, &self.proxy, &self.pty_buffers, startup_cwd) {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to spawn initial pane: {e}");
                event_loop.exit();
                return;
            }
        };

        let mut panes = HashMap::new();
        panes.insert(initial_id, pane);

        let initial_tab = Tab {
            layout,
            panes,
            name: "Tab 1".to_string(),
            display_title: format_tab_title(&cfg.tab_bar.format, "", "", 1),
        };

        let initial_workspace = Workspace {
            tabs: vec![initial_tab],
            active_tab: 0,
            name: "Workspace 1".to_string(),
        };

        let keybindings = KeybindingConfig::load();

        // Load external commands from ~/.config/termojinal/commands/.
        let external_commands = command_loader::load_commands();
        log::info!("loaded {} external commands", external_commands.len());

        // Build the command palette with external commands appended.
        let mut palette = CommandPalette::new();
        for cmd in &external_commands {
            let kind = if cmd.verify_result.is_verified() {
                CommandKind::PluginVerified
            } else {
                CommandKind::Plugin
            };
            palette.commands.push(PaletteCommand {
                name: cmd.meta.name.clone(),
                description: cmd.meta.description.clone(),
                action: Action::Command(cmd.meta.name.clone()),
                kind,
            });
        }
        palette.update_filter();

        self.state = Some(AppState {
            window,
            renderer,
            workspaces: vec![initial_workspace],
            active_workspace: 0,
            keybindings,
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
            drag_resize: None,
            next_pane_id: 1, // 0 is already used
            sidebar_visible: true,
            sidebar_width: cfg.sidebar.width,
            sidebar_drag: false,
            command_palette: palette,
            font_size: cfg.font.size,
            search: None,
            workspace_infos: vec![WorkspaceInfo::new()],
            agent_infos: vec![AgentSessionInfo::default()],
            app_start_time: std::time::Instant::now(),
            tab_drag: None,
            config: cfg.clone(),
            status_cache: StatusCache::new(),
            pane_git_cache: PaneGitCache::new(),
            status_collector: AsyncStatusCollector::new(self.proxy.clone()),
            scale_factor: initial_scale_factor,
            allow_flow: allow_flow::AllowFlowUI::new(cfg.allow_flow.clone()),
            pending_ipc_responses: HashMap::new(),
            session_to_workspace: HashMap::new(),
            command_execution: None,
            external_commands,
            quick_terminal: QuickTerminalState::new(),
            about_visible: false,
            about_scroll: 0,
        });

        // Activate Quick Terminal mode if --quick-terminal was passed.
        if self.quick_terminal_mode {
            let state = self.state.as_mut().unwrap();
            state.quick_terminal.active = true;
            log::info!("quick terminal state activated");
        }

        // Detect ProMotion display and try low-latency present mode.
        {
            let state = self.state.as_mut().unwrap();
            let monitor = state.window.current_monitor();
            if let Some(m) = monitor {
                let refresh = m.refresh_rate_millihertz().unwrap_or(60000);
                if refresh > 60000 {
                    log::info!("high refresh rate display detected: {}Hz", refresh / 1000);
                    // Try Mailbox first (low latency), fall back to Immediate.
                    // If neither is supported, keep the default Fifo.
                    if state.renderer.try_set_present_mode(wgpu::PresentMode::Mailbox)
                        || state.renderer.try_set_present_mode(wgpu::PresentMode::Immediate)
                    {
                        log::info!("using low-latency present mode");
                    }
                }
            }
        }

        // On macOS, set window background to clear for transparency to work.
        #[cfg(target_os = "macos")]
        if transparent {
            set_macos_window_transparent(&self.state.as_ref().unwrap().window);
        }

        // Set Dock icon now that NSApplication is fully initialized.
        set_dock_icon();

        // Initialize notification system (sets bundle ID for app icon in notifications).
        notification::init();

        // Request notification permission if not already granted.
        #[cfg(target_os = "macos")]
        notification::request_notification_permission_if_needed();

        // Enable IME after window is fully created and request initial redraw.
        let state = self.state.as_ref().unwrap();
        state.window.set_ime_allowed(true);
        // Set initial IME cursor area so macOS will start sending IME events.
        state.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(0.0, 0.0),
            winit::dpi::PhysicalSize::new(
                state.renderer.cell_size().width as f64,
                state.renderer.cell_size().height as f64,
            ),
        );
        // Seed the background status collector with the initial pane's PID.
        {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                state.status_collector.update_request(
                    pane.pty.pid().as_raw(),
                    &pane.terminal.osc.cwd,
                );
            }
        }
        state.window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                // Save last CWD for restore_last_directory feature
                if state.config.startup.mode == config::StartupMode::Restore {
                    if let Some(cwd) = focused_pane_cwd(state) {
                        save_last_cwd(&cwd);
                    }
                }
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                state.renderer.resize(size.width, size.height);
                resize_all_panes(state);
                state.window.request_redraw();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let old_scale = state.scale_factor;
                state.scale_factor = scale_factor;
                state.renderer.scale_factor = scale_factor as f32;
                log::info!("scale factor changed: {old_scale} -> {scale_factor}");

                // Rebuild font atlas at new DPI. Logical font size unchanged,
                // but physical rasterization pixels = logical * scale_factor.
                if let Err(e) = state.renderer.set_font_size(state.font_size) {
                    log::error!("failed to rebuild font atlas on scale change: {e}");
                }
                resize_all_panes(state);
                state.window.request_redraw();
            }

            WindowEvent::Focused(focused) => {
                if focused {
                    // Re-enable IME when window gains focus.
                    state.window.set_ime_allowed(true);
                }
                // Send focus in/out events to the focused pane if it has focus_events mode.
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                    if pane.terminal.modes.focus_events {
                        let seq = if focused { b"\x1b[I" } else { b"\x1b[O" };
                        let _ = pane.pty.write(seq);
                    }
                }

                // Quick Terminal: hide on focus loss if configured.
                if !focused
                    && state.quick_terminal.active
                    && state.quick_terminal.visible
                    && state.config.quick_terminal.hide_on_focus_loss
                {
                    hide_quick_terminal(state);
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                // About overlay intercepts all keyboard input when visible.
                if state.about_visible {
                    let scroll_down = matches!(&event.logical_key, Key::Named(NamedKey::ArrowDown))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "j");
                    let scroll_up = matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "k");
                    if scroll_down {
                        state.about_scroll = state.about_scroll.saturating_add(3);
                        state.window.request_redraw();
                    } else if scroll_up {
                        state.about_scroll = state.about_scroll.saturating_sub(3);
                        state.window.request_redraw();
                    } else {
                        // Any other key dismisses the about screen.
                        state.about_visible = false;
                        state.window.request_redraw();
                    }
                    return;
                }

                // Allow Flow: intercept physical keys even during IME composition.
                // This lets y/n/a work for fast-allow regardless of IME state.
                if state.allow_flow.first_workspace_with_pending().is_some() {
                    use winit::keyboard::KeyCode;
                    let physical = match event.physical_key {
                        winit::keyboard::PhysicalKey::Code(code) => Some(code),
                        _ => None,
                    };
                    let shift = state.modifiers.shift_key();
                    let no_mods = !state.modifiers.control_key()
                        && !state.modifiers.super_key()
                        && !state.modifiers.alt_key();
                    if no_mods {
                        if let Some(code) = physical {
                            let mapped = match (code, shift) {
                                (KeyCode::KeyY, false) => Some(Key::Character("y".into())),
                                (KeyCode::KeyY, true) => Some(Key::Character("Y".into())),
                                (KeyCode::KeyN, false) => Some(Key::Character("n".into())),
                                (KeyCode::KeyN, true) => Some(Key::Character("N".into())),
                                (KeyCode::KeyA, _) => Some(Key::Character(if shift { "A" } else { "a" }.into())),
                                (KeyCode::Escape, _) => Some(Key::Named(winit::keyboard::NamedKey::Escape)),
                                _ => None,
                            };
                            if let Some(key) = mapped {
                                let active_ws = state.active_workspace;
                                let mut pane_ptys: std::collections::HashMap<u64, *mut Pty> = std::collections::HashMap::new();
                                for ws in &mut state.workspaces {
                                    for tab in &mut ws.tabs {
                                        for (pid, pane) in &mut tab.panes {
                                            pane_ptys.insert(*pid, &mut pane.pty as *mut Pty);
                                        }
                                    }
                                }
                                let key_result = state.allow_flow.process_key(
                                    &key,
                                    active_ws,
                                    &mut pane_ptys,
                                );
                                match key_result {
                                    crate::allow_flow::AllowFlowKeyResult::NotConsumed => {}
                                    crate::allow_flow::AllowFlowKeyResult::Consumed => {
                                        state.window.request_redraw();
                                        return;
                                    }
                                    crate::allow_flow::AllowFlowKeyResult::Resolved(decisions) => {
                                        for (req_id, decision) in &decisions {
                                            if let Some((tx, _alive)) = state.pending_ipc_responses.remove(req_id) {
                                                let decision_str = match decision {
                                                    termojinal_claude::AllowDecision::Allow => "allow",
                                                    termojinal_claude::AllowDecision::Deny => "deny",
                                                };
                                                let _ = tx.send(AppIpcResponse::ok(
                                                    serde_json::json!({"decision": decision_str}),
                                                ));
                                            }
                                        }
                                        // Update agent state: if no more pending requests for
                                        // a workspace, transition agent from WaitingForPermission to Running.
                                        for wi in 0..state.agent_infos.len() {
                                            if state.agent_infos[wi].active
                                                && matches!(state.agent_infos[wi].state, AgentState::WaitingForPermission)
                                                && !state.allow_flow.has_pending_for_workspace(wi)
                                            {
                                                state.agent_infos[wi].state = AgentState::Running;
                                                state.agent_infos[wi].last_updated = std::time::Instant::now();
                                            }
                                        }
                                        state.window.request_redraw();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                // Suppress raw key events during IME composition (but not when palette is open).
                let focused_id = active_tab(state).layout.focused();
                if !state.command_palette.visible {
                    if active_tab(state).panes.get(&focused_id).map_or(false, |p| p.preedit.is_some()) {
                        return;
                    }
                }

                // Emacs keybindings: Ctrl+N = Down, Ctrl+P = Up (in palette/command UI)
                let is_ctrl = state.modifiers.control_key();
                if is_ctrl && (state.command_palette.visible || state.command_execution.is_some()) {
                    match &event.logical_key {
                        Key::Character(c) if c.as_str() == "n" || c.as_str() == "\x0e" => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.selected = if exec.filtered_items.is_empty() {
                                    0
                                } else if exec.selected + 1 >= exec.filtered_items.len() {
                                    0
                                } else {
                                    exec.selected + 1
                                };
                            } else {
                                state.command_palette.select_next();
                            }
                            state.window.request_redraw();
                            return;
                        }
                        Key::Character(c) if c.as_str() == "p" || c.as_str() == "\x10" => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.selected = if exec.filtered_items.is_empty() {
                                    0
                                } else if exec.selected == 0 {
                                    exec.filtered_items.len() - 1
                                } else {
                                    exec.selected - 1
                                };
                            } else {
                                state.command_palette.select_prev();
                            }
                            state.window.request_redraw();
                            return;
                        }
                        _ => {}
                    }
                }

                // Active command execution intercepts ALL keyboard input.
                if state.command_execution.is_some() && state.command_palette.visible {
                    let result = state.command_execution.as_mut().unwrap().handle_key(&event);
                    match result {
                        CommandKeyResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        CommandKeyResult::Cancelled | CommandKeyResult::Dismiss => {
                            state.command_execution = None;
                            state.command_palette.visible = false;
                            state.window.request_redraw();
                            return;
                        }
                    }
                }

                // Command palette intercepts ALL keyboard input when visible.
                if state.command_palette.visible {
                    match state.command_palette.handle_key(&event) {
                        PaletteResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Execute(action) => {
                            state.command_palette.visible = false;
                            dispatch_action(
                                state,
                                &action,
                                &self.proxy,
                                &self.pty_buffers,
                                event_loop,
                            );
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Dismiss => {
                            state.command_palette.visible = false;
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Pass => {}
                    }
                }

                // Search bar intercepts keyboard input when visible (Feature 5).
                if state.search.is_some() {
                    let search_result = handle_search_key(state, &event);
                    match search_result {
                        SearchKeyResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        SearchKeyResult::Dismiss => {
                            state.search = None;
                            state.window.request_redraw();
                            return;
                        }
                        SearchKeyResult::Pass => {
                            // Escape was not pressed, fall through.
                        }
                    }
                }

                // Quick Terminal: Esc dismisses when no sub-UI is active.
                if state.quick_terminal.active
                    && state.quick_terminal.visible
                    && state.config.quick_terminal.dismiss_on_esc
                    && event.logical_key == Key::Named(NamedKey::Escape)
                    && !state.command_palette.visible
                    && state.command_execution.is_none()
                    && state.search.is_none()
                {
                    hide_quick_terminal(state);
                    return;
                }

                // Try keybinding lookup (all keybindings are now routed through the TOML config).
                if let Some(binding_str) = key_to_binding_string(&event, state.modifiers) {
                    // Determine layer: check if focused pane is in alternate_screen.
                    let is_alt_screen = active_tab(state)
                        .panes
                        .get(&focused_id)
                        .map(|p| p.terminal.modes.alternate_screen)
                        .unwrap_or(false);

                    let action = if is_alt_screen {
                        state
                            .keybindings
                            .lookup_alternate_screen(&binding_str)
                            .or_else(|| state.keybindings.lookup_normal(&binding_str))
                    } else {
                        state.keybindings.lookup_normal(&binding_str)
                    };

                    if let Some(action) = action.cloned() {
                        if dispatch_action(
                            state,
                            &action,
                            &self.proxy,
                            &self.pty_buffers,
                            event_loop,
                        ) {
                            return;
                        }
                        // Action::Passthrough falls through to send key to PTY.
                    }
                }

                // Clear selection on any keypress, EXCEPT modifier-only keys
                // (so that Cmd+C can copy without the Cmd press clearing the selection first).
                let is_modifier_only = matches!(
                    event.logical_key,
                    Key::Named(
                        NamedKey::Super
                            | NamedKey::Shift
                            | NamedKey::Control
                            | NamedKey::Alt
                            | NamedKey::Meta
                    )
                );
                if !is_modifier_only {
                    if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                        if pane.selection.is_some() {
                            pane.selection = None;
                            state.window.request_redraw();
                        }
                    }
                }

                // Forward key to PTY.
                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                        let _ = pane.pty.write(&bytes);
                    }
                }
            }

            // IME events.
            WindowEvent::Ime(ime) => {
                // Route IME to command palette/execution when visible.
                if state.command_palette.visible {
                    match ime {
                        winit::event::Ime::Commit(text) => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.preedit.clear();
                                if !text.is_empty() {
                                    exec.input.push_str(&text);
                                    exec.filter_items();
                                }
                            } else {
                                state.command_palette.preedit.clear();
                                if !text.is_empty() {
                                    state.command_palette.input.push_str(&text);
                                    state.command_palette.update_filter();
                                }
                            }
                            state.window.request_redraw();
                        }
                        winit::event::Ime::Preedit(text, _cursor) => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.preedit = text;
                            } else {
                                state.command_palette.preedit = text;
                            }
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                let focused_id = active_tab(state).layout.focused();
                match ime {
                    winit::event::Ime::Enabled => {
                        // IME session started — nothing to do.
                    }
                    winit::event::Ime::Preedit(text, _cursor) => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            if text.is_empty() {
                                pane.preedit = None;
                            } else {
                                pane.preedit = Some(text);
                            }
                            state.window.request_redraw();
                        }
                    }
                    winit::event::Ime::Commit(text) => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            pane.preedit = None;
                            if !text.is_empty() {
                                let _ = pane.pty.write(text.as_bytes());
                            }
                            state.window.request_redraw();
                        }
                    }
                    winit::event::Ime::Disabled => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            pane.preedit = None;
                            state.window.request_redraw();
                        }
                    }
                }
            }

            // --- Mouse events ---

            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_pos = (position.x, position.y);

                // --- Sidebar drag active: update sidebar width (Feature 1) ---
                if state.sidebar_drag {
                    let new_width = (position.x as f32).clamp(
                        state.config.sidebar.min_width,
                        state.config.sidebar.max_width,
                    );
                    state.sidebar_width = new_width;
                    resize_all_panes(state);
                    state.window.request_redraw();
                    return;
                }

                // --- Tab drag active: check for reordering (Feature 4) ---
                if state.tab_drag.is_some() {
                    let drag_idx = state.tab_drag.as_ref().unwrap().tab_idx;
                    let drag_start_x = state.tab_drag.as_ref().unwrap().start_x;
                    let cx = position.x as f32;
                    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
                    let local_cx = cx - sidebar_w;
                    let cell_w = state.renderer.cell_size().width;
                    let max_tab_w = state.config.tab_bar.max_width;
                    let ws = active_ws(state);
                    // Determine which tab position the cursor is over.
                    let mut tab_x: f32 = 0.0;
                    let mut target_idx = drag_idx;
                    for (i, tab) in ws.tabs.iter().enumerate() {
                        let tab_w = compute_tab_width(&tab.display_title, cell_w, max_tab_w, state.config.tab_bar.min_tab_width);
                        if local_cx >= tab_x && local_cx < tab_x + tab_w {
                            target_idx = i;
                            break;
                        }
                        tab_x += tab_w;
                    }
                    if target_idx != drag_idx {
                        let ws = active_ws_mut(state);
                        let tab = ws.tabs.remove(drag_idx);
                        ws.tabs.insert(target_idx, tab);
                        if ws.active_tab == drag_idx {
                            ws.active_tab = target_idx;
                        } else if drag_idx < ws.active_tab && target_idx >= ws.active_tab {
                            ws.active_tab -= 1;
                        } else if drag_idx > ws.active_tab && target_idx <= ws.active_tab {
                            ws.active_tab += 1;
                        }
                        state.tab_drag = Some(TabDrag {
                            tab_idx: target_idx,
                            start_x: drag_start_x,
                        });
                        state.window.request_redraw();
                    }
                    return;
                }

                // --- Drag-resize active: update layout ---
                if let Some(ref mut drag) = state.drag_resize {
                    let current_pos = match drag.direction {
                        SplitDirection::Horizontal => position.x,
                        SplitDirection::Vertical => position.y,
                    };
                    let pixel_delta = current_pos - drag.last_pos;
                    drag.last_pos = current_pos;

                    // Scale pixel delta to the 1000x1000 coordinate space used
                    // by LayoutTree::resize().
                    let size = state.window.inner_size();
                    let phys_w = size.width as f32;
                    let phys_h = size.height as f32;
                    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
                    let ws = &state.workspaces[state.active_workspace];
                    let show_tab_bar = state.config.tab_bar.always_show || ws.tabs.len() > 1;
                    let tab_bar_h = if show_tab_bar { state.config.tab_bar.height } else { 0.0 };
                    let cw = (phys_w - sidebar_w).max(1.0);
                    let ch = (phys_h - tab_bar_h).max(1.0);
                    let actual_dim = match drag.direction {
                        SplitDirection::Horizontal => cw as f64,
                        SplitDirection::Vertical => ch as f64,
                    };
                    if actual_dim > 0.0 {
                        let scaled_delta = (pixel_delta * 1000.0 / actual_dim) as f32;
                        let pane_id = drag.pane_id;
                        let direction = drag.direction;
                        let tab = active_tab_mut(state);
                        tab.layout = tab.layout.resize(pane_id, direction, scaled_delta);
                        resize_all_panes(state);
                        state.window.request_redraw();
                    }
                } else {
                    // --- Cursor icon management: show resize cursor near separators or sidebar edge ---
                    let pane_rects = active_pane_rects(state);
                    let mx = position.x as f32;
                    let my = position.y as f32;

                    // Check sidebar edge first (Feature 1).
                    let sep_tol = state.config.pane.separator_tolerance;
                    let near_sidebar_edge = state.sidebar_visible
                        && (mx - state.sidebar_width).abs() < sep_tol;

                    if near_sidebar_edge {
                        state.window.set_cursor(CursorIcon::ColResize);
                    } else if let Some((dir, _)) = find_separator(&pane_rects, mx, my, 4.0) {
                        let icon = match dir {
                            SplitDirection::Horizontal => CursorIcon::ColResize,
                            SplitDirection::Vertical => CursorIcon::RowResize,
                        };
                        state.window.set_cursor(icon);
                    } else {
                        // Context-aware cursor: sidebar → pointer, tab bar → pointer/hand, pane → text
                        let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
                        let tab_h = if tab_bar_visible(state) { state.config.tab_bar.height } else { 0.0 };
                        if mx < sidebar_w {
                            state.window.set_cursor(CursorIcon::Pointer);
                        } else if my < tab_h {
                            // In tab bar: hand on close buttons, pointer elsewhere
                            let cursor = tab_bar_cursor(state, mx, my);
                            state.window.set_cursor(cursor);
                        } else {
                            state.window.set_cursor(CursorIcon::Text);
                        }
                    }

                    // --- Original mouse handling (motion reporting / selection) ---
                    let focused_id = active_tab(state).layout.focused();
                    let cell_size = state.renderer.cell_size();
                    let tab = active_tab_mut(state);
                    if let Some(pane) = tab.panes.get_mut(&focused_id) {
                        // Handle mouse motion reporting for the terminal.
                        if pane.terminal.modes.mouse_mode == MouseMode::AnyMotion
                            || (pane.terminal.modes.mouse_mode == MouseMode::ButtonMotion
                                && pane
                                    .selection
                                    .as_ref()
                                    .map_or(false, |s| s.active))
                        {
                            if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                // Subtract in f64 to avoid rounding the cursor
                                // position before the subtraction, then floor to
                                // get the correct column/row index.
                                let local_x = ((position.x - rect.x as f64) as f32).max(0.0);
                                let local_y = ((position.y - rect.y as f64) as f32).max(0.0);
                                let col = (local_x / cell_size.width).floor() as usize;
                                let row = (local_y / cell_size.height).floor() as usize;
                                // Motion event: button 32 + 0 = 32.
                                let seq = encode_mouse_sgr(32, col, row, true);
                                let _ = pane.pty.write(&seq);
                            }
                        } else if let Some(ref mut sel) = pane.selection {
                            // Non-mouse-mode selection dragging.
                            if sel.active {
                                if let Some((_, rect)) =
                                    pane_rects.iter().find(|(id, _)| *id == focused_id)
                                {
                                    let local_x = ((position.x - rect.x as f64) as f32).max(0.0);
                                    let local_y = ((position.y - rect.y as f64) as f32).max(0.0);
                                    let so = pane.terminal.scroll_offset();
                                    sel.end = GridPos {
                                        col: (local_x / cell_size.width).floor() as usize,
                                        row: (local_y / cell_size.height).floor() as usize,
                                    };
                                    sel.scroll_offset_at_end = so;
                                }
                            }
                        }
                    }
                    state.window.request_redraw();
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => 'mouse_input: {
                // --- Handle drag-resize / sidebar-drag / tab-drag release ---
                if btn_state == ElementState::Released && button == MouseButton::Left {
                    if state.sidebar_drag {
                        state.sidebar_drag = false;
                        state.window.set_cursor(CursorIcon::Default);
                        break 'mouse_input;
                    }
                    if state.tab_drag.is_some() {
                        state.tab_drag = None;
                        state.window.request_redraw();
                        break 'mouse_input;
                    }
                    if state.drag_resize.is_some() {
                        state.drag_resize = None;
                        state.window.set_cursor(CursorIcon::Default);
                        break 'mouse_input;
                    }
                }

                // --- Priority 0: Check if click is on sidebar right edge for DnD resize (Feature 1) ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.sidebar_visible
                {
                    let cx = state.cursor_pos.0 as f32;
                    if (cx - state.sidebar_width).abs() < state.config.pane.separator_tolerance {
                        state.sidebar_drag = true;
                        state.window.set_cursor(CursorIcon::ColResize);
                        break 'mouse_input;
                    }
                }

                // --- Priority 0: Check if click is in the sidebar area ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.sidebar_visible
                {
                    let cx = state.cursor_pos.0 as f32;
                    if cx < state.sidebar_width {
                        if let Some(action) = handle_sidebar_click(state) {
                            let proxy = &self.proxy;
                            let buffers = &self.pty_buffers;
                            dispatch_action(state, &action, proxy, buffers, event_loop);
                        }
                        break 'mouse_input;
                    }
                }

                // --- Priority 0.5: Check if click is in the tab bar area (Feature 4: tab drag) ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let tab_bar_h = if tab_bar_visible(state) { state.config.tab_bar.height } else { 0.0 };
                    let cy = state.cursor_pos.1 as f32;
                    if tab_bar_h > 0.0 && cy < tab_bar_h {
                        match handle_tab_bar_click(state) {
                            TabBarClickResult::Tab(tab_idx) => {
                                state.tab_drag = Some(TabDrag {
                                    tab_idx,
                                    start_x: state.cursor_pos.0,
                                });
                            }
                            TabBarClickResult::CloseTab(tab_idx) => {
                                // Close the clicked tab.
                                let ws = active_ws_mut(state);
                                if ws.tabs.len() > 1 {
                                    ws.tabs.remove(tab_idx);
                                    if ws.active_tab >= ws.tabs.len() {
                                        ws.active_tab = ws.tabs.len() - 1;
                                    } else if ws.active_tab > tab_idx {
                                        ws.active_tab -= 1;
                                    }
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                } else {
                                    // Last tab — close focused pane instead.
                                    close_focused_pane(
                                        state,
                                        &self.pty_buffers,
                                        event_loop,
                                    );
                                }
                            }
                            TabBarClickResult::NewTab => {
                                dispatch_action(
                                    state,
                                    &Action::NewTab,
                                    &self.proxy,
                                    &self.pty_buffers,
                                    event_loop,
                                );
                            }
                            TabBarClickResult::None => {}
                        }
                        break 'mouse_input;
                    }
                }

                let focused_id = active_tab(state).layout.focused();

                // --- Priority 1: Check if clicking on a separator → start drag resize ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;

                    if let Some((direction, pane_id)) = find_separator(&pane_rects, cx, cy, 4.0) {
                        let last_pos = match direction {
                            SplitDirection::Horizontal => state.cursor_pos.0,
                            SplitDirection::Vertical => state.cursor_pos.1,
                        };
                        let icon = match direction {
                            SplitDirection::Horizontal => CursorIcon::ColResize,
                            SplitDirection::Vertical => CursorIcon::RowResize,
                        };
                        state.window.set_cursor(icon);
                        state.drag_resize = Some(DragResize {
                            direction,
                            pane_id,
                            last_pos,
                        });
                        // Don't fall through to focus-change or selection.
                        break 'mouse_input;
                    }
                }

                // --- Priority 1.5: Cmd+click on a pane → extract to new tab (Feature 4) ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.modifiers.super_key()
                {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    let tab = active_tab(state);
                    // Only allow extraction if there are multiple panes.
                    if tab.layout.pane_count() > 1 {
                        for (pid, rect) in &pane_rects {
                            if cx >= rect.x
                                && cx < rect.x + rect.w
                                && cy >= rect.y
                                && cy < rect.y + rect.h
                            {
                                let target_pane = *pid;
                                let tab = active_tab(state);
                                if let Some((remaining, _extracted)) = tab.layout.extract_pane(target_pane) {
                                    // Remove the pane from current tab and create a new tab with it.
                                    let pane = active_tab_mut(state).panes.remove(&target_pane);
                                    active_tab_mut(state).layout = remaining;

                                    if let Some(pane) = pane {
                                        let new_layout = LayoutTree::new(target_pane);
                                        let mut new_panes = HashMap::new();
                                        new_panes.insert(target_pane, pane);
                                        let fmt = state.config.tab_bar.format.clone();
                                        let fb_cwd = state.pane_git_cache.cwd.clone();
                                        let ws = active_ws_mut(state);
                                        let tab_num = ws.tabs.len() + 1;
                                        let new_tab = Tab {
                                            layout: new_layout,
                                            panes: new_panes,
                                            name: format!("Tab {tab_num}"),
                                            display_title: format_tab_title(&fmt, "", "", tab_num),
                                        };
                                        ws.tabs.push(new_tab);
                                        let new_tab_idx = ws.tabs.len() - 1;
                                        ws.active_tab = new_tab_idx;
                                        update_tab_title(&mut ws.tabs[new_tab_idx], &fmt, tab_num, &fb_cwd);
                                    }
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                    break 'mouse_input;
                                }
                                break;
                            }
                        }
                    }
                }

                // --- Priority 2: Check if click is in a different pane → change focus ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    for (pid, rect) in &pane_rects {
                        if *pid != focused_id
                            && cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            let tab = active_tab_mut(state);
                            tab.layout = tab.layout.focus(*pid);
                            update_window_title(state);
                            state.window.request_redraw();
                            break;
                        }
                    }
                }

                // --- Priority 3: Selection / mouse-mode forwarding ---
                let focused_id = active_tab(state).layout.focused();
                let cell_size = state.renderer.cell_size();
                let cursor_pos = state.cursor_pos;
                let pane_rects = active_pane_rects(state);
                let tab = active_tab_mut(state);

                if let Some(pane) = tab.panes.get_mut(&focused_id) {
                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward mouse event to PTY.
                        let btn_code = match button {
                            MouseButton::Left => 0u8,
                            MouseButton::Middle => 1,
                            MouseButton::Right => 2,
                            _ => return,
                        };
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            // Subtract in f64 to avoid rounding before subtraction.
                            let local_x = ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                            let local_y = ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                            let col = (local_x / cell_size.width).floor() as usize;
                            let row = (local_y / cell_size.height).floor() as usize;
                            let pressed = btn_state == ElementState::Pressed;
                            let seq = encode_mouse_sgr(btn_code, col, row, pressed);
                            let _ = pane.pty.write(&seq);
                        }
                    } else {
                        // Selection mode.
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            // cursor_pos is in physical pixels (from CursorMoved PhysicalPosition).
                            // rect and cell_size are also in physical pixels.
                            let local_x = ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                            let local_y = ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                            let pos = GridPos {
                                col: (local_x / cell_size.width).floor() as usize,
                                row: (local_y / cell_size.height).floor() as usize,
                            };

                            if button == MouseButton::Left {
                                match btn_state {
                                    ElementState::Pressed => {
                                        let so = pane.terminal.scroll_offset();
                                        pane.selection = Some(Selection {
                                            start: pos,
                                            end: pos,
                                            active: true,
                                            scroll_offset_at_start: so,
                                            scroll_offset_at_end: so,
                                        });
                                    }
                                    ElementState::Released => {
                                        if let Some(ref mut sel) = pane.selection {
                                            sel.active = false;
                                            let so = pane.terminal.scroll_offset();
                                            sel.end = pos;
                                            sel.scroll_offset_at_end = so;
                                            if sel.is_empty() {
                                                pane.selection = None;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Right-click: show native context menu.
                        #[cfg(target_os = "macos")]
                        if button == MouseButton::Right && btn_state == ElementState::Pressed {
                            let has_sel = pane.selection.is_some();
                            if let Some(action) = show_context_menu(&state.window, has_sel) {
                                let mapped = match action {
                                    ContextMenuAction::Copy => Action::Copy,
                                    ContextMenuAction::Paste => Action::Paste,
                                    ContextMenuAction::SelectAll => Action::SelectAll,
                                    ContextMenuAction::Clear => Action::ClearScrollback,
                                    ContextMenuAction::SplitRight => Action::SplitRight,
                                    ContextMenuAction::SplitDown => Action::SplitDown,
                                };
                                dispatch_action(
                                    state,
                                    &mapped,
                                    &self.proxy,
                                    &self.pty_buffers,
                                    event_loop,
                                );
                            }
                        }
                    }
                }
                state.window.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let focused_id = active_tab(state).layout.focused();
                let cell_size = state.renderer.cell_size();
                let cell_h = cell_size.height as f64;
                let cursor_pos = state.cursor_pos;
                let pane_rects = active_pane_rects(state);
                let tab = active_tab_mut(state);
                if let Some(pane) = tab.panes.get_mut(&focused_id) {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => {
                            (pos.y / cell_h).round() as i32
                        }
                    };

                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward scroll as mouse events.
                        // Scroll up = button 64, scroll down = button 65.
                        if lines != 0 {
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let local_x =
                                    ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                                let local_y =
                                    ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                                let col = (local_x / cell_size.width).floor() as usize;
                                let row = (local_y / cell_size.height).floor() as usize;
                                let count = lines.unsigned_abs();
                                let btn = if lines > 0 { 64u8 } else { 65u8 };
                                for _ in 0..count {
                                    let seq = encode_mouse_sgr(btn, col, row, true);
                                    let _ = pane.pty.write(&seq);
                                }
                            }
                        }
                    } else if lines != 0 {
                        let current = pane.terminal.scroll_offset() as i32;
                        let new_offset = (current + lines).max(0) as usize;
                        pane.terminal.set_scroll_offset(new_offset);
                        state.window.request_redraw();
                    }
                }
            }

            WindowEvent::ThemeChanged(_winit_theme) => {
                // macOS appearance changed (Dark <-> Light).
                if state.config.theme.auto_switch {
                    let new_appearance = appearance::detect_macos_appearance();
                    let effective_theme = resolve_theme(&state.config, new_appearance);
                    let palette = ThemePalette::from_theme_colors(
                        &effective_theme.background,
                        &effective_theme.foreground,
                        &effective_theme.black,
                        &effective_theme.red,
                        &effective_theme.green,
                        &effective_theme.yellow,
                        &effective_theme.blue,
                        &effective_theme.magenta,
                        &effective_theme.cyan,
                        &effective_theme.white,
                        &effective_theme.bright_black,
                        &effective_theme.bright_red,
                        &effective_theme.bright_green,
                        &effective_theme.bright_yellow,
                        &effective_theme.bright_blue,
                        &effective_theme.bright_magenta,
                        &effective_theme.bright_cyan,
                        &effective_theme.bright_white,
                    );
                    state.renderer.set_theme(palette);
                    state.renderer.default_bg =
                        color_or(&effective_theme.background, [0.067, 0.067, 0.09, 1.0]);
                    state.renderer.preedit_bg =
                        color_or(&effective_theme.preedit_bg, [0.15, 0.15, 0.20, 1.0]);
                    log::info!("theme switched to {:?}", new_appearance);
                    state.window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                // Poll active command execution for new messages.
                if let Some(ref mut exec) = state.command_execution {
                    if exec.poll() {
                        // State changed; check if the command completed.
                        if exec.is_done() {
                            // Send macOS desktop notification on Done.
                            if let CommandUIState::Done(Some(ref msg)) = exec.ui_state {
                                log::info!("command '{}' done: {}", exec.command_name, msg);
                                if state.config.notifications.enabled {
                                    notification::send_notification(
                                        "termojinal",
                                        msg,
                                        state.config.notifications.sound,
                                    );
                                }
                            }
                        }
                    }
                    // Keep requesting redraws while a command is active
                    // so we poll for new messages.
                    state.window.request_redraw();
                }

                // Quick Terminal animation tick.
                if tick_quick_terminal_animation(state) {
                    state.window.request_redraw();
                }

                // Agent pulse animation: request continuous redraws when any
                // workspace has an active agent with pulse indicator style.
                if state.sidebar_visible
                    && state.config.sidebar.agent_status_enabled
                    && state.config.sidebar.agent_indicator_style == "pulse"
                    && state.agent_infos.iter().any(|a| a.active)
                {
                    state.window.request_redraw();
                }

                if let Err(e) = render_frame(state) {
                    log::error!("render error: {e}");
                }
            }

            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput(pane_id) => {
                let Some(state) = &mut self.state else {
                    return;
                };
                // Find the pane across all workspaces and tabs.
                let cell_size = state.renderer.cell_size();
                let cell_w = cell_size.width as u32;
                let cell_h = cell_size.height as u32;
                let mut lock = self.pty_buffers.lock().unwrap();
                let mut total = 0usize;
                let mut found_ws_idx: Option<usize> = None;
                if let Some(q) = lock.get_mut(&pane_id) {
                    'outer_feed: for (wi, ws) in state.workspaces.iter_mut().enumerate() {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
                                // Sync cell size so image protocols compute
                                // correct cell spans before processing data.
                                pane.terminal.image_store.set_cell_size(cell_w, cell_h);
                                while let Some(data) = q.pop_front() {
                                    total += data.len();
                                    pane.terminal.feed(&mut pane.vt_parser, &data);
                                }
                                found_ws_idx = Some(wi);
                                break 'outer_feed;
                            }
                        }
                    }
                }
                drop(lock);

                // Mark non-active workspaces as having unread output.
                if let Some(wi) = found_ws_idx {
                    if total > 0 && wi != state.active_workspace {
                        if wi < state.workspace_infos.len() {
                            state.workspace_infos[wi].has_unread = true;
                        }
                    }
                }

                // Handle OSC 52 clipboard events.
                'outer_clip: for ws in &mut state.workspaces {
                    for tab in &mut ws.tabs {
                        if let Some(pane) = tab.panes.get_mut(&pane_id) {
                            if let Some(ref clipboard_event) = pane.terminal.clipboard_event.take() {
                                match clipboard_event {
                                    ClipboardEvent::Set { data, .. } => {
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            let _ = cb.set_text(data);
                                        }
                                    }
                                    ClipboardEvent::Query { selection } => {
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            if let Ok(text) = cb.get_text() {
                                                use base64::Engine as _;
                                                let b64 = base64::engine::general_purpose::STANDARD
                                                    .encode(text.as_bytes());
                                                let response =
                                                    format!("\x1b]52;{selection};{b64}\x07");
                                                let _ = pane.pty.write(response.as_bytes());
                                            }
                                        }
                                    }
                                }
                            }
                            break 'outer_clip;
                        }
                    }
                }

                // ---------------------------------------------------------------
                // Allow Flow: check for OSC notifications and scan output
                // ---------------------------------------------------------------
                if total > 0 {
                    // Check for Allow Flow notifications via OSC 9/99/777.
                    let ws_idx = found_ws_idx.unwrap_or(state.active_workspace);
                    let mut has_notification = false;
                    'outer_osc: for ws in &mut state.workspaces {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
                                if let Some(notification) = pane.terminal.osc.last_notification.take() {
                                    // Send desktop notification if enabled and window not focused.
                                    if state.config.notifications.enabled
                                        && !state.window.has_focus()
                                    {
                                        notification::send_notification(
                                            "termojinal",
                                            &notification,
                                            state.config.notifications.sound,
                                        );
                                    }
                                    if let Some(_req) = state.allow_flow.engine.process_osc(
                                        pane_id, ws_idx, &notification
                                    ) {
                                        state.allow_flow.pane_hint_visible = true;
                                        has_notification = true;
                                    }
                                }
                                break 'outer_osc;
                            }
                        }
                    }

                    // Scan visible lines for Allow prompts (only on new data).
                    if !has_notification {
                        'outer_scan: for ws in &state.workspaces {
                            for tab in &ws.tabs {
                                if let Some(pane) = tab.panes.get(&pane_id) {
                                    let grid = pane.terminal.grid();
                                    let scan_rows = grid.rows().min(5);
                                    let visible_lines: Vec<String> = (0..scan_rows)
                                        .rev()
                                        .map(|row_idx| {
                                            (0..grid.cols())
                                                .map(|col| {
                                                    let c = grid.cell(col, row_idx).c;
                                                    if c == '\0' { ' ' } else { c }
                                                })
                                                .collect::<String>()
                                        })
                                        .collect();
                                    let line_refs: Vec<&str> = visible_lines.iter().map(|s| s.as_str()).collect();
                                    if let Some(_req) = state.allow_flow.engine.process_output(
                                        pane_id, ws_idx, &line_refs
                                    ) {
                                        state.allow_flow.pane_hint_visible = true;
                                    }
                                    break 'outer_scan;
                                }
                            }
                        }
                    }
                }

                // Nudge status collector so it picks up changes quickly
                // (e.g., SSH exit, cd, git operations).
                if total > 0 {
                    state.status_collector.nudge();
                }

                // Update tab display titles after VT feed.
                if total > 0 {
                    let fmt = state.config.tab_bar.format.clone();
                    for ws in &mut state.workspaces {
                        for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                            if tab.panes.contains_key(&pane_id) {
                                update_tab_title(tab, &fmt, ti + 1, &state.pane_git_cache.cwd.clone());
                            }
                        }
                    }
                    update_window_title(state);
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited(pane_id) => {
                log::info!("pane {pane_id}: shell exited");
                unregister_pane_from_daemon(pane_id);
                let Some(state) = &mut self.state else {
                    return;
                };

                // Find which workspace/tab owns this pane and remove it.
                self.pty_buffers.lock().unwrap().remove(&pane_id);

                let mut found = None; // (ws_idx, tab_idx)
                for (wi, ws) in state.workspaces.iter_mut().enumerate() {
                    for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                        if tab.panes.remove(&pane_id).is_some() {
                            found = Some((wi, ti));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }

                let Some((ws_idx, tab_idx)) = found else {
                    return;
                };

                let tab = &state.workspaces[ws_idx].tabs[tab_idx];
                match tab.layout.close(pane_id) {
                    Some(new_layout) => {
                        state.workspaces[ws_idx].tabs[tab_idx].layout = new_layout;
                        resize_all_panes(state);
                        update_window_title(state);
                        state.window.request_redraw();
                    }
                    None => {
                        // Last pane in the tab exited — close the tab.
                        let ws = &mut state.workspaces[ws_idx];
                        if ws.tabs.len() == 1 {
                            // Last tab in this workspace — close the workspace.
                            if state.workspaces.len() == 1 {
                                // Last workspace — quit.
                                event_loop.exit();
                            } else {
                                state.workspaces.remove(ws_idx);
                                if ws_idx < state.workspace_infos.len() {
                                    state.workspace_infos.remove(ws_idx);
                                }
                                if ws_idx < state.agent_infos.len() {
                                    state.agent_infos.remove(ws_idx);
                                }
                                cleanup_session_to_workspace(state, ws_idx);
                                if state.active_workspace >= state.workspaces.len() {
                                    state.active_workspace = state.workspaces.len() - 1;
                                }
                                resize_all_panes(state);
                                update_window_title(state);
                                state.window.request_redraw();
                            }
                        } else {
                            ws.tabs.remove(tab_idx);
                            if ws.active_tab >= ws.tabs.len() {
                                ws.active_tab = ws.tabs.len() - 1;
                            }
                            resize_all_panes(state);
                            update_window_title(state);
                            state.window.request_redraw();
                        }
                    }
                }
            }
            UserEvent::StatusUpdate => {
                let Some(state) = &mut self.state else { return };
                // Update tab titles with the latest CWD from background thread.
                let fmt = state.config.tab_bar.format.clone();
                let cwd = state.pane_git_cache.cwd.clone();
                let ws = active_ws_mut(state);
                for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                    update_tab_title(tab, &fmt, ti + 1, &cwd);
                }
                update_window_title(state);
                state.window.request_redraw();
            }
            UserEvent::ToggleQuickTerminal => {
                log::info!("toggle quick terminal requested");
                let Some(state) = &mut self.state else { return };
                toggle_quick_terminal(state);
            }
            UserEvent::AppIpc {
                request,
                response_tx,
                connection_alive,
            } => {
                let Some(state) = &mut self.state else { return };
                if let Some(response) = handle_app_ipc_request(
                    state,
                    &request,
                    &self.proxy,
                    &self.pty_buffers,
                    event_loop,
                    &response_tx,
                    connection_alive,
                ) {
                    let _ = response_tx.send(response);
                }
                // If None, the response is deferred (PermissionRequest).
                state.window.request_redraw();
            }
            UserEvent::IpcClientDisconnected => {
                let Some(state) = &mut self.state else { return };
                // Remove pending IPC responses whose connection is dead.
                let stale_ids: Vec<u64> = state
                    .pending_ipc_responses
                    .iter()
                    .filter(|(_, (_, alive))| !alive.load(Ordering::SeqCst))
                    .map(|(id, _)| *id)
                    .collect();

                if stale_ids.is_empty() {
                    return;
                }

                log::info!(
                    "IPC client disconnected — cleaning up {} stale pending request(s)",
                    stale_ids.len()
                );

                for req_id in &stale_ids {
                    state.pending_ipc_responses.remove(req_id);
                    state.allow_flow.engine.dismiss_request(*req_id);
                }

                // Update pane hint visibility.
                if state.allow_flow.engine.pending_requests().is_empty() {
                    state.allow_flow.pane_hint_visible = false;
                }

                // Reset agent state for workspaces that no longer have pending requests.
                for wi in 0..state.agent_infos.len() {
                    if state.agent_infos[wi].active
                        && matches!(
                            state.agent_infos[wi].state,
                            AgentState::WaitingForPermission
                        )
                        && !state.allow_flow.has_pending_for_workspace(wi)
                    {
                        state.agent_infos[wi].state = AgentState::Idle;
                        state.agent_infos[wi].last_updated = std::time::Instant::now();
                    }
                }

                state.window.request_redraw();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Search key handling (Feature 5)
// ---------------------------------------------------------------------------

enum SearchKeyResult {
    Consumed,
    Dismiss,
    Pass,
}

fn handle_search_key(state: &mut AppState, event: &winit::event::KeyEvent) -> SearchKeyResult {
    if state.search.is_none() {
        return SearchKeyResult::Pass;
    }

    match &event.logical_key {
        Key::Named(NamedKey::Escape) => {
            return SearchKeyResult::Dismiss;
        }
        Key::Named(NamedKey::Enter) => {
            let shift = state.modifiers.shift_key();
            if let Some(ref mut search) = state.search {
                if shift {
                    search.prev_match();
                } else {
                    search.next_match();
                }
            }
            return SearchKeyResult::Consumed;
        }
        Key::Named(NamedKey::Backspace) => {
            if let Some(ref mut search) = state.search {
                search.query.pop();
            }
            // Re-search after modifying query.
            search_in_focused_pane(state);
            return SearchKeyResult::Consumed;
        }
        _ => {
            if let Some(ref text) = event.text {
                if !text.is_empty() && !text.contains('\r') && !text.contains('\x1b') {
                    let text = text.clone();
                    if let Some(ref mut search) = state.search {
                        search.query.push_str(&text);
                    }
                    search_in_focused_pane(state);
                    return SearchKeyResult::Consumed;
                }
            }
        }
    }

    SearchKeyResult::Pass
}

/// Run search on the focused pane's grid.
fn search_in_focused_pane(state: &mut AppState) {
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    let focused_id = state.workspaces[ws_idx].tabs[tab_idx].layout.focused();
    // We need to get a reference to the grid, then run search.
    // Build search matches from grid data, then update state.search.
    let query = state.search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
    if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(&focused_id) {
        let grid = pane.terminal.grid();
        // Perform inline search to avoid borrow issues.
        let mut matches = Vec::new();
        if !query.is_empty() {
            let query_lower = query.to_lowercase();
            let query_chars: Vec<char> = query_lower.chars().collect();
            let qlen = query_chars.len();
            for row in 0..grid.rows() {
                let mut row_lower = Vec::with_capacity(grid.cols());
                for col in 0..grid.cols() {
                    let cell = grid.cell(col, row);
                    let c = if cell.c == '\0' { ' ' } else { cell.c };
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    row_lower.push(s.to_lowercase().chars().next().unwrap_or(c));
                }
                for start_col in 0..row_lower.len() {
                    if start_col + qlen > row_lower.len() {
                        break;
                    }
                    let mut found = true;
                    for (qi, qc) in query_chars.iter().enumerate() {
                        if row_lower[start_col + qi] != *qc {
                            found = false;
                            break;
                        }
                    }
                    if found {
                        matches.push((row, start_col, start_col + qlen - 1));
                    }
                }
            }
        }
        if let Some(ref mut search) = state.search {
            search.matches = matches;
            search.current = 0;
        }
    }
}

/// Dispatch an action from keybinding or command palette.
///
/// Returns `true` if the action was handled (caller should `return`),
/// or `false` if the key should fall through to PTY (e.g., `Passthrough`).
fn dispatch_action(
    state: &mut AppState,
    action: &Action,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) -> bool {
    let focused_id = active_tab(state).layout.focused();
    match action {
        Action::SplitRight => {
            let cwd = resolve_new_pane_cwd(state);
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) =
                tab.layout.split(focused_id, SplitDirection::Horizontal);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) =
                    state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers, cwd) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::SplitDown => {
            let cwd = resolve_new_pane_cwd(state);
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) =
                tab.layout.split(focused_id, SplitDirection::Vertical);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) =
                    state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers, cwd) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::ZoomPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.toggle_zoom();
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::NextPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Next);
            let fmt = state.config.tab_bar.format.clone();
            let fb_cwd = state.pane_git_cache.cwd.clone();
            let ws = active_ws_mut(state);
            let ti = ws.active_tab;
            update_tab_title(&mut ws.tabs[ti], &fmt, ti + 1, &fb_cwd);
            update_window_title(state);
            state.window.request_redraw();
            true
        }
        Action::PrevPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Prev);
            let fmt = state.config.tab_bar.format.clone();
            let fb_cwd = state.pane_git_cache.cwd.clone();
            let ws = active_ws_mut(state);
            let ti = ws.active_tab;
            update_tab_title(&mut ws.tabs[ti], &fmt, ti + 1, &fb_cwd);
            update_window_title(state);
            state.window.request_redraw();
            true
        }
        Action::NewTab => {
            // Create a new tab in the current workspace.
            let cwd = resolve_new_pane_cwd(state);
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            // When we add a new tab, the workspace will have >1 tabs, so tab bar will appear.
            let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = (phys_h - state.config.tab_bar.height).max(1.0);
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers, cwd) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let fmt = state.config.tab_bar.format.clone();
                    let ws = active_ws_mut(state);
                    let tab_num = ws.tabs.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: format!("Tab {tab_num}"),
                        display_title: format_tab_title(&fmt, "", "", tab_num),
                    };
                    ws.tabs.push(tab);
                    ws.active_tab = ws.tabs.len() - 1;
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new tab: {e}");
                }
            }
            true
        }
        Action::CloseTab => {
            close_focused_pane(state, buffers, event_loop);
            true
        }
        Action::NewWorkspace => {
            // Create a new workspace with one tab and one pane.
            let cwd = resolve_new_pane_cwd(state);
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = phys_h.max(1.0); // Single tab, no tab bar
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers, cwd) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws_num = state.workspaces.len() + 1;
                    let fmt = state.config.tab_bar.format.clone();
                    let tab = Tab {
                        layout,
                        panes,
                        name: "Tab 1".to_string(),
                        display_title: format_tab_title(&fmt, "", "", 1),
                    };
                    let ws = Workspace {
                        tabs: vec![tab],
                        active_tab: 0,
                        name: format!("Workspace {ws_num}"),
                    };
                    state.workspaces.push(ws);
                    state.agent_infos.push(AgentSessionInfo::default());
                    state.active_workspace = state.workspaces.len() - 1;
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new workspace: {e}");
                }
            }
            true
        }
        Action::NextTab => {
            let ws = active_ws_mut(state);
            if ws.active_tab + 1 < ws.tabs.len() {
                ws.active_tab += 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::PrevTab => {
            let ws = active_ws_mut(state);
            if ws.active_tab > 0 {
                ws.active_tab -= 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::Workspace(n) => {
            let idx = (*n as usize).saturating_sub(1);
            if idx < state.workspaces.len() {
                state.active_workspace = idx;
                if idx < state.workspace_infos.len() {
                    state.workspace_infos[idx].has_unread = false;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::CommandPalette => {
            // If a command execution is active, cancel it on toggle off.
            if state.command_palette.visible {
                if let Some(ref mut exec) = state.command_execution {
                    exec.runner.cancel();
                }
                state.command_execution = None;
            }
            state.command_palette.toggle();
            state.window.request_redraw();
            true
        }
        Action::Copy => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                if let Some(ref sel) = pane.selection {
                    let text = sel.text(&pane.terminal);
                    if !text.is_empty() {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            let _ = cb.set_text(&text);
                        }
                        // Keep selection visible after copy.
                        state.window.request_redraw();
                        return true;
                    }
                }
            }
            // No selection — send Ctrl+C to the focused pane.
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                let _ = pane.pty.write(&[0x03]);
            }
            true
        }
        Action::Paste => {
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        if pane.terminal.modes.bracketed_paste {
                            let _ = pane.pty.write(b"\x1b[200~");
                            let _ = pane.pty.write(text.as_bytes());
                            let _ = pane.pty.write(b"\x1b[201~");
                        } else {
                            let _ = pane.pty.write(text.as_bytes());
                        }
                    }
                }
            }
            true
        }
        Action::SelectAll => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                let cols = pane.terminal.cols();
                let rows = pane.terminal.rows();
                let sb_len = pane.terminal.scrollback_len();
                // Select from the very top of scrollback to the bottom-right of the visible grid.
                // Scroll to the top so the start of selection is visible.
                pane.terminal.set_scroll_offset(sb_len);
                pane.selection = Some(Selection {
                    start: GridPos { col: 0, row: 0 },
                    end: GridPos {
                        col: cols.saturating_sub(1),
                        row: rows.saturating_sub(1),
                    },
                    active: false,
                    scroll_offset_at_start: sb_len,
                    scroll_offset_at_end: 0,
                });
                state.window.request_redraw();
            }
            true
        }
        Action::ClearScreen => {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab_mut(state).panes.get(&focused_id) {
                let _ = pane.pty.write(b"\x1b[2J\x1b[H");
            }
            true
        }
        Action::ClearScrollback => {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                pane.terminal.clear_all();
            }
            state.window.request_redraw();
            true
        }
        Action::Quit => {
            event_loop.exit();
            true
        }
        Action::ToggleSidebar => {
            state.sidebar_visible = !state.sidebar_visible;
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::NextWorkspace => {
            if state.active_workspace + 1 < state.workspaces.len() {
                state.active_workspace += 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::PrevWorkspace => {
            if state.active_workspace > 0 {
                state.active_workspace -= 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::FontIncrease => {
            let new_size = (state.font_size + state.config.font.size_step).min(state.config.font.max_size);
            if let Err(e) = state.renderer.set_font_size(new_size) {
                log::error!("failed to increase font size: {e}");
            } else {
                state.font_size = new_size;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::FontDecrease => {
            let new_size = (state.font_size - state.config.font.size_step).max(6.0);
            if let Err(e) = state.renderer.set_font_size(new_size) {
                log::error!("failed to decrease font size: {e}");
            } else {
                state.font_size = new_size;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::Search => {
            if state.search.is_some() {
                state.search = None;
            } else {
                state.search = Some(SearchState::new());
            }
            state.window.request_redraw();
            true
        }
        Action::Passthrough => {
            // Force key through to PTY, skip binding.
            false
        }
        Action::None => true,
        Action::AllowFlowPanel => {
            // Open sidebar if closed, then jump to the first workspace
            // with pending Allow Flow requests.
            if !state.sidebar_visible {
                state.sidebar_visible = true;
            }
            if let Some(ws_idx) = state.allow_flow.first_workspace_with_pending() {
                if state.active_workspace != ws_idx {
                    state.active_workspace = ws_idx;
                    resize_all_panes(state);
                    update_window_title(state);
                }
            }
            state.window.request_redraw();
            true
        }
        Action::Command(name) => {
            if let Some(cmd) = state.external_commands.iter().find(|c| c.meta.name == *name) {
                match CommandExecution::new(cmd) {
                    Ok(exec) => {
                        state.command_execution = Some(exec);
                        state.command_palette.visible = true;
                        state.window.request_redraw();
                    }
                    Err(e) => log::error!("failed to start command '{}': {}", name, e),
                }
            } else {
                log::warn!("unknown command: {}", name);
            }
            true
        }
        Action::ToggleQuickTerminal => {
            toggle_quick_terminal(state);
            true
        }
        Action::About => {
            state.about_visible = !state.about_visible;
            state.about_scroll = 0;
            state.window.request_redraw();
            true
        }
        Action::UnreadJump
        | Action::OpenSettings => {
            log::debug!("unhandled action: {:?}", action);
            true
        }
    }
}

/// Resize all panes' terminals and PTYs to match current layout rects.
fn resize_all_panes(state: &mut AppState) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width as u32;
    let cell_h = cell_size.height as u32;
    let pane_rects = active_pane_rects(state);
    for (pid, rect) in &pane_rects {
        let (cols, rows) =
            state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
        let cols = cols.max(1);
        let rows = rows.max(1);
        let tab = active_tab_mut(state);
        if let Some(pane) = tab.panes.get_mut(pid) {
            pane.terminal.resize(cols as usize, rows as usize);
            pane.terminal.image_store.set_cell_size(cell_w, cell_h);
            let _ = pane.pty.resize(PtySize { cols, rows });
        }
    }
}

/// Detect if the mouse at (mx, my) is near a separator between panes.
///
/// Returns `Some((direction, pane_id))` where:
/// - `direction` is the split direction (Horizontal = vertical separator line,
///   Vertical = horizontal separator line)
/// - `pane_id` is the pane on the "first" (left/top) side of the separator
///
/// The threshold is how many pixels from the separator boundary to detect.
fn find_separator(
    pane_rects: &[(PaneId, termojinal_layout::Rect)],
    mx: f32,
    my: f32,
    threshold: f32,
) -> Option<(SplitDirection, PaneId)> {
    for i in 0..pane_rects.len() {
        for j in (i + 1)..pane_rects.len() {
            let (id1, r1) = &pane_rects[i];
            let (_, r2) = &pane_rects[j];

            // Vertical separator: r1 is to the left of r2 (horizontal split).
            let r1_right = r1.x + r1.w;
            if (r1_right - r2.x).abs() <= 2.0 {
                let y_top = r1.y.max(r2.y);
                let y_bot = (r1.y + r1.h).min(r2.y + r2.h);
                if y_bot > y_top
                    && my >= y_top
                    && my <= y_bot
                    && (mx - r1_right).abs() <= threshold
                {
                    return Some((SplitDirection::Horizontal, *id1));
                }
            }

            // Horizontal separator: r1 is above r2 (vertical split).
            let r1_bottom = r1.y + r1.h;
            if (r1_bottom - r2.y).abs() <= 2.0 {
                let x_left = r1.x.max(r2.x);
                let x_right = (r1.x + r1.w).min(r2.x + r2.w);
                if x_right > x_left
                    && mx >= x_left
                    && mx <= x_right
                    && (my - r1_bottom).abs() <= threshold
                {
                    return Some((SplitDirection::Vertical, *id1));
                }
            }
        }
    }
    None
}

/// Close the focused pane. If it is the last pane in the tab, close the tab.
/// If it is the last tab in the workspace, close the workspace.
/// If it is the last workspace, exit the app.
fn close_focused_pane(
    state: &mut AppState,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) {
    let tab = active_tab_mut(state);
    let focused_id = tab.layout.focused();
    // Drop the pane (this sends SIGHUP to the PTY child).
    tab.panes.remove(&focused_id);
    buffers.lock().unwrap().remove(&focused_id);
    unregister_pane_from_daemon(focused_id);

    match active_tab(state).layout.close(focused_id) {
        Some(new_layout) => {
            active_tab_mut(state).layout = new_layout;
            resize_all_panes(state);
            state.window.request_redraw();
        }
        None => {
            // Last pane in the current tab — close the tab.
            let ws = active_ws_mut(state);
            if ws.tabs.len() == 1 {
                // Last tab in this workspace — close the workspace.
                if state.workspaces.len() == 1 {
                    // Last workspace — quit.
                    event_loop.exit();
                } else {
                    let removed_idx = state.active_workspace;
                    state.workspaces.remove(removed_idx);
                    if removed_idx < state.workspace_infos.len() {
                        state.workspace_infos.remove(removed_idx);
                    }
                    if removed_idx < state.agent_infos.len() {
                        state.agent_infos.remove(removed_idx);
                    }
                    cleanup_session_to_workspace(state, removed_idx);
                    if state.active_workspace >= state.workspaces.len() {
                        state.active_workspace = state.workspaces.len() - 1;
                    }
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
            } else {
                let tab_idx = ws.active_tab;
                ws.tabs.remove(tab_idx);
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len() - 1;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
        }
    }
}

/// Determine the cursor icon for a position within the tab bar.
fn tab_bar_cursor(state: &AppState, mx: f32, _my: f32) -> CursorIcon {
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let local_cx = mx - sidebar_w;
    if local_cx < 0.0 {
        return CursorIcon::Default;
    }

    let cell_w = state.renderer.cell_size().width;
    let max_tab_w = state.config.tab_bar.max_width;
    let ws = &state.workspaces[state.active_workspace];
    let mut tab_x: f32 = 0.0;

    for tab in ws.tabs.iter() {
        let tab_w = compute_tab_width(&tab.display_title, cell_w, max_tab_w, state.config.tab_bar.min_tab_width);
        if local_cx >= tab_x && local_cx < tab_x + tab_w {
            // Check if over close button (rightmost 1.5 cells)
            let close_start = tab_x + tab_w - 1.5 * cell_w;
            if local_cx >= close_start {
                return CursorIcon::Pointer;
            }
            return CursorIcon::Default;
        }
        tab_x += tab_w;
    }

    // Over new-tab button
    if local_cx >= tab_x && local_cx < tab_x + state.config.tab_bar.new_tab_button_width {
        return CursorIcon::Pointer;
    }

    CursorIcon::Default
}

/// Result of clicking in the tab bar.
enum TabBarClickResult {
    /// Clicked on a tab body — switch to it (and potentially start drag).
    Tab(usize),
    /// Clicked the close button on a tab.
    CloseTab(usize),
    /// Clicked the `+` new-tab button.
    NewTab,
    /// Click didn't hit anything meaningful.
    None,
}

/// Compute the pixel width of a single tab given its display title.
fn compute_tab_width(title: &str, cell_w: f32, max_width: f32, min_width: f32) -> f32 {
    // Text width + left padding (1 cell) + right padding (1 cell) + close button area (1.5 cells).
    let text_width = title.len() as f32 * cell_w + 3.5 * cell_w;
    text_width.clamp(min_width, max_width)
}

/// Truncate a string to at most `max_chars` characters, appending an
/// ellipsis if truncation occurs.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('\u{2026}'); // ellipsis
        out
    }
}

/// Handle a click in the tab bar area. Determine which tab was clicked and switch to it.
/// Returns a `TabBarClickResult` describing what was clicked.
fn handle_tab_bar_click(state: &mut AppState) -> TabBarClickResult {
    let cx = state.cursor_pos.0 as f32;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let local_cx = cx - sidebar_w;
    if local_cx < 0.0 {
        return TabBarClickResult::None;
    }
    let cell_w = state.renderer.cell_size().width;
    let max_tab_w = state.config.tab_bar.max_width;

    let ws = active_ws(state);
    let mut tab_x: f32 = 0.0;
    for (i, tab) in ws.tabs.iter().enumerate() {
        let tab_w = compute_tab_width(&tab.display_title, cell_w, max_tab_w, state.config.tab_bar.min_tab_width);
        if local_cx >= tab_x && local_cx < tab_x + tab_w {
            // Check if click is on the close button (rightmost 1.5 cells of the tab).
            let close_zone_start = tab_x + tab_w - 1.5 * cell_w;
            if local_cx >= close_zone_start {
                return TabBarClickResult::CloseTab(i);
            }
            // Switch to the clicked tab.
            let ws = active_ws_mut(state);
            if ws.active_tab != i {
                ws.active_tab = i;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            return TabBarClickResult::Tab(i);
        }
        tab_x += tab_w;
    }

    // Check if click is on the `+` new-tab button (after all tabs).
    if local_cx >= tab_x && local_cx < tab_x + state.config.tab_bar.new_tab_button_width {
        return TabBarClickResult::NewTab;
    }

    TabBarClickResult::None
}

/// Handle a click in the sidebar area. Determine which workspace was clicked.
/// The new sidebar layout has:
///   - config top_padding
///   - Each workspace: name + cwd + git + ports + allow flow lines + entry_gap
///   - Separator line (1px + 8px padding each side)
///   - "New Workspace" button
fn handle_sidebar_click(state: &mut AppState) -> Option<Action> {
    let cy = state.cursor_pos.1 as f32;
    let cell_h = state.renderer.cell_size().height;
    let sc = &state.config.sidebar;
    let top_pad = sc.top_padding;
    let entry_gap = sc.entry_gap;
    let info_line_gap = sc.info_line_gap;

    let mut entry_y = top_pad;
    for (i, _ws) in state.workspaces.iter().enumerate() {
        let info = state.workspace_infos.get(i);
        let is_active = i == state.active_workspace;
        let has_allow_pending = state.allow_flow.has_pending_for_workspace(i);

        // Name line
        let mut entry_h = cell_h;
        // CWD line
        let ws_cwd = {
            let ws = &state.workspaces[i];
            let tab = &ws.tabs[ws.active_tab];
            let fid = tab.layout.focused();
            tab.panes.get(&fid)
                .map(|p| p.terminal.osc.cwd.clone())
                .unwrap_or_default()
        };
        if !ws_cwd.is_empty() {
            entry_h += info_line_gap + cell_h;
        }
        // Git info line (always present if branch known)
        if let Some(info) = info {
            if info.git_branch.is_some() {
                entry_h += info_line_gap + cell_h;
            }
            // Ports line
            if !info.ports.is_empty() {
                entry_h += info_line_gap + cell_h;
            }
        }
        // Allow Flow lines.
        if has_allow_pending {
            if is_active {
                let pending = state.allow_flow.pending_for_workspace(i);
                let shown = pending.len().min(3);
                entry_h += info_line_gap;
                entry_h += (shown as f32) * (cell_h * 3.0 + info_line_gap);
                if pending.len() > 3 {
                    entry_h += cell_h;
                }
            } else {
                entry_h += info_line_gap + cell_h;
            }
        }
        // Agent status lines.
        let has_agent = state.config.sidebar.agent_status_enabled
            && i < state.agent_infos.len()
            && state.agent_infos[i].active;
        if has_agent {
            entry_h += info_line_gap + cell_h; // agent status line
            if !state.agent_infos[i].summary.is_empty() {
                entry_h += info_line_gap + cell_h; // summary line
            }
            if state.agent_infos[i].subagent_count > 0 {
                entry_h += info_line_gap + cell_h; // subagent line
            }
        }
        entry_h += entry_gap; // gap between entries

        if cy >= entry_y && cy < entry_y + entry_h {
            if state.active_workspace != i {
                state.active_workspace = i;
                if i < state.workspace_infos.len() {
                    state.workspace_infos[i].has_unread = false;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            return None;
        }
        entry_y += entry_h;
    }

    // Check "+ New Workspace" button (below separator).
    // Separator: 8px + 1px + 8px = 17px
    entry_y += 17.0;
    if cy >= entry_y && cy < entry_y + cell_h {
        return Some(Action::NewWorkspace);
    }

    // --- Sessions Summary click handling ---
    // Mirror the layout calculation from render_sidebar to find session entry positions.
    let phys_h = state.window.inner_size().height as f32;

    // Collect active agent sessions (same logic as render_sidebar).
    let mut session_ws_indices: Vec<usize> = Vec::new();
    for wi in 0..state.workspaces.len() {
        if wi >= state.agent_infos.len() {
            break;
        }
        if state.agent_infos[wi].active {
            session_ws_indices.push(wi);
        }
    }

    if !session_ws_indices.is_empty() {
        let session_line_h = cell_h;
        let session_gap = info_line_gap;
        let session_pad_y = 4.0;
        let per_session_h = session_line_h * 3.0 + session_gap * 2.0 + session_pad_y * 2.0;
        let header_h = session_line_h + session_gap * 2.0;
        let session_entry_gap = 6.0;
        let sessions_total_h = header_h
            + session_ws_indices.len() as f32 * per_session_h
            + (session_ws_indices.len().saturating_sub(1)) as f32 * session_entry_gap;
        let sessions_sep_h = 1.0 + 10.0 * 2.0;
        let bottom_pad = 8.0;

        let sessions_start_y = phys_h - sessions_total_h - sessions_sep_h - bottom_pad;
        let new_ws_y_end = entry_y + cell_h;
        let min_start_y = new_ws_y_end + 16.0;

        if sessions_start_y >= min_start_y && cy >= sessions_start_y {
            let mut sy = sessions_start_y + sessions_sep_h + header_h;

            for &wi in &session_ws_indices {
                let session_end = sy + per_session_h + session_entry_gap;
                if cy >= sy && cy < session_end {
                    // Switch to the workspace containing this session.
                    if state.active_workspace != wi {
                        state.active_workspace = wi;
                        if wi < state.workspace_infos.len() {
                            state.workspace_infos[wi].has_unread = false;
                        }
                        resize_all_panes(state);
                        update_window_title(state);
                        state.window.request_redraw();
                    }
                    return None;
                }
                sy = session_end;
            }
        }
    }

    None
}

/// Render the iTerm2-inspired tab bar.
fn render_tab_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32) {
    // --- Color palette from config ---
    let tc = &state.config.tab_bar;
    let tab_bar_bg = color_or(&tc.bg, [0.10, 0.10, 0.12, 1.0]);
    let active_tab_bg = color_or(&tc.active_tab_bg, [0.18, 0.18, 0.22, 1.0]);
    let active_fg = color_or(&tc.active_tab_fg, [0.95, 0.95, 0.97, 1.0]);
    let inactive_fg = color_or(&tc.inactive_tab_fg, [0.55, 0.55, 0.60, 1.0]);
    let accent_color = color_or(&tc.accent_color, [0.30, 0.55, 1.0, 1.0]);
    let separator_color = color_or(&tc.separator_color, [0.22, 0.22, 0.25, 1.0]);
    let close_fg = color_or(&tc.close_button_fg, [0.50, 0.50, 0.55, 1.0]);
    let new_tab_fg = color_or(&tc.new_button_fg, [0.50, 0.50, 0.55, 1.0]);

    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;
    let max_tab_w = state.config.tab_bar.max_width;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let bar_x = sidebar_w as u32;
    let bar_w = (phys_w - sidebar_w).max(0.0) as u32;
    let bar_h_f = state.config.tab_bar.height;
    let bar_h = bar_h_f as u32;
    let accent_h = state.config.tab_bar.accent_height;
    let tab_pad_x = state.config.tab_bar.padding_x;
    let tab_pad_y = state.config.tab_bar.padding_y;

    // Draw tab bar background.
    state.renderer.submit_separator(view, bar_x, 0, bar_w, bar_h, tab_bar_bg);

    // Draw bottom border if enabled.
    if state.config.tab_bar.bottom_border {
        let border_color = color_or(&state.config.tab_bar.bottom_border_color, [0.16, 0.16, 0.20, 1.0]);
        state.renderer.submit_separator(view, bar_x, bar_h.saturating_sub(1), bar_w, 1, border_color);
    }

    // Draw each tab in the current workspace.
    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let active_tab_idx = ws.active_tab;
    let num_tabs = ws.tabs.len();
    let mut tab_x: f32 = sidebar_w;
    let text_y = (bar_h_f - cell_h) / 2.0;

    for (i, tab) in ws.tabs.iter().enumerate() {
        let tab_w = compute_tab_width(&tab.display_title, cell_w, max_tab_w, state.config.tab_bar.min_tab_width);
        let is_active = i == active_tab_idx;

        // Draw tab background (active tab is brighter).
        if is_active {
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                0,
                tab_w as u32,
                bar_h,
                active_tab_bg,
            );

            // Draw accent-colored bottom border (2px) for active tab.
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                bar_h - accent_h,
                tab_w as u32,
                accent_h,
                accent_color,
            );
        }

        // Draw tab title text.
        let fg = if is_active { active_fg } else { inactive_fg };
        let bg = if is_active { active_tab_bg } else { tab_bar_bg };

        // Indicator dot for active tab.
        let dot_offset = if is_active {
            let dot_str = "\u{25CF} "; // ● with space
            state.renderer.render_text(
                view,
                dot_str,
                tab_x + tab_pad_x,
                text_y.max(0.0),
                accent_color,
                bg,
            );
            cell_w * 2.0 // dot + space width
        } else {
            0.0
        };

        // Title text.
        let text_x = tab_x + tab_pad_x + dot_offset;
        // Truncate title if it won't fit (leave room for close button).
        let avail_chars =
            ((tab_w - 3.5 * cell_w - dot_offset) / cell_w).max(1.0) as usize;
        let display: String = if tab.display_title.len() > avail_chars {
            let mut s: String = tab.display_title.chars().take(avail_chars.saturating_sub(1)).collect();
            s.push('\u{2026}'); // ellipsis
            s
        } else {
            tab.display_title.clone()
        };
        state.renderer.render_text(view, &display, text_x, text_y.max(0.0), fg, bg);

        // Close button: show `\u{00d7}` (always for active tab, area is always clickable).
        if is_active || num_tabs > 1 {
            let close_x = tab_x + tab_w - tab_pad_x - cell_w;
            let close_char = "\u{00D7}"; // ×
            state.renderer.render_text(
                view,
                close_char,
                close_x,
                text_y.max(0.0),
                if is_active { close_fg } else { inactive_fg },
                bg,
            );
        }

        tab_x += tab_w;

        // Draw vertical separator between tabs (1px).
        if i < num_tabs - 1 {
            let sep_margin = tab_pad_y as u32;
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                sep_margin,
                1,
                bar_h.saturating_sub(sep_margin * 2).max(1),
                separator_color,
            );
        }
    }

    // Draw `+` new-tab button after all tabs.
    let plus_x = tab_x + (state.config.tab_bar.new_tab_button_width - cell_w) / 2.0;
    state.renderer.render_text(
        view,
        "+",
        plus_x,
        text_y.max(0.0),
        new_tab_fg,
        tab_bar_bg,
    );
}

/// Render the sidebar showing workspaces with rich information.
/// Inspired by cmux terminal (minimal vertical tabs) and Arc browser (colorful dots).
fn render_sidebar(state: &mut AppState, view: &wgpu::TextureView, phys_h: f32) {
    // --- Color palette from config ---
    let sc = &state.config.sidebar;
    let sidebar_bg = color_or(&sc.bg, [0.051, 0.051, 0.071, 1.0]);
    let active_entry_bg = color_or(&sc.active_entry_bg, [0.118, 0.118, 0.165, 1.0]);
    let active_fg = color_or(&sc.active_fg, [0.95, 0.95, 0.97, 1.0]);
    let inactive_fg = color_or(&sc.inactive_fg, [0.627, 0.627, 0.675, 1.0]);
    let dim_fg = color_or(&sc.dim_fg, [0.467, 0.467, 0.541, 1.0]);
    let inactive_dot_color = dim_fg;
    let git_branch_fg = color_or(&sc.git_branch_fg, [0.35, 0.70, 0.85, 1.0]);
    let separator_color = color_or(&sc.separator_color, [0.20, 0.20, 0.22, 1.0]);
    let notification_dot = color_or(&sc.notification_dot, [1.0, 0.58, 0.26, 1.0]);
    let yellow_fg = color_or(&sc.git_dirty_color, [0.8, 0.7, 0.3, 1.0]);
    // Allow Flow accent colors.
    let allow_accent_color = color_or(&sc.allow_accent_color, [0.31, 0.76, 1.0, 1.0]);
    let allow_hint_fg = color_or(&sc.allow_hint_fg, [0.49, 0.78, 1.0, 1.0]);
    // Agent status colors.
    let agent_status_enabled = sc.agent_status_enabled;
    let agent_indicator_style = sc.agent_indicator_style.clone();
    let agent_pulse_speed = sc.agent_pulse_speed;
    let agent_active_color = color_or(&sc.agent_active_color, [0.655, 0.545, 0.98, 1.0]);
    let agent_idle_color = color_or(&sc.agent_idle_color, [0.984, 0.749, 0.141, 1.0]);

    let cell_h = state.renderer.cell_size().height;
    let cell_w = state.renderer.cell_size().width;
    let sidebar_w = state.sidebar_width;

    // Spacing from config.
    let top_pad = sc.top_padding;
    let side_pad = sc.side_padding;
    let entry_gap = sc.entry_gap;
    let info_line_gap = sc.info_line_gap;

    // --- Draw sidebar background (full height) ---
    state.renderer.submit_separator(view, 0, 0, sidebar_w as u32, phys_h as u32, sidebar_bg);

    // --- Refresh workspace info ---
    while state.workspace_infos.len() < state.workspaces.len() {
        state.workspace_infos.push(WorkspaceInfo::new());
    }
    // Keep agent_infos in sync.
    while state.agent_infos.len() < state.workspaces.len() {
        state.agent_infos.push(AgentSessionInfo::default());
    }

    // Refresh all workspaces (active every 5s, inactive every 30s).
    for wi in 0..state.workspaces.len() {
        if wi >= state.workspace_infos.len() {
            break;
        }
        let elapsed = state.workspace_infos[wi].last_updated.elapsed();
        let refresh_interval = if wi == state.active_workspace { 5 } else { 30 };
        if elapsed.as_secs() >= refresh_interval || state.workspace_infos[wi].name.is_empty() {
            let cwd = {
                let ws = &state.workspaces[wi];
                let tab = &ws.tabs[ws.active_tab];
                let focused_id = tab.layout.focused();
                tab.panes
                    .get(&focused_id)
                    .map(|p| p.terminal.osc.cwd.clone())
                    .unwrap_or_default()
            };
            refresh_workspace_info(&mut state.workspace_infos[wi], &cwd);
        }
    }

    // --- Draw workspace entries ---
    // Layout: [side_pad] [dot] [gap] [text ...] [side_pad]
    let dot_area = cell_w * 1.5; // dot character + small gap
    let text_left = side_pad + dot_area;
    let max_chars = ((sidebar_w - text_left - side_pad) / cell_w).max(1.0) as usize;
    let num_workspaces = state.workspaces.len();
    let mut entry_y = top_pad;

    for i in 0..num_workspaces {
        // Bail if not even the name line fits.
        if entry_y + cell_h > phys_h {
            break;
        }

        let is_active = i == state.active_workspace;
        let info = state.workspace_infos.get(i);
        let ws_color = WORKSPACE_COLORS[i % WORKSPACE_COLORS.len()];

        // Get additional workspace info: cwd and pane count.
        let ws_cwd = {
            let ws = &state.workspaces[i];
            let tab = &ws.tabs[ws.active_tab];
            let fid = tab.layout.focused();
            tab.panes.get(&fid)
                .map(|p| p.terminal.osc.cwd.clone())
                .unwrap_or_default()
        };
        let ws_pane_count: usize = state.workspaces[i].tabs.iter()
            .map(|t| t.panes.len())
            .sum();

        // Calculate entry height for active highlight background.
        let has_git = info.map_or(false, |inf| inf.git_branch.is_some());
        let has_ports = info.map_or(false, |inf| !inf.ports.is_empty());
        let has_cwd = !ws_cwd.is_empty();
        let has_allow_pending = state.allow_flow.has_pending_for_workspace(i);
        let mut content_h = cell_h; // name line
        if has_cwd {
            content_h += info_line_gap + cell_h; // cwd line
        }
        if has_git {
            content_h += info_line_gap + cell_h; // git info line
        }
        if has_ports {
            content_h += info_line_gap + cell_h; // ports line
        }
        // Allow Flow lines: expanded for active workspace, collapsed for inactive.
        if has_allow_pending {
            if is_active {
                // Active workspace: up to 3 expanded requests, each has
                // tool+action line + detail line + key hints line = 3 lines.
                let pending = state.allow_flow.pending_for_workspace(i);
                let shown = pending.len().min(3);
                content_h += info_line_gap; // gap before first request
                content_h += (shown as f32) * (cell_h * 3.0 + info_line_gap);
                if pending.len() > 3 {
                    content_h += cell_h; // "+N more..." line
                }
            } else {
                // Inactive workspace: single collapsed badge line.
                content_h += info_line_gap + cell_h;
            }
        }
        // Agent status line (below Allow Flow / ports / git).
        let has_agent = agent_status_enabled
            && i < state.agent_infos.len()
            && state.agent_infos[i].active;
        if has_agent {
            content_h += info_line_gap + cell_h; // agent status line (compact)
            // Summary line if non-empty.
            if !state.agent_infos[i].summary.is_empty() {
                content_h += info_line_gap + cell_h;
            }
            // Subagent count is now merged into the status line, no extra line needed.
        }

        // Clamp content height to remaining space so we don't render past the sidebar.
        let content_h = content_h.min(phys_h - entry_y);

        // --- Allow Flow accent stripe (rendered BEFORE entry background) ---
        // A 3px colored stripe on the left edge when pending requests exist.
        let entry_pad_y = entry_gap / 2.0; // vertical padding around highlight
        if has_allow_pending {
            state.renderer.submit_separator(
                view,
                0,
                (entry_y - entry_pad_y).max(0.0) as u32,
                3, // 3px wide accent stripe
                (content_h + entry_pad_y * 2.0) as u32,
                allow_accent_color,
            );
        }

        // --- Active workspace: highlight background (full sidebar width) ---
        let bg = if is_active { active_entry_bg } else { sidebar_bg };
        if is_active {
            // Start highlight after the accent stripe area to keep it visible.
            let accent_w: u32 = 3;
            let bg_x = if has_allow_pending { accent_w } else { accent_w };
            state.renderer.submit_separator(
                view,
                bg_x,
                (entry_y - entry_pad_y).max(0.0) as u32,
                (sidebar_w as u32).saturating_sub(bg_x),
                (content_h + entry_pad_y * 2.0) as u32,
                active_entry_bg,
            );
            // Workspace color accent bar on the left edge (3px) for active entry.
            if !has_allow_pending {
                state.renderer.submit_separator(
                    view,
                    0,
                    (entry_y - entry_pad_y).max(0.0) as u32,
                    accent_w,
                    (content_h + entry_pad_y * 2.0) as u32,
                    ws_color,
                );
            }
        }

        // --- Workspace indicator dot ---
        // Active: filled colored circle ●, Inactive: outline circle ○
        let dot_char = if has_agent { "\u{25CF}" } else if is_active { "\u{25CF}" } else { "\u{25CB}" }; // ● or ○
        let mut dot_color = if is_active {
            ws_color
        } else if info.map_or(false, |inf| inf.has_unread) {
            notification_dot
        } else {
            inactive_dot_color
        };
        // Agent pulse animation on dot.
        if has_agent && agent_indicator_style == "pulse" {
            let elapsed = state.app_start_time.elapsed().as_secs_f32();
            let alpha = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * elapsed / agent_pulse_speed.max(0.1)).sin();
            let base = match state.agent_infos[i].state {
                AgentState::WaitingForPermission => agent_idle_color,
                _ => agent_active_color,
            };
            dot_color = [base[0], base[1], base[2], alpha];
        } else if has_agent && agent_indicator_style == "color" {
            dot_color = match state.agent_infos[i].state {
                AgentState::WaitingForPermission => agent_idle_color,
                _ => agent_active_color,
            };
        }
        state.renderer.render_text(view, dot_char, side_pad, entry_y, dot_color, bg);

        // --- Notification dot for unread activity (small bright dot next to circle) ---
        if !is_active {
            if let Some(inf) = info {
                if inf.has_unread {
                    // Render a small bright dot indicator after the workspace dot
                    state.renderer.render_text(
                        view,
                        "\u{2022}", // •
                        side_pad + cell_w * 1.2,
                        entry_y - cell_h * 0.3,
                        notification_dot,
                        bg,
                    );
                }
            }
        }

        // --- Workspace name (project name from CWD basename, no "Workspace N" label) ---
        let display_name = if let Some(info) = info {
            if !info.name.is_empty() {
                info.name.clone()
            } else {
                // Fallback: use CWD basename or a minimal dot label.
                if !ws_cwd.is_empty() {
                    std::path::Path::new(&ws_cwd)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("~")
                        .to_string()
                } else {
                    "~".to_string()
                }
            }
        } else if !ws_cwd.is_empty() {
            std::path::Path::new(&ws_cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("~")
                .to_string()
        } else {
            "~".to_string()
        };

        // Tab/pane count badge.
        let tab_count = state.workspaces[i].tabs.len();
        let badge = if tab_count > 1 || ws_pane_count > 1 {
            let pane_str = if ws_pane_count > 1 { format!(" \u{25A8}{ws_pane_count}") } else { String::new() };
            format!(" [{tab_count}\u{25AB}{pane_str}]")
        } else {
            String::new()
        };

        let name_label = format!("{display_name}{badge}");
        let name_display: String = name_label.chars().take(max_chars).collect();
        let name_fg = if is_active { active_fg } else { inactive_fg };
        state.renderer.render_text(view, &name_display, text_left, entry_y, name_fg, bg);

        let mut line_y = entry_y + cell_h;

        // --- CWD line (below name) ---
        if has_cwd {
            line_y += info_line_gap;
            let cwd_short = if let Ok(home) = std::env::var("HOME") {
                if ws_cwd.starts_with(&home) {
                    format!("~{}", &ws_cwd[home.len()..])
                } else {
                    ws_cwd.clone()
                }
            } else {
                ws_cwd.clone()
            };
            // Show only last 2 path components
            let cwd_display: String = {
                let parts: Vec<&str> = cwd_short.rsplitn(3, '/').collect();
                if parts.len() >= 2 {
                    format!("\u{1F4C1} {}/{}", parts[1], parts[0])
                } else {
                    format!("\u{1F4C1} {cwd_short}")
                }
            };
            let info_indent = text_left + cell_w * 0.5;
            let cwd_trimmed: String = cwd_display.chars().take(max_chars.saturating_sub(1)).collect();
            state.renderer.render_text(view, &cwd_trimmed, info_indent, line_y, dim_fg, bg);
            line_y += cell_h;
        }

        // --- Git info line (below cwd) ---
        if let Some(info) = info {
            if let Some(ref branch) = info.git_branch {
                line_y += info_line_gap;

                // Build git status string: branch ⇡N ⇣N !N ?N
                let mut git_parts = format!("\u{E0A0} {branch}"); // git branch icon
                if info.git_ahead > 0 {
                    git_parts.push_str(&format!(" \u{21E1}{}", info.git_ahead));
                }
                if info.git_behind > 0 {
                    git_parts.push_str(&format!(" \u{21E3}{}", info.git_behind));
                }
                if info.git_dirty > 0 {
                    git_parts.push_str(&format!(" !{}", info.git_dirty));
                }
                if info.git_untracked > 0 {
                    git_parts.push_str(&format!(" ?{}", info.git_untracked));
                }

                let info_indent = text_left + cell_w * 0.5;
                let git_display: String = git_parts.chars().take(max_chars.saturating_sub(1)).collect();
                // Branch in cyan/blue, status indicators dimmed for inactive
                let git_fg = if is_active {
                    if info.git_dirty > 0 || info.git_untracked > 0 {
                        yellow_fg
                    } else {
                        git_branch_fg
                    }
                } else {
                    dim_fg
                };
                state.renderer.render_text(view, &git_display, info_indent, line_y, git_fg, bg);
                line_y += cell_h;
            }

            // --- Ports line (below git) ---
            if !info.ports.is_empty() {
                line_y += info_line_gap;
                let ports_str: String = format!("\u{F0AC} {}",  // globe icon
                    info.ports.iter()
                        .map(|p| format!(":{p}"))
                        .collect::<Vec<_>>()
                        .join(" "));
                let info_indent = text_left + cell_w * 0.5;
                let ports_display: String = ports_str.chars().take(max_chars.saturating_sub(1)).collect();
                state.renderer.render_text(view, &ports_display, info_indent, line_y, dim_fg, bg);
                line_y += cell_h;
            }
        }

        // --- Inline Allow Flow requests (below ports) ---
        if has_allow_pending {
            if is_active {
                // Active workspace: expanded view with full request details.
                let pending = state.allow_flow.pending_for_workspace(i);
                for (ri, req) in pending.iter().take(3).enumerate() {
                    line_y += info_line_gap;
                    // Tool + action line (lightning bolt icon).
                    let tool_line = format!(
                        "\u{26A1} {}: {}",
                        req.tool_name,
                        truncate_str(&req.action, max_chars.saturating_sub(4))
                    );
                    state.renderer.render_text(
                        view, &tool_line, text_left, line_y, allow_hint_fg, bg,
                    );
                    line_y += cell_h;

                    // Detail line (quoted, slightly dimmed).
                    let detail_line = format!(
                        "  \"{}\"",
                        truncate_str(&req.detail, max_chars.saturating_sub(4))
                    );
                    state.renderer.render_text(
                        view, &detail_line, text_left, line_y, dim_fg, bg,
                    );
                    line_y += cell_h;

                    // Key hints line (only on the first request to avoid clutter,
                    // unless there's only one request).
                    if ri == 0 || pending.len() == 1 {
                        let hint = if pending.len() > 1 {
                            "  y/n one  Y/N all  A always"
                        } else {
                            "  Y Allow  N Deny  A Always"
                        };
                        let hint_fg_col = [0.45, 0.55, 0.65, 1.0];
                        state.renderer.render_text(
                            view, hint, text_left, line_y, hint_fg_col, bg,
                        );
                    }
                    line_y += cell_h;
                }
                if pending.len() > 3 {
                    let more = format!("  +{} more...", pending.len() - 3);
                    state.renderer.render_text(
                        view, &more, text_left, line_y, dim_fg, bg,
                    );
                    // line_y not used further; entry_y advances by content_h.
                }
            } else {
                // Inactive workspace: collapsed badge.
                line_y += info_line_gap;
                let count = state.allow_flow.pending_count_for_workspace(i);
                let badge = format!("\u{26A1} {} pending", count);
                state.renderer.render_text(
                    view, &badge, text_left, line_y, allow_accent_color, bg,
                );
                // line_y not used further; entry_y advances by content_h.
            }
        }

        // --- Agent status lines (below Allow Flow / ports / git) ---
        if has_agent {
            let agent = &state.agent_infos[i];
            line_y += info_line_gap;
            // Compact agent status: icon + short state label.
            let (agent_icon, state_label) = match agent.state {
                AgentState::Running => ("\u{26A1}", "running"),    // ⚡ running
                AgentState::WaitingForPermission => ("\u{23F3}", "waiting"), // ⏳ waiting
                AgentState::Idle => ("\u{25CB}", "idle"),          // ○ idle
                AgentState::Inactive => ("\u{25CB}", "idle"),      // ○ idle
            };
            let agent_line = if agent.subagent_count > 0 {
                format!("{} {} (+{})", agent_icon, state_label, agent.subagent_count)
            } else {
                format!("{} {}", agent_icon, state_label)
            };
            let agent_fg = match agent.state {
                AgentState::Running => agent_active_color,
                AgentState::Idle | AgentState::Inactive => dim_fg,
                AgentState::WaitingForPermission => agent_idle_color,
            };
            let info_indent = text_left + cell_w * 0.5;
            let agent_display: String = agent_line.chars().take(max_chars).collect();
            state.renderer.render_text(view, &agent_display, info_indent, line_y, agent_fg, bg);
            line_y += cell_h;

            // Summary line (truncated).
            if !agent.summary.is_empty() {
                line_y += info_line_gap;
                let summary_display: String = agent.summary.chars().take(max_chars.saturating_sub(1)).collect();
                state.renderer.render_text(view, &summary_display, info_indent, line_y, dim_fg, bg);
                line_y += cell_h;
            }
            let _ = line_y; // suppress unused warning
        }

        entry_y += content_h + entry_gap;

        // --- Subtle separator line between entries ---
        if i < num_workspaces - 1 {
            let sep_line_y = (entry_y - entry_gap / 2.0) as u32;
            let sep_dim_color = [separator_color[0], separator_color[1], separator_color[2], 0.4];
            if (sep_line_y as f32) + 1.0 < phys_h {
                state.renderer.submit_separator(
                    view,
                    (side_pad + dot_area * 0.5) as u32,
                    sep_line_y,
                    (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                    1,
                    sep_dim_color,
                );
            }
        }
    }

    // --- Separator line ---
    let sep_y = entry_y + 8.0;
    if sep_y + 1.0 < phys_h {
        state.renderer.submit_separator(
            view,
            side_pad as u32,
            sep_y as u32,
            (sidebar_w - 2.0 * side_pad) as u32,
            1,
            separator_color,
        );
    }

    // --- "New Workspace" button ---
    let new_ws_y = sep_y + 8.0 + 1.0; // 8px below separator
    if new_ws_y + cell_h <= phys_h {
        let new_ws_label = "+ New Workspace";
        state.renderer.render_text(view, new_ws_label, side_pad, new_ws_y, dim_fg, sidebar_bg);
    }

    // --- Sessions Summary (bottom of sidebar) ---
    // Collect all active agent sessions across workspaces.
    let mut session_entries: Vec<(usize, String, &str, usize)> = Vec::new();
    for wi in 0..state.workspaces.len() {
        if wi >= state.agent_infos.len() {
            break;
        }
        let agent = &state.agent_infos[wi];
        if !agent.active {
            continue;
        }
        let title = {
            let ws_info = state.workspace_infos.get(wi);
            let name = ws_info.map(|i| i.name.as_str()).unwrap_or("");
            if !name.is_empty() {
                name.to_string()
            } else {
                let ws = &state.workspaces[wi];
                let tab = &ws.tabs[ws.active_tab];
                if !tab.display_title.is_empty() {
                    tab.display_title.clone()
                } else {
                    format!("Workspace {}", wi + 1)
                }
            }
        };
        let state_label = match agent.state {
            AgentState::Running => "running",
            AgentState::WaitingForPermission => "wait user action",
            AgentState::Idle => "pause",
            AgentState::Inactive => "pause",
        };
        session_entries.push((wi, title, state_label, agent.subagent_count));
    }

    if !session_entries.is_empty() {
        let session_line_h = cell_h;
        let session_gap = info_line_gap;
        // Each session: title + state + subagent (3 lines, 2 inner gaps) + padding.
        let session_pad_y = 4.0; // vertical padding inside each session card
        let per_session_h = session_line_h * 3.0 + session_gap * 2.0 + session_pad_y * 2.0;
        let header_h = session_line_h + session_gap * 2.0;
        let session_entry_gap = 6.0; // gap between session cards
        let sessions_total_h = header_h
            + session_entries.len() as f32 * per_session_h
            + (session_entries.len().saturating_sub(1)) as f32 * session_entry_gap;
        let sessions_sep_h = 1.0 + 10.0 * 2.0; // 10px padding + 1px line + 10px padding
        let bottom_pad = 8.0;

        let sessions_start_y = phys_h - sessions_total_h - sessions_sep_h - bottom_pad;
        let min_start_y = new_ws_y + cell_h + 16.0;
        if sessions_start_y >= min_start_y {
            // --- Gradient-like separator (double thin lines with gap) ---
            let sep_sess_y = sessions_start_y;
            let sep_upper_color = [separator_color[0], separator_color[1], separator_color[2], 0.25];
            let sep_lower_color = [separator_color[0], separator_color[1], separator_color[2], 0.5];
            state.renderer.submit_separator(
                view,
                (side_pad + dot_area * 0.5) as u32,
                (sep_sess_y + 9.0) as u32,
                (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                1,
                sep_upper_color,
            );
            state.renderer.submit_separator(
                view,
                (side_pad + dot_area * 0.5) as u32,
                (sep_sess_y + 11.0) as u32,
                (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                1,
                sep_lower_color,
            );

            // --- Header: icon + "Sessions" + count badge ---
            let header_y = sep_sess_y + sessions_sep_h;
            // Subtle header icon.
            let header_icon_fg = agent_active_color;
            state.renderer.render_text(view, "\u{2630}", side_pad, header_y, header_icon_fg, sidebar_bg); // ☰
            let header_text = format!("Sessions ({})", session_entries.len());
            let header_fg = [active_fg[0] * 0.8, active_fg[1] * 0.8, active_fg[2] * 0.8, 0.9];
            state.renderer.render_text(view, &header_text, side_pad + cell_w * 2.0, header_y, header_fg, sidebar_bg);

            let mut sy = header_y + header_h;
            let info_indent = text_left + cell_w * 0.5;

            for (idx, (wi, title, state_label, subagent_count)) in session_entries.iter().enumerate() {
                if sy + per_session_h > phys_h - bottom_pad {
                    break;
                }

                let is_active_ws = *wi == state.active_workspace;
                let session_bg = if is_active_ws { active_entry_bg } else { sidebar_bg };

                // --- Card background for each session entry ---
                let card_bg = if is_active_ws {
                    active_entry_bg
                } else {
                    // Subtle card background slightly lighter than sidebar.
                    [sidebar_bg[0] + 0.02, sidebar_bg[1] + 0.02, sidebar_bg[2] + 0.025, 1.0]
                };
                let card_x: u32 = (side_pad * 0.5) as u32;
                let card_w = (sidebar_w - side_pad) as u32;
                state.renderer.submit_separator(
                    view,
                    card_x,
                    sy as u32,
                    card_w,
                    per_session_h as u32,
                    card_bg,
                );

                // --- Left accent bar (workspace color for active, dim for inactive) ---
                let accent_w: u32 = 3;
                let accent_color = if is_active_ws {
                    WORKSPACE_COLORS[*wi % WORKSPACE_COLORS.len()]
                } else {
                    let ws_col = WORKSPACE_COLORS[*wi % WORKSPACE_COLORS.len()];
                    [ws_col[0] * 0.5, ws_col[1] * 0.5, ws_col[2] * 0.5, 0.6]
                };
                state.renderer.submit_separator(
                    view,
                    card_x,
                    sy as u32,
                    accent_w,
                    per_session_h as u32,
                    accent_color,
                );

                let content_y = sy + session_pad_y;

                // --- Line 1: Workspace color dot (with pulse for running) + title ---
                let ws_col = WORKSPACE_COLORS[*wi % WORKSPACE_COLORS.len()];
                let mut dot_col = ws_col;
                // Pulse animation for running sessions (matches workspace dot behavior).
                if *state_label == "running" && agent_indicator_style == "pulse" {
                    let elapsed = state.app_start_time.elapsed().as_secs_f32();
                    let alpha = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * elapsed / agent_pulse_speed.max(0.1)).sin();
                    dot_col = [agent_active_color[0], agent_active_color[1], agent_active_color[2], alpha];
                } else if *state_label == "wait user action" {
                    dot_col = agent_idle_color;
                }
                state.renderer.render_text(view, "\u{25CF}", side_pad + 2.0, content_y, dot_col, card_bg); // ●
                let title_display: String = title.chars().take(max_chars.saturating_sub(1)).collect();
                let title_fg = if is_active_ws { active_fg } else { inactive_fg };
                state.renderer.render_text(view, &title_display, text_left, content_y, title_fg, card_bg);

                // --- Line 2: State with colored icon ---
                let line2_y = content_y + session_line_h + session_gap;
                let (state_icon, state_color) = match *state_label {
                    "running" => ("\u{25B6}", agent_active_color),       // ▶ purple
                    "wait user action" => ("\u{23F3}", agent_idle_color), // ⏳ orange
                    _ => ("\u{23F8}", dim_fg),                            // ⏸ dim
                };
                state.renderer.render_text(view, state_icon, info_indent, line2_y, state_color, card_bg);
                // State label in slightly dimmer version of state color.
                let label_fg = [state_color[0] * 0.85, state_color[1] * 0.85, state_color[2] * 0.85, state_color[3]];
                state.renderer.render_text(view, state_label, info_indent + cell_w * 2.5, line2_y, label_fg, card_bg);

                // --- Line 3: Subagent info ---
                let line3_y = line2_y + session_line_h + session_gap;
                if *subagent_count > 0 {
                    let sub_icon = "\u{2514}"; // └
                    let sub_text = format!("{} {} subagent{}", sub_icon, subagent_count,
                        if *subagent_count > 1 { "s" } else { "" });
                    let sub_fg = [dim_fg[0], dim_fg[1], dim_fg[2] + 0.08, dim_fg[3]]; // slightly blue tint
                    state.renderer.render_text(view, &sub_text, info_indent, line3_y, sub_fg, card_bg);
                } else {
                    let sub_text = "\u{2514} solo";
                    state.renderer.render_text(view, sub_text, info_indent, line3_y, dim_fg, card_bg);
                }

                sy += per_session_h + session_entry_gap;

                // Subtle separator between session cards (not after last one).
                if idx < session_entries.len() - 1 {
                    let card_sep_y = (sy - session_entry_gap / 2.0) as u32;
                    let card_sep_color = [separator_color[0], separator_color[1], separator_color[2], 0.2];
                    if (card_sep_y as f32) + 1.0 < phys_h {
                        state.renderer.submit_separator(
                            view,
                            (side_pad + dot_area) as u32,
                            card_sep_y,
                            (sidebar_w - side_pad * 2.0 - dot_area) as u32,
                            1,
                            card_sep_color,
                        );
                    }
                }
            }
        }
    }
}

/// Build the `StatusContext` for the current frame by collecting variable values.
fn build_status_context(state: &mut AppState) -> StatusContext {
    let cache = &mut state.status_cache;
    let (time, date) = cache.time_date();
    let time = time.to_string();
    let date = date.to_string();

    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let tab_idx = ws.active_tab;
    let tab = &ws.tabs[tab_idx];
    let focused_id = tab.layout.focused();

    // Extract user, host from focused pane's OSC 7 URI (file://user@host/path).
    // Fallback 1: detect SSH from child process tree (parse `ssh user@host` args).
    // Fallback 2: cached global $USER / gethostname.
    let focused_pane = tab.panes.get(&focused_id);
    let (user, host) = {
        let mut u = cache.user.clone();
        let mut h = cache.host.clone();
        let mut found = false;

        // Try OSC 7 URI first.
        if let Some(pane) = focused_pane {
            let uri = &pane.terminal.osc.cwd_uri;
            if let Some(rest) = uri.strip_prefix("file://") {
                if let Some(slash_idx) = rest.find('/') {
                    let authority = &rest[..slash_idx];
                    if !authority.is_empty() {
                        if let Some(at_idx) = authority.find('@') {
                            let pu = &authority[..at_idx];
                            let ph = &authority[at_idx + 1..];
                            if !pu.is_empty() { u = pu.to_string(); }
                            if !ph.is_empty() { h = ph.to_string(); }
                            found = true;
                        } else {
                            h = authority.to_string();
                            found = true;
                        }
                    }
                }
            }

            // If OSC 7 didn't provide host info, use cached SSH detection
            // (populated during git cache refresh, not every frame).
            if !found {
                let gc = &state.pane_git_cache;
                if !gc.ssh_host.is_empty() {
                    h = gc.ssh_host.clone();
                    if !gc.ssh_user.is_empty() {
                        u = gc.ssh_user.clone();
                    }
                }
            }
        }
        (u, h)
    };

    // Shell from focused pane's PTY shell command (basename).
    let shell = focused_pane
        .map(|p| {
            std::path::Path::new(&p.shell)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_else(|| cache.shell.clone());

    // CWD: prefer OSC 7, otherwise use cached value from lsof (updated every refresh).
    // Send current pane info to the background status collector (non-blocking).
    let osc_cwd = focused_pane
        .map(|p| p.terminal.osc.cwd.clone())
        .unwrap_or_default();
    let pty_pid = focused_pane.map(|p| p.pty.pid().as_raw()).unwrap_or(0);
    state.status_collector.update_request(pty_pid, &osc_cwd);

    // Read latest snapshot from background thread (non-blocking).
    let snap = state.status_collector.get();
    state.pane_git_cache.update_from_snapshot(&snap);

    let gc = &state.pane_git_cache;
    let cwd = if !osc_cwd.is_empty() { osc_cwd } else { gc.cwd.clone() };
    let cwd_short = if let Ok(home) = std::env::var("HOME") {
        if cwd.starts_with(&home) {
            format!("~{}", &cwd[home.len()..])
        } else {
            cwd.clone()
        }
    } else {
        cwd.clone()
    };
    let git_branch = gc.git_branch.clone();
    let git_worktree = gc.git_worktree.clone();
    let git_stash = if gc.git_stash > 0 {
        format!("{}", gc.git_stash)
    } else {
        String::new()
    };
    let git_ahead = format!("{}", gc.git_ahead);
    let git_behind = format!("{}", gc.git_behind);
    let git_dirty = format!("{}", gc.git_dirty);
    let git_untracked = format!("{}", gc.git_untracked);
    let git_status = {
        let mut parts = Vec::new();
        if gc.git_ahead > 0 {
            parts.push(format!("\u{21E1}{}", gc.git_ahead));
        }
        if gc.git_behind > 0 {
            parts.push(format!("\u{21E3}{}", gc.git_behind));
        }
        if gc.git_dirty > 0 {
            parts.push(format!("!{}", gc.git_dirty));
        }
        if gc.git_untracked > 0 {
            parts.push(format!("?{}", gc.git_untracked));
        }
        parts.join(" ")
    };

    // Ports from WorkspaceInfo.
    let info = state.workspace_infos.get(ws_idx);
    let ports = info
        .map(|i| {
            i.ports
                .iter()
                .map(|p| format!(":{p}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    // PID of the focused pane's PTY.
    let pid = focused_pane
        .map(|p| p.pty.pid().as_raw().to_string())
        .unwrap_or_default();

    // Pane size (cols x rows) of the focused pane.
    let pane_size = focused_pane
        .map(|p| {
            let g = p.terminal.grid();
            format!("{}x{}", g.cols(), g.rows())
        })
        .unwrap_or_default();

    let font_size = format!("{}", state.font_size as u32);

    let workspace = ws.name.clone();
    let workspace_index = format!("{}", ws_idx + 1);
    let tab_name = tab.display_title.clone();
    let tab_index = format!("{}", tab_idx + 1);

    let git_remote = gc.git_remote.clone();

    StatusContext {
        user,
        host,
        cwd,
        cwd_short,
        git_branch,
        git_status,
        git_remote,
        git_worktree,
        git_stash,
        git_ahead,
        git_behind,
        git_dirty,
        git_untracked,
        ports,
        shell,
        pid,
        pane_size,
        font_size,
        workspace,
        workspace_index,
        tab: tab_name,
        tab_index,
        time,
        date,
    }
}

/// Calculate the display width of a string in cell units (accounting for wide chars).
fn str_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(1)).sum()
}

/// Render the bottom status bar.
fn render_status_bar(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cfg = state.config.status_bar.clone();
    if !cfg.enabled {
        return;
    }

    let ctx = build_status_context(state);

    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    // Bar height: at least cell_h + padding.
    let bar_h = effective_status_bar_height(state);
    let bar_x = sidebar_w.floor();
    let bar_w = (phys_w - bar_x).floor();
    let bar_y = (phys_h - bar_h).floor();

    // Draw full status bar background.
    let status_bg = parse_hex_color(&cfg.background).unwrap_or([0.1, 0.1, 0.14, 1.0]);
    let bar_yi = bar_y as u32;
    let bar_hi = bar_h as u32;
    state.renderer.submit_separator(view, bar_x as u32, bar_yi, bar_w as u32, bar_hi, status_bg);

    // Draw top border if enabled.
    if cfg.top_border {
        let border_color = color_or(&cfg.top_border_color, [
            (status_bg[0] + 0.08).min(1.0), (status_bg[1] + 0.08).min(1.0),
            (status_bg[2] + 0.08).min(1.0), 1.0,
        ]);
        state.renderer.submit_separator(view, bar_x as u32, bar_yi, bar_w as u32, 1, border_color);
    }

    // Optically center text within the bar.
    // The cell includes descent space below the baseline, so mathematical center
    // looks too high. Shift down by ~half the descent to visually center the
    // cap-height region (where most text lives).
    let descent = state.renderer.cell_size().descent.abs();
    let optical_offset = (descent * 0.4).round();
    let text_y = (bar_y + (bar_h - cell_h) / 2.0 + optical_offset).floor();

    // Segment horizontal padding (each side).
    let seg_pad = cell_w;

    // --- Expand all segments and compute widths ---
    // Segment width = text width + padding on each side.
    let expand_segs = |segs: &[config::StatusSegment]| -> Vec<(String, [f32; 4], [f32; 4], f32, f32)> {
        segs.iter().filter_map(|seg| {
            let text = expand_status_variables(&seg.content, &ctx);
            if segment_is_empty(&text) { return None; }
            let fg = parse_hex_color(&seg.fg).unwrap_or([0.8, 0.8, 0.8, 1.0]);
            let bg = parse_hex_color(&seg.bg).unwrap_or(status_bg);
            let text_w = (str_display_width(&text) as f32 * cell_w).ceil();
            let seg_w = text_w + seg_pad * 2.0;
            Some((text, fg, bg, seg_w, text_w))
        }).collect()
    };

    let left_segs = expand_segs(&cfg.left);
    let right_segs = expand_segs(&cfg.right);

    // --- Render segments ---
    // Text is horizontally centered within each segment.
    let text_yi = text_y as u32;

    let render_seg = |state: &mut AppState, xi: u32, text: &str, fg: [f32; 4], bg: [f32; 4], seg_w: f32, text_w: f32| {
        let wi = seg_w as u32;
        state.renderer.submit_separator(view, xi, bar_yi, wi, bar_hi, bg);
        let text_x = xi as f32 + ((seg_w - text_w) / 2.0).floor();
        state.renderer.render_text_clipped(
            view, text, text_x, text_yi as f32, fg, bg,
            Some((xi, bar_yi, wi, bar_hi)),
        );
    };

    let mut xi = bar_x as u32;
    for (text, fg, bg, seg_w, text_w) in &left_segs {
        render_seg(state, xi, text, *fg, *bg, *seg_w, *text_w);
        xi += *seg_w as u32;
    }

    let total_right: u32 = right_segs.iter().map(|(_, _, _, sw, _)| *sw as u32).sum();
    let mut xi = (bar_x as u32 + bar_w as u32).saturating_sub(total_right);
    for (text, fg, bg, seg_w, text_w) in &right_segs {
        render_seg(state, xi, text, *fg, *bg, *seg_w, *text_w);
        xi += *seg_w as u32;
    }
}

/// Render all panes with tab bar and sidebar.
fn render_frame(state: &mut AppState) -> Result<(), termojinal_render::RenderError> {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let pane_rects = active_pane_rects(state);
    let focused_id = active_tab(state).layout.focused();
    let has_tab_bar = tab_bar_visible(state);

    // Always use the multi-pane path since we may have the tab bar/sidebar occupying space.
    let output = state.renderer.get_surface_texture()?;
    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // Clear entire surface.
    state.renderer.clear_surface(&view);

    // Fill content area with terminal background (prevents transparent padding).
    // Apply bg_opacity so transparency still works.
    {
        let mut term_bg = color_or(&state.config.theme.background, [0.067, 0.067, 0.09, 1.0]);
        term_bg[3] = state.config.window.opacity;
        let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
        let tab_h = if has_tab_bar { state.config.tab_bar.height } else { 0.0 };
        let status_h = effective_status_bar_height(state);
        let content_x = sidebar_w as u32;
        let content_y = tab_h as u32;
        let content_w = (phys_w - sidebar_w).max(0.0) as u32;
        let content_h = (phys_h - tab_h - status_h).max(0.0) as u32;
        state.renderer.submit_separator(&view, content_x, content_y, content_w, content_h, term_bg);
    }

    // Render sidebar if visible.
    if state.sidebar_visible {
        render_sidebar(state, &view, phys_h);
    }

    // Render tab bar if visible (always_show or >1 tabs).
    if has_tab_bar {
        render_tab_bar(state, &view, phys_w);
    }

    // Render each pane.
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    for (pid, rect) in &pane_rects {
        if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(pid) {
            let sel_bounds = sel_bounds_for(pane);
            let preedit = if *pid == focused_id {
                pane.preedit.as_deref()
            } else {
                None
            };
            let viewport = (
                rect.x as u32,
                rect.y as u32,
                (rect.w as u32).max(1),
                (rect.h as u32).max(1),
            );
            state
                .renderer
                .render_pane(&pane.terminal, sel_bounds, viewport, *pid, preedit, &view)?;
        }
    }

    // Draw separators between panes.
    let sep_color = color_or(&state.config.pane.separator_color, [0.3, 0.3, 0.3, 1.0]);
    let sep = state.config.pane.separator_width;
    for i in 0..pane_rects.len() {
        for j in (i + 1)..pane_rects.len() {
            let (_, r1) = &pane_rects[i];
            let (_, r2) = &pane_rects[j];

            let r1_right = (r1.x + r1.w) as u32;
            let r2_left = r2.x as u32;
            if r1_right.abs_diff(r2_left) <= 1 {
                let y0 = r1.y.max(r2.y) as u32;
                let y1 = (r1.y + r1.h).min(r2.y + r2.h) as u32;
                if y1 > y0 {
                    state.renderer.submit_separator(
                        &view,
                        r1_right.saturating_sub(sep / 2),
                        y0,
                        sep,
                        y1 - y0,
                        sep_color,
                    );
                }
            }

            let r1_bottom = (r1.y + r1.h) as u32;
            let r2_top = r2.y as u32;
            if r1_bottom.abs_diff(r2_top) <= 1 {
                let x0 = r1.x.max(r2.x) as u32;
                let x1 = (r1.x + r1.w).min(r2.x + r2.w) as u32;
                if x1 > x0 {
                    state.renderer.submit_separator(
                        &view,
                        x0,
                        r1_bottom.saturating_sub(sep / 2),
                        x1 - x0,
                        sep,
                        sep_color,
                    );
                }
            }
        }
    }

    // Focus border on the focused pane (only when multiple panes).
    if pane_rects.len() > 1 {
        let focus_color = color_or(&state.config.pane.focus_border_color, [0.2, 0.6, 1.0, 0.8]);
        let b = state.config.pane.focus_border_width;
        if let Some((_, r)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
            let (x, y, w, h) = (r.x as u32, r.y as u32, r.w as u32, r.h as u32);
            state.renderer.submit_separator(&view, x, y, w, b, focus_color);
            if h > b {
                state.renderer.submit_separator(&view, x, y + h - b, w, b, focus_color);
            }
            state.renderer.submit_separator(&view, x, y, b, h, focus_color);
            if w > b {
                state.renderer.submit_separator(&view, x + w - b, y, b, h, focus_color);
            }
        }
    }

    // Update IME cursor position for the focused pane.
    if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
        if let Some(fp) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(&focused_id) {
            let cell_size = state.renderer.cell_size();
            let x = rect.x + (fp.terminal.cursor_col as f32 * cell_size.width);
            let y = rect.y + (fp.terminal.cursor_row as f32 * cell_size.height);
            state.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x as f64, y as f64),
                winit::dpi::PhysicalSize::new(
                    cell_size.width as f64,
                    cell_size.height as f64,
                ),
            );
        }
    }

    // Render status bar at the bottom if enabled.
    if state.config.status_bar.enabled {
        render_status_bar(state, &view, phys_w, phys_h);
    }

    // Render search bar if visible (Feature 5).
    if state.search.is_some() {
        render_search_bar(state, &view, phys_w);
    }

    // Render Allow Flow overlay at the bottom of the focused pane.
    // Render Allow Flow pane hint bar (thin 1-line bar at the bottom of the
    // focused pane) when there are pending requests for the active workspace.
    if state.allow_flow.pane_hint_visible
        && state.allow_flow.has_pending_for_workspace(state.active_workspace)
    {
        render_allow_flow_pane_hint(state, &view, &pane_rects, focused_id);
    }

    // Render command palette overlay if visible.
    if state.command_palette.visible {
        render_command_palette(state, &view, phys_w, phys_h);
    }

    // Render About overlay if visible.
    if state.about_visible {
        render_about_overlay(state, &view, phys_w, phys_h);
    }

    output.present();
    Ok(())
}

/// Render the search bar at the top of the content area (Feature 5).
fn render_search_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32) {
    let search = match &state.search {
        Some(s) => s,
        None => return,
    };
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };

    let bar_x = sidebar_w;
    let bar_y: f32 = 0.0;
    let bar_w = phys_w - sidebar_w;
    let bar_h = cell_h + 4.0;

    // Background from config.
    let bar_bg = color_or(&state.config.search.bar_bg, [0.15, 0.15, 0.20, 0.95]);
    state.renderer.submit_separator(
        view,
        bar_x as u32,
        bar_y as u32,
        bar_w as u32,
        bar_h as u32,
        bar_bg,
    );

    // Input text from config.
    let input_fg = color_or(&state.config.search.input_fg, [0.95, 0.95, 0.95, 1.0]);
    let match_count = search.matches.len();
    let current = if match_count > 0 { search.current + 1 } else { 0 };
    let prompt = format!("Find: {}  ({current}/{match_count})", search.query);
    let max_chars = ((bar_w - 2.0 * cell_w) / cell_w) as usize;
    let display: String = prompt.chars().take(max_chars).collect();
    state.renderer.render_text(view, &display, bar_x + cell_w, bar_y + 2.0, input_fg, bar_bg);

    // Bottom border from config.
    let border_color = color_or(&state.config.search.border_color, [0.3, 0.3, 0.4, 1.0]);
    state.renderer.submit_separator(
        view,
        bar_x as u32,
        (bar_y + bar_h) as u32,
        bar_w as u32,
        1,
        border_color,
    );
}

/// Render a minimal 1-line hint bar at the bottom of the focused pane.
///
/// This is a thin reminder that pending Allow Flow requests exist; the
/// full request details live in the sidebar.
fn render_allow_flow_pane_hint(
    state: &mut AppState,
    view: &wgpu::TextureView,
    pane_rects: &[(PaneId, termojinal_layout::Rect)],
    focused_id: PaneId,
) {
    // Find the focused pane's rect.
    let rect = match pane_rects.iter().find(|(id, _)| *id == focused_id) {
        Some((_, r)) => r,
        None => return,
    };

    let cell_size = state.renderer.cell_size();
    let cell_h = cell_size.height;
    let cell_w = cell_size.width;

    // Thin bar: 1 cell row + small vertical padding.
    let bar_pad = 2.0_f32;
    let bar_h = cell_h + bar_pad * 2.0;
    let bar_x = rect.x as u32;
    let bar_y = ((rect.y + rect.h) - bar_h).max(rect.y) as u32;
    let bar_w = rect.w as u32;

    // Colors from config (with sensible fallbacks).
    let ui = &state.config.allow_flow_ui;
    let bar_bg = color_or(&ui.hint_bar_bg, [0.85, 0.47, 0.02, 0.88]);
    let accent = color_or(&ui.hint_bar_accent, [0.96, 0.62, 0.04, 1.0]);
    let hint_fg = color_or(&ui.hint_bar_fg, [0.10, 0.10, 0.14, 1.0]);

    state.renderer.submit_separator(view, bar_x, bar_y, bar_w, bar_h as u32, bar_bg);

    // Top accent line.
    state.renderer.submit_separator(view, bar_x, bar_y, bar_w, 1, accent);

    // Text: lightning bolt + short message + key hints.
    let text_x = bar_x as f32 + cell_w;
    let text_y = bar_y as f32 + bar_pad;
    let max_chars = ((bar_w as f32 - 2.0 * cell_w) / cell_w).max(1.0) as usize;
    let msg = "\u{26A1} AI permission needed \u{2014} y/n one \u{00B7} Y/N all \u{00B7} A always";
    let display: String = msg.chars().take(max_chars).collect();
    state.renderer.render_text(view, &display, text_x, text_y, hint_fg, bar_bg);
}

/// Render the command palette as an overlay on top of the terminal.
fn render_command_palette(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    // If a command execution is active, render its UI instead.
    if state.command_execution.is_some() {
        render_command_execution(state, view, phys_w, phys_h);
        return;
    }

    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;
    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state.renderer.submit_separator(
        view,
        0,
        0,
        phys_w as u32,
        phys_h as u32,
        overlay_color,
    );

    // 2. Centered floating box.
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let max_box_h = pc.max_height;
    let max_visible_items = pc.max_visible_items;
    let visible_items = state.command_palette.filtered.len().min(max_visible_items);
    let rows_needed = 1 + visible_items.max(1); // input row + command rows (min 1 for "No matches")
    let box_h = ((rows_needed as f32) * cell_h + cell_h).min(max_box_h); // extra padding
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background and border using SDF rounded rectangle.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let palette_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[palette_rect]);

    // 3. Input field at the top of the box.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let preedit = &state.command_palette.preedit;
    let prompt = if preedit.is_empty() {
        format!("> {}", state.command_palette.input)
    } else {
        format!("> {}[{}]", state.command_palette.input, preedit)
    };
    let palette_input_fg = color_or(&pc.input_fg, [0.95, 0.95, 0.95, 1.0]);
    state.renderer.render_text(view, &prompt, input_x, input_y, palette_input_fg, box_bg);

    // Draw a separator line below the input.
    let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
    let palette_sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        sep_y,
        box_w as u32 - 2,
        1,
        palette_sep_color,
    );

    // 4. Filtered command list.
    let list_start_y = sep_y as f32 + cell_h * 0.25;
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.82, 1.0]);
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);

    // Calculate max characters that fit in the box (for truncation).
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    // Ensure selected item is within the visible scroll window.
    state.command_palette.ensure_visible(max_visible_items);
    let scroll_offset = state.command_palette.scroll_offset;

    for (vi, &cmd_idx) in state
        .command_palette
        .filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(max_visible_items)
    {
        let row = vi - scroll_offset;
        let item_y = list_start_y + (row as f32) * cell_h;
        if item_y + cell_h > box_y + box_h {
            break;
        }

        let is_selected = vi == state.command_palette.selected;
        let bg = if is_selected { selected_bg } else { box_bg };

        // Highlight selected row with a rounded rect for consistent appearance.
        if is_selected {
            let sel_rect = RoundedRect {
                rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                color: selected_bg,
                border_color: [0.0; 4],
                params: [4.0, 0.0, 0.0, 0.0],
            };
            state.renderer.submit_rounded_rects(view, &[sel_rect]);
        }

        let cmd = &state.command_palette.commands[cmd_idx];

        // Kind badge: builtin = none, plugin = [ext], verified = [ok]
        let (badge, badge_fg) = match cmd.kind {
            CommandKind::Builtin => ("", [0.0; 4]),
            CommandKind::Plugin => ("[ext] ", [0.7, 0.55, 0.2, 1.0]),
            CommandKind::PluginVerified => ("[ok] ", [0.4, 0.8, 0.4, 1.0]),
        };
        let badge_w = if badge.is_empty() { 0.0 } else { badge.chars().count() as f32 * cell_w };
        if !badge.is_empty() {
            state.renderer.render_text(view, badge, input_x, item_y, badge_fg, bg);
        }

        // Render name in brighter color, description in dimmer color.
        let name_display: String = cmd.name.chars().take(max_chars.saturating_sub(badge.chars().count())).collect();
        let fg = if is_selected { palette_input_fg } else { cmd_fg };
        state.renderer.render_text(
            view,
            &name_display,
            input_x + badge_w,
            item_y,
            fg,
            bg,
        );

        // Render description after the name.
        let desc_offset = badge_w + name_display.len() as f32 * cell_w + 2.0 * cell_w;
        if desc_offset < box_w - 2.0 * cell_w {
            let remaining = max_chars.saturating_sub(name_display.len() + 2);
            let desc_display: String = cmd.description.chars().take(remaining).collect();
            state.renderer.render_text(
                view,
                &desc_display,
                input_x + desc_offset,
                item_y,
                desc_fg,
                bg,
            );
        }
    }

    // Show "No matches" if filtered list is empty.
    if state.command_palette.filtered.is_empty() {
        let no_match = "No matching commands";
        let empty_fg = [0.5, 0.5, 0.55, 1.0];
        state.renderer.render_text(
            view,
            no_match,
            input_x,
            list_start_y,
            empty_fg,
            box_bg,
        );
    }
}

/// Render the command execution UI as an overlay (replaces normal palette content).
fn render_command_execution(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state.renderer.submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // Borrow the execution state to compute box dimensions.
    let exec = state.command_execution.as_ref().unwrap();
    let max_visible = pc.max_visible_items;

    // Determine how many rows the content needs.
    let content_rows = match &exec.ui_state {
        CommandUIState::Loading => 2,
        CommandUIState::Fuzzy { .. } | CommandUIState::Multi { .. } => {
            let visible = exec.filtered_items.len().min(max_visible);
            2 + visible // prompt/input + separator + items
        }
        CommandUIState::Confirm { .. } => 3,
        CommandUIState::Text { .. } => 3,
        CommandUIState::Info => 2,
        CommandUIState::Done(_) => 3,
        CommandUIState::Error(_) => 3,
    };

    // 2. Centered floating box.
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let max_box_h = pc.max_height;
    let box_h = ((content_rows as f32) * cell_h + cell_h).min(max_box_h);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background and border using SDF rounded rectangle.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let exec_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[exec_rect]);

    let input_x = box_x + cell_w;
    let input_y = box_y + cell_h * 0.25;
    let palette_input_fg = color_or(&pc.input_fg, [0.95, 0.95, 0.95, 1.0]);
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.82, 1.0]);
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);
    let palette_sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    // Re-borrow exec (the earlier borrow was dropped before renderer calls).
    let exec = state.command_execution.as_ref().unwrap();

    match &exec.ui_state {
        CommandUIState::Loading => {
            let msg = format!("Running {}...", exec.command_name);
            let display: String = msg.chars().take(max_chars).collect();
            state.renderer.render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);
        }

        CommandUIState::Fuzzy { prompt } | CommandUIState::Multi { prompt } => {
            let is_multi = matches!(exec.ui_state, CommandUIState::Multi { .. });
            let prompt_str = prompt.clone();
            let input_str = exec.input.clone();
            let filtered: Vec<usize> = exec.filtered_items.clone();
            let selected_idx = exec.selected;
            let selected_set_snapshot: std::collections::HashSet<usize> = exec.selected_set.clone();
            let items_snapshot: Vec<_> = exec.items.iter().map(|item| {
                (
                    item.label.clone(),
                    item.value.clone(),
                    item.description.clone(),
                )
            }).collect();

            // Prompt and input at the top.
            let prompt_display = format!("{}: {}", prompt_str, input_str);
            let display: String = prompt_display.chars().take(max_chars).collect();
            state.renderer.render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);

            // Separator below prompt.
            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(view, box_x as u32 + 1, sep_y, box_w as u32 - 2, 1, palette_sep_color);

            // Item list.
            let list_start_y = sep_y as f32 + cell_h * 0.25;
            for (i, &item_idx) in filtered.iter().enumerate().take(max_visible) {
                let item_y = list_start_y + (i as f32) * cell_h;
                if item_y + cell_h > box_y + box_h {
                    break;
                }

                let is_selected = i == selected_idx;
                let bg = if is_selected { selected_bg } else { box_bg };

                if is_selected {
                    let sel_rect = RoundedRect {
                        rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                        color: selected_bg,
                        border_color: [0.0; 4],
                        params: [4.0, 0.0, 0.0, 0.0],
                    };
                    state.renderer.submit_rounded_rects(view, &[sel_rect]);
                }

                let (ref label_opt, ref value, ref desc_opt) = items_snapshot[item_idx];
                let label = label_opt.as_deref().unwrap_or(value.as_str());

                // Multi-select: show check mark for toggled items.
                let prefix = if is_multi {
                    if selected_set_snapshot.contains(&item_idx) { "[x] " } else { "[ ] " }
                } else {
                    ""
                };

                let name_display: String = format!("{}{}", prefix, label)
                    .chars()
                    .take(max_chars)
                    .collect();
                let fg = if is_selected { palette_input_fg } else { cmd_fg };
                state.renderer.render_text(view, &name_display, input_x, item_y, fg, bg);

                // Description after the name.
                if let Some(ref desc) = desc_opt {
                    let desc_offset = name_display.len() as f32 * cell_w + 2.0 * cell_w;
                    if desc_offset < box_w - 2.0 * cell_w {
                        let remaining = max_chars.saturating_sub(name_display.len() + 2);
                        let desc_display: String = desc.chars().take(remaining).collect();
                        state.renderer.render_text(view, &desc_display, input_x + desc_offset, item_y, desc_fg, bg);
                    }
                }
            }

            // Show "No matches" if filtered list is empty.
            if filtered.is_empty() {
                let no_match = "No matching items";
                let empty_fg = [0.5, 0.5, 0.55, 1.0];
                let list_start_y = (input_y + cell_h + cell_h * 0.25) as f32 + cell_h * 0.25;
                state.renderer.render_text(view, no_match, input_x, list_start_y, empty_fg, box_bg);
            }
        }

        CommandUIState::Confirm { message, default } => {
            let message_str = message.clone();
            let default_val = *default;
            let display: String = message_str.chars().take(max_chars).collect();
            state.renderer.render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);

            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(view, box_x as u32 + 1, sep_y, box_w as u32 - 2, 1, palette_sep_color);

            let hint = if default_val {
                "Press Enter for Yes, or N for No"
            } else {
                "Press Y for Yes, or Enter for No"
            };
            let hint_y = sep_y as f32 + cell_h * 0.25;
            state.renderer.render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }

        CommandUIState::Text { label, placeholder } => {
            let label_str = label.clone();
            let placeholder_str = placeholder.clone();
            let input_str = exec.input.clone();
            let label_display: String = format!("{}:", label_str).chars().take(max_chars).collect();
            state.renderer.render_text(view, &label_display, input_x, input_y, palette_input_fg, box_bg);

            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(view, box_x as u32 + 1, sep_y, box_w as u32 - 2, 1, palette_sep_color);

            let text_y = sep_y as f32 + cell_h * 0.25;
            if input_str.is_empty() && !placeholder_str.is_empty() {
                let ph_display: String = placeholder_str.chars().take(max_chars).collect();
                state.renderer.render_text(view, &ph_display, input_x, text_y, desc_fg, box_bg);
            } else {
                let input_display: String = input_str.chars().take(max_chars).collect();
                state.renderer.render_text(view, &input_display, input_x, text_y, palette_input_fg, box_bg);
            }
        }

        CommandUIState::Info => {
            let info_msg = exec.info_message.clone();
            let spinner_chars = ["|", "/", "-", "\\"];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() / 200)
                .unwrap_or(0) % 4) as usize;
            let spinner = spinner_chars[tick];
            let msg = format!("{} {}", spinner, info_msg);
            let display: String = msg.chars().take(max_chars).collect();
            state.renderer.render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);
        }

        CommandUIState::Done(notify) => {
            let msg = if let Some(ref text) = notify {
                format!("Done: {}", text)
            } else {
                let name = exec.command_name.clone();
                format!("Command '{}' completed", name)
            };
            let display: String = msg.chars().take(max_chars).collect();
            let done_fg = [0.55, 0.82, 0.33, 1.0]; // green
            state.renderer.render_text(view, &display, input_x, input_y, done_fg, box_bg);

            let hint = "Press any key to dismiss";
            let hint_y = input_y + cell_h;
            state.renderer.render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }

        CommandUIState::Error(message) => {
            let msg = format!("Error: {}", message);
            let display: String = msg.chars().take(max_chars).collect();
            let error_fg = [1.0, 0.42, 0.42, 1.0]; // red
            state.renderer.render_text(view, &display, input_x, input_y, error_fg, box_bg);

            let hint = "Press Esc to dismiss";
            let hint_y = input_y + cell_h;
            state.renderer.render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }
    }
}

/// Load the "About Termojinal" text including version, copyright, and third-party licenses.
fn load_about_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut text = format!("Termojinal v{version}\n");
    text.push_str("GPU-accelerated terminal emulator\n");
    text.push_str("Copyright (c) 2026 Tomoo Kikuchi\n");
    text.push_str("MIT License\n\n");

    // Try to load THIRD_PARTY_LICENSES.md from the executable's directory
    // or from known locations.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let search_paths = [
        exe_dir.as_ref().map(|d| d.join("../Resources/THIRD_PARTY_LICENSES.md")),
        exe_dir.as_ref().map(|d| d.join("../../THIRD_PARTY_LICENSES.md")),
        Some(std::path::PathBuf::from("THIRD_PARTY_LICENSES.md")),
    ];

    for path in search_paths.iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(path) {
            text.push_str(&content);
            break;
        }
    }

    text
}

/// Render the "About Termojinal" overlay on top of the terminal.
fn render_about_overlay(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state.renderer.submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box (use most of the window).
    let box_w = (phys_w * 0.7).min(phys_w - 40.0).max(200.0);
    let box_h = (phys_h * 0.7).min(phys_h - 40.0).max(100.0);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h - box_h) / 2.0;

    // Draw box background and border.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let about_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[about_rect]);

    // 3. Load and render the about text.
    let about_text = load_about_text();
    let lines: Vec<&str> = about_text.lines().collect();
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;
    let content_x = box_x + cell_w;

    // Reserve space for the footer hint line.
    let footer_h = cell_h * 1.5;
    let content_area_h = box_h - cell_h * 0.5 - footer_h;
    let max_visible_lines = (content_area_h / cell_h) as usize;

    // Clamp scroll offset.
    let max_scroll = lines.len().saturating_sub(max_visible_lines);
    if state.about_scroll > max_scroll {
        state.about_scroll = max_scroll;
    }
    let scroll = state.about_scroll;

    // Title/header styling.
    let title_fg = [0.95, 0.95, 0.95, 1.0];
    let text_fg = [0.75, 0.75, 0.78, 1.0];
    let hint_fg = [0.5, 0.5, 0.55, 1.0];

    for (i, line) in lines.iter().enumerate().skip(scroll).take(max_visible_lines) {
        let row = i - scroll;
        let y = box_y + cell_h * 0.25 + (row as f32) * cell_h;
        let display: String = line.chars().take(max_chars).collect();

        // Use brighter color for the first few header lines.
        let fg = if i < 4 { title_fg } else { text_fg };
        state.renderer.render_text(view, &display, content_x, y, fg, box_bg);
    }

    // 4. Footer: "Press any key to close" hint.
    let footer_y = box_y + box_h - cell_h * 1.25;

    // Separator above footer.
    let sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        (footer_y - cell_h * 0.25) as u32,
        box_w as u32 - 2,
        1,
        sep_color,
    );

    let scroll_hint = if max_scroll > 0 {
        "Arrow keys to scroll, any other key to close"
    } else {
        "Press any key to close"
    };
    state.renderer.render_text(view, scroll_hint, content_x, footer_y, hint_fg, box_bg);
}

fn sel_bounds_for(pane: &Pane) -> Option<((usize, usize), (usize, usize))> {
    match &pane.selection {
        Some(s) if !s.is_empty() => {
            let ((sc, abs_sr), (ec, abs_er)) = s.ordered_abs();
            let current_scroll = pane.terminal.scroll_offset() as isize;
            let rows = pane.terminal.rows() as isize;

            // Convert absolute row to screen-relative:
            // screen_row = abs_row + current_scroll_offset
            let vis_sr = abs_sr + current_scroll;
            let vis_er = abs_er + current_scroll;

            // If the selection is entirely outside the viewport, return None.
            if vis_er < 0 || vis_sr >= rows {
                return None;
            }

            // Clamp to viewport bounds.
            let clamped_sr = vis_sr.max(0) as usize;
            let clamped_er = (vis_er.min(rows - 1)) as usize;
            let clamped_sc = if vis_sr >= 0 { sc } else { 0 };
            let clamped_ec = if vis_er < rows {
                ec
            } else {
                pane.terminal.cols().saturating_sub(1)
            };

            Some(((clamped_sc, clamped_sr), (clamped_ec, clamped_er)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// macOS window transparency
// ---------------------------------------------------------------------------

/// Get the CWD of a process via `lsof` (called from background thread only).
fn get_child_cwd(pid: i32) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(path) = line.strip_prefix('n') {
            if path.starts_with('/') { return Some(path.to_string()); }
        }
    }
    None
}

/// Detect SSH connection from a PTY's child process tree.
/// Walks the process tree starting from `pid` to find an `ssh` child process,
/// then parses its arguments to extract user@host.
fn detect_ssh_from_pid(pid: i32) -> Option<(Option<String>, String)> {
    // Get all processes with ppid,pid,command to build child lookup.
    let output = std::process::Command::new("ps")
        .args(["ax", "-o", "ppid=,pid=,command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    // Build a map: ppid -> [(pid, command)]
    let mut children: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let mut parts = trimmed.splitn(3, char::is_whitespace);
        let ppid: i32 = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(-1);
        let child_pid: i32 = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(-1);
        let cmd = parts.next().unwrap_or("").trim().to_string();
        if ppid >= 0 && child_pid >= 0 {
            children.entry(ppid).or_default().push((child_pid, cmd));
        }
    }

    // BFS from pid to find ssh in descendants.
    let mut queue = vec![pid];
    let mut visited = std::collections::HashSet::new();
    while let Some(current) = queue.pop() {
        if !visited.insert(current) { continue; }
        if let Some(kids) = children.get(&current) {
            for (child_pid, cmd) in kids {
                queue.push(*child_pid);
                if let Some(result) = parse_ssh_command(cmd) {
                    return Some(result);
                }
            }
        }
    }
    None
}

/// Parse an ssh command string to extract (Option<user>, host).
/// Uses `ssh -G <destination>` to resolve the actual hostname and user
/// from ~/.ssh/config, supporting aliases like `ssh mydev`.
fn parse_ssh_command(cmd: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return None; }

    let bin = std::path::Path::new(parts[0])
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if bin != "ssh" { return None; }

    // Find destination: first non-flag argument.
    let mut i = 1;
    let mut destination = None;
    while i < parts.len() {
        let arg = parts[i];
        if arg.starts_with('-') {
            if matches!(arg, "-p" | "-i" | "-l" | "-o" | "-F" | "-J"
                | "-L" | "-R" | "-D" | "-W" | "-b" | "-c" | "-e" | "-m" | "-S" | "-w") {
                i += 1;
            }
        } else {
            destination = Some(arg.to_string());
            break;
        }
        i += 1;
    }

    let dest = destination?;

    // Use `ssh -G <dest>` to resolve actual user and hostname from ssh config.
    // Output is `key value` pairs, one per line. We split on first whitespace.
    if let Ok(output) = std::process::Command::new("ssh")
        .args(["-G", &dest])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut resolved_user = None;
            let mut resolved_host = None;
            for line in text.lines() {
                let line = line.trim();
                if let Some(idx) = line.find(char::is_whitespace) {
                    let key = &line[..idx];
                    let val = line[idx..].trim();
                    match key.to_lowercase().as_str() {
                        "user" => resolved_user = Some(val.to_string()),
                        "hostname" => resolved_host = Some(val.to_string()),
                        _ => {}
                    }
                }
            }
            if let Some(host) = resolved_host {
                return Some((resolved_user, host));
            }
        }
    }

    // Fallback: parse destination directly.
    if let Some(at_idx) = dest.find('@') {
        Some((Some(dest[..at_idx].to_string()), dest[at_idx + 1..].to_string()))
    } else {
        Some((None, dest))
    }
}

// ---------------------------------------------------------------------------
// Right-click context menu (macOS native NSMenu)
// ---------------------------------------------------------------------------

/// Context menu actions returned after the user makes a selection.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuAction {
    Copy,
    Paste,
    SelectAll,
    Clear,
    SplitRight,
    SplitDown,
}

/// Show a native macOS right-click context menu and return the selected action.
///
/// Uses `popUpContextMenu:withEvent:forView:` which is synchronous — it blocks
/// until the user picks an item or dismisses the menu. Menu item callbacks
/// record the selected tag into a thread-local, which we read after the menu
/// closes.
#[cfg(target_os = "macos")]
fn show_context_menu(
    window: &winit::window::Window,
    has_selection: bool,
) -> Option<ContextMenuAction> {
    use std::cell::Cell;
    use objc2::rc::{Allocated, Id};
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
    use objc2::{class, msg_send, msg_send_id, sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(_) => return None,
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let ns_view = appkit.ns_view.as_ptr() as *mut AnyObject;

    // Tag constants.
    const TAG_COPY: isize = 1;
    const TAG_PASTE: isize = 2;
    const TAG_SELECT_ALL: isize = 3;
    const TAG_CLEAR: isize = 4;
    const TAG_SPLIT_RIGHT: isize = 5;
    const TAG_SPLIT_DOWN: isize = 6;

    // Thread-local to communicate the selected tag from the ObjC callback.
    thread_local! {
        static SELECTED_TAG: Cell<isize> = const { Cell::new(0) };
    }

    // ObjC callback function for menu item selection.
    unsafe extern "C" fn menu_item_clicked(
        _this: *mut AnyObject,
        _sel: Sel,
        sender: *mut AnyObject,
    ) {
        let tag: isize = msg_send![&*sender, tag];
        SELECTED_TAG.with(|cell| cell.set(tag));
    }

    unsafe {
        // Register (or reuse) a small ObjC class with a `menuAction:` method.
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        static mut MENU_TARGET_CLASS: *const AnyClass = std::ptr::null();

        REGISTERED.call_once(|| {
            let superclass = class!(NSObject);
            let mut builder = ClassBuilder::new("TermojinalMenuTarget", superclass)
                .expect("failed to create TermojinalMenuTarget class");
            builder.add_method(
                sel!(menuAction:),
                menu_item_clicked as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            MENU_TARGET_CLASS = builder.register() as *const AnyClass;
        });

        // Create an instance of our target class.
        let target_class = &*MENU_TARGET_CLASS;
        let target: Id<NSObject> = msg_send_id![target_class, new];
        let action_sel = sel!(menuAction:);

        // Create the menu.
        let menu: Id<NSObject> = msg_send_id![class!(NSMenu), new];
        // Disable auto-enable so we can manually control enabled state.
        let () = msg_send![&*menu, setAutoenablesItems: false];

        // Helper: NSString from &str (null-terminated via CString).
        let make_nsstring = |s: &str| -> Id<NSObject> {
            let cstr = std::ffi::CString::new(s).unwrap();
            let ns: Id<NSObject> = msg_send_id![
                class!(NSString),
                stringWithUTF8String: cstr.as_ptr()
            ];
            ns
        };

        // Helper: create a menu item.
        let make_item =
            |title: &str, key_equiv: &str, tag: isize, enabled: bool| -> Id<NSObject> {
                let ns_title = make_nsstring(title);
                let ns_key = make_nsstring(key_equiv);
                let item: Allocated<NSObject> = msg_send_id![
                    class!(NSMenuItem),
                    alloc
                ];
                let item: Id<NSObject> = msg_send_id![
                    item,
                    initWithTitle: &*ns_title,
                    action: action_sel,
                    keyEquivalent: &*ns_key
                ];
                let () = msg_send![&*item, setTarget: &*target];
                let () = msg_send![&*item, setTag: tag];
                let () = msg_send![&*item, setEnabled: enabled];
                item
            };

        let make_separator = || -> Id<NSObject> {
            msg_send_id![class!(NSMenuItem), separatorItem]
        };

        // --- Build menu ---
        let copy_item = make_item("Copy", "c", TAG_COPY, has_selection);
        let () = msg_send![&*menu, addItem: &*copy_item];

        let paste_item = make_item("Paste", "v", TAG_PASTE, true);
        let () = msg_send![&*menu, addItem: &*paste_item];

        let sep1 = make_separator();
        let () = msg_send![&*menu, addItem: &*sep1];

        let select_all_item = make_item("Select All", "a", TAG_SELECT_ALL, true);
        let () = msg_send![&*menu, addItem: &*select_all_item];

        let clear_item = make_item("Clear", "", TAG_CLEAR, true);
        let () = msg_send![&*menu, addItem: &*clear_item];

        let sep2 = make_separator();
        let () = msg_send![&*menu, addItem: &*sep2];

        let split_right_item = make_item("Split Right", "d", TAG_SPLIT_RIGHT, true);
        let () = msg_send![&*menu, addItem: &*split_right_item];

        let split_down_item = make_item("Split Down", "D", TAG_SPLIT_DOWN, true);
        let () = msg_send![&*menu, addItem: &*split_down_item];

        // Reset selected tag before showing menu.
        SELECTED_TAG.with(|cell| cell.set(0));

        // Get current event from NSApplication (needed for popUpContextMenu).
        let ns_app: Id<NSObject> = msg_send_id![class!(NSApplication), sharedApplication];
        let current_event: Option<Id<NSObject>> = msg_send_id![&*ns_app, currentEvent];

        let Some(event) = current_event else {
            return None;
        };

        // popUpContextMenu:withEvent:forView: is synchronous — blocks until
        // the user selects an item or dismisses the menu.
        let () = msg_send![
            class!(NSMenu),
            popUpContextMenu: &*menu,
            withEvent: &*event,
            forView: &*ns_view
        ];

        // Read back which tag was selected (0 = dismissed without selection).
        let selected = SELECTED_TAG.with(|cell| cell.get());

        match selected {
            TAG_COPY => Some(ContextMenuAction::Copy),
            TAG_PASTE => Some(ContextMenuAction::Paste),
            TAG_SELECT_ALL => Some(ContextMenuAction::SelectAll),
            TAG_CLEAR => Some(ContextMenuAction::Clear),
            TAG_SPLIT_RIGHT => Some(ContextMenuAction::SplitRight),
            TAG_SPLIT_DOWN => Some(ContextMenuAction::SplitDown),
            _ => None,
        }
    }
}

/// Set NSWindow background to clear so wgpu alpha compositing shows through.
#[cfg(target_os = "macos")]
fn set_macos_window_transparent(window: &winit::window::Window) {
    use objc2::{class, msg_send, msg_send_id};
    use objc2::rc::Id;
    use objc2::runtime::NSObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(_) => return,
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
    let ns_view = appkit.ns_view.as_ptr() as *const NSObject;

    unsafe {
        // ns_view.window -> NSWindow
        let ns_window: Option<Id<NSObject>> = msg_send_id![ns_view, window];
        if let Some(ns_window) = ns_window {
            let clear_color: Id<NSObject> = msg_send_id![class!(NSColor), clearColor];
            let () = msg_send![&*ns_window, setBackgroundColor: &*clear_color];
            let () = msg_send![&*ns_window, setOpaque: false];
            log::info!("macOS window background set to clear for transparency");
        }
    }
}

// ---------------------------------------------------------------------------
// Window icon
// ---------------------------------------------------------------------------

/// Set the macOS Dock icon from an embedded PNG using raw objc messaging.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{class, msg_send, msg_send_id};
    use objc2::rc::Id;
    use objc2::runtime::NSObject;

    // Load icon PNG and add ~18% transparent padding (Apple HIG standard).
    let png_bytes = include_bytes!("../resources/Assets.xcassets/AppIcon.appiconset/256.png");
    let padded = match add_icon_padding(png_bytes) {
        Some(data) => data,
        None => png_bytes.to_vec(),
    };

    unsafe {
        let cls = class!(NSData);
        let ptr = padded.as_ptr() as *const std::ffi::c_void;
        let len = padded.len();
        let data: Id<NSObject> = msg_send_id![
            cls, dataWithBytes: ptr, length: len
        ];

        let cls = class!(NSImage);
        let image: Option<Id<NSObject>> = msg_send_id![
            msg_send_id![cls, alloc],
            initWithData: &*data
        ];

        if let Some(image) = image {
            let cls = class!(NSApplication);
            let app: Id<NSObject> = msg_send_id![cls, sharedApplication];
            let () = msg_send![&*app, setApplicationIconImage: &*image];
            log::info!("dock icon set");
        }
    }
}

/// Add transparent padding around an icon PNG (~18% on each side per Apple HIG).
#[cfg(target_os = "macos")]
fn add_icon_padding(png_bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png).ok()?;
    let src = img.to_rgba8();

    // Target: 1024x1024 canvas with icon at ~824x824 (80% of canvas), centered.
    let canvas_size = 1024u32;
    let icon_size = (canvas_size as f32 * 0.80) as u32;
    let offset = (canvas_size - icon_size) / 2;

    let resized = image::imageops::resize(&src, icon_size, icon_size, image::imageops::FilterType::Lanczos3);

    let mut canvas = image::RgbaImage::new(canvas_size, canvas_size);
    image::imageops::overlay(&mut canvas, &resized, offset as i64, offset as i64);

    let mut buf = std::io::Cursor::new(Vec::new());
    canvas.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

#[cfg(not(target_os = "macos"))]
fn set_dock_icon() {}

// ---------------------------------------------------------------------------
// App IPC request handler (JSON protocol)
// ---------------------------------------------------------------------------

/// Handle a structured `AppIpcRequest` and return an `AppIpcResponse`.
///
/// Returns `None` for `PermissionRequest` to indicate the response is deferred
/// (the `response_tx` is stored in `state.pending_ipc_responses` and sent later
/// when the user makes a decision).
///
/// This is called on the GUI thread from the `UserEvent::AppIpc` handler.
fn handle_app_ipc_request(
    state: &mut AppState,
    request: &AppIpcRequest,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
    response_tx: &std_mpsc::Sender<AppIpcResponse>,
    connection_alive: Option<Arc<AtomicBool>>,
) -> Option<AppIpcResponse> {
    // PermissionRequest is deferred: store the response_tx and return None.
    if let AppIpcRequest::PermissionRequest {
        tool_name,
        tool_input,
        session_id,
    } = request
    {
        use termojinal_claude::request::{AllowRequest, DetectionSource};

        let action = tool_input
            .get("command")
            .or(tool_input.get("file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("tool use")
            .to_string();
        let detail = serde_json::to_string(&tool_input).unwrap_or_default();

        // Resolve workspace index from session mapping, falling back to active.
        let ws_idx = session_id
            .as_ref()
            .and_then(|sid| state.session_to_workspace.get(sid).copied())
            .unwrap_or(state.active_workspace);

        // Record the mapping for future requests from this session.
        if let Some(sid) = session_id.as_ref() {
            if !state.session_to_workspace.contains_key(sid) {
                state.session_to_workspace.insert(sid.clone(), ws_idx);
            }
        }

        // Update agent session info for this workspace.
        while state.agent_infos.len() <= ws_idx {
            state.agent_infos.push(AgentSessionInfo::default());
        }
        let agent = &mut state.agent_infos[ws_idx];
        agent.active = true;
        agent.state = AgentState::WaitingForPermission;
        agent.session_id = session_id.clone();
        agent.summary = format!("{}: {}", tool_name, &action);
        agent.last_updated = std::time::Instant::now();

        let notif_msg = format!("Permission: {} {}", tool_name, action);
        let request = AllowRequest::new(
            0, // no specific pane for IPC-originated requests
            ws_idx,
            tool_name.clone(),
            action,
            detail,
            DetectionSource::Ipc,
            String::new(), // yes_response: not used for IPC (decision goes back via channel)
            String::new(), // no_response: not used for IPC
        );

        if let Some(req) = state.allow_flow.engine.add_request(request) {
            let req_id = req.id;
            let alive = connection_alive.unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
            state
                .pending_ipc_responses
                .insert(req_id, (response_tx.clone(), alive));
            state.allow_flow.pane_hint_visible = true;
            notification::send_notification(
                "Claude Code",
                &notif_msg,
                state.config.notifications.sound,
            );
            state.window.request_redraw();
            return None; // Deferred — response sent when user decides
        } else {
            // Auto-resolved by a rule.
            return Some(AppIpcResponse::ok(json!({"decision": "allow"})));
        }
    }

    Some(match request {
        AppIpcRequest::Ping => AppIpcResponse::ok(json!("pong")),

        AppIpcRequest::GetStatus => {
            let ws_idx = state.active_workspace;
            let ws = &state.workspaces[ws_idx];
            let tab = &ws.tabs[ws.active_tab];
            let focused_id = tab.layout.focused();
            AppIpcResponse::ok(json!({
                "active_workspace": ws_idx,
                "workspace_name": ws.name,
                "active_tab": ws.active_tab,
                "focused_pane": focused_id,
                "workspace_count": state.workspaces.len(),
            }))
        }

        AppIpcRequest::GetConfig => {
            AppIpcResponse::ok(json!({
                "font_size": state.config.font.size,
                "opacity": state.config.window.opacity,
                "theme_bg": state.config.theme.background,
                "theme_fg": state.config.theme.foreground,
            }))
        }

        AppIpcRequest::ListWorkspaces => {
            let workspaces: Vec<_> = state
                .workspaces
                .iter()
                .enumerate()
                .map(|(i, ws)| {
                    json!({
                        "index": i,
                        "name": ws.name,
                        "tab_count": ws.tabs.len(),
                        "active_tab": ws.active_tab,
                        "is_active": i == state.active_workspace,
                    })
                })
                .collect();
            AppIpcResponse::ok(json!({ "workspaces": workspaces }))
        }

        AppIpcRequest::CreateWorkspace { name, .. } => {
            dispatch_action(state, &Action::NewWorkspace, proxy, buffers, event_loop);
            // Optionally set the workspace name.
            if let Some(name) = name {
                if let Some(ws) = state.workspaces.last_mut() {
                    ws.name = name.clone();
                }
            }
            AppIpcResponse::ok(json!({ "index": state.workspaces.len() - 1 }))
        }

        AppIpcRequest::SwitchWorkspace { index } => {
            if *index < state.workspaces.len() {
                state.active_workspace = *index;
                update_window_title(state);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err(format!("workspace index {} out of range", index))
            }
        }

        AppIpcRequest::CloseWorkspace { index } => {
            if *index < state.workspaces.len() && state.workspaces.len() > 1 {
                state.workspaces.remove(*index);
                if *index < state.workspace_infos.len() {
                    state.workspace_infos.remove(*index);
                }
                if *index < state.agent_infos.len() {
                    state.agent_infos.remove(*index);
                }
                cleanup_session_to_workspace(state, *index);
                if state.active_workspace >= state.workspaces.len() {
                    state.active_workspace = state.workspaces.len() - 1;
                }
                update_window_title(state);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("cannot close workspace")
            }
        }

        AppIpcRequest::ListTabs { workspace } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let tabs: Vec<_> = ws
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| {
                        json!({
                            "index": i,
                            "name": tab.name,
                            "pane_count": tab.panes.len(),
                            "is_active": i == ws.active_tab,
                        })
                    })
                    .collect();
                AppIpcResponse::ok(json!({ "tabs": tabs }))
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::CreateTab { .. } => {
            dispatch_action(state, &Action::NewTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::SwitchTab { workspace, index } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get_mut(ws_idx) {
                if *index < ws.tabs.len() {
                    ws.active_tab = *index;
                    update_window_title(state);
                    state.window.request_redraw();
                    AppIpcResponse::ok_empty()
                } else {
                    AppIpcResponse::err("tab index out of range")
                }
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::CloseTab { .. } => {
            dispatch_action(state, &Action::CloseTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ListPanes { workspace, tab } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let tab_idx = tab.unwrap_or(ws.active_tab);
                if let Some(tab) = ws.tabs.get(tab_idx) {
                    let focused = tab.layout.focused();
                    let panes: Vec<_> = tab
                        .panes
                        .iter()
                        .map(|(id, pane)| {
                            json!({
                                "pane_id": id,
                                "cols": pane.terminal.cols(),
                                "rows": pane.terminal.rows(),
                                "is_focused": *id == focused,
                                "cwd": pane.terminal.osc.cwd,
                            })
                        })
                        .collect();
                    AppIpcResponse::ok(json!({ "panes": panes }))
                } else {
                    AppIpcResponse::err("invalid tab index")
                }
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::SplitPane { direction, .. } => {
            let action = match direction.as_str() {
                "horizontal" | "right" => Action::SplitRight,
                "vertical" | "down" => Action::SplitDown,
                _ => return Some(AppIpcResponse::err("direction must be 'horizontal' or 'vertical'")),  // early return still needs Some
            };
            dispatch_action(state, &action, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ClosePane { .. } => {
            dispatch_action(state, &Action::CloseTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::FocusPane { pane_id } => {
            let ws = &mut state.workspaces[state.active_workspace];
            let tab = &mut ws.tabs[ws.active_tab];
            let target = *pane_id;
            if tab.panes.contains_key(&target) {
                tab.layout = tab.layout.focus(target);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err(format!("pane {} not found", pane_id))
            }
        }

        AppIpcRequest::ZoomPane { .. } => {
            dispatch_action(state, &Action::ZoomPane, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::SendKeys { pane_id, keys } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let bytes = unescape_keys(keys);
                let _ = pane.pty.write(&bytes);
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::RunCommand { pane_id, command } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let mut cmd = command.clone();
                if !cmd.ends_with('\n') {
                    cmd.push('\n');
                }
                let _ = pane.pty.write(cmd.as_bytes());
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::GetTerminalContent { pane_id } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let grid = pane.terminal.grid();
                let cols = grid.cols();
                let rows = grid.rows();
                let mut lines = Vec::new();
                for row in 0..rows {
                    let mut line = String::new();
                    for col in 0..cols {
                        let cell = grid.cell(col, row);
                        line.push(if cell.c == '\0' { ' ' } else { cell.c });
                    }
                    lines.push(line.trim_end().to_string());
                }
                AppIpcResponse::ok(json!({
                    "lines": lines,
                    "cols": cols,
                    "rows": rows,
                    "cursor_row": pane.terminal.cursor_row,
                    "cursor_col": pane.terminal.cursor_col,
                }))
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::GetScrollback { pane_id, lines } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let max_lines = lines.unwrap_or(100).min(5000);
                let scrollback_len = pane.terminal.scrollback_len();
                let start = scrollback_len.saturating_sub(max_lines);
                let mut result_lines = Vec::new();
                for i in start..scrollback_len {
                    if let Some(row) = pane.terminal.scrollback_row(i) {
                        let line: String =
                            row.iter()
                                .map(|c| if c.c == '\0' { ' ' } else { c.c })
                                .collect();
                        result_lines.push(line.trim_end().to_string());
                    }
                }
                AppIpcResponse::ok(json!({
                    "lines": result_lines,
                    "total": scrollback_len,
                }))
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::ListPendingRequests { workspace } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            let pending = state.allow_flow.pending_for_workspace(ws_idx);
            let requests: Vec<_> = pending
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "tool": r.tool_name,
                        "action": r.action,
                        "detail": r.detail,
                        "pane_id": r.pane_id,
                    })
                })
                .collect();
            AppIpcResponse::ok(json!({ "requests": requests }))
        }

        AppIpcRequest::ApproveRequest { request_id } => {
            let mut pane_ptys: HashMap<u64, *mut Pty> = HashMap::new();
            for ws in &mut state.workspaces {
                for tab in &mut ws.tabs {
                    for (pid, pane) in &mut tab.panes {
                        pane_ptys.insert(*pid, &mut pane.pty as *mut Pty);
                    }
                }
            }
            if let Some(response) =
                state
                    .allow_flow
                    .engine
                    .respond(*request_id, termojinal_claude::AllowDecision::Allow)
            {
                allow_flow::AllowFlowUI::write_to_pty(
                    &mut pane_ptys,
                    response.pane_id,
                    &response.pty_write,
                );
                // Also resolve deferred IPC response if this was hook-originated.
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(request_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "allow"})));
                }
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("request not found or already resolved")
            }
        }

        AppIpcRequest::DenyRequest { request_id } => {
            let mut pane_ptys: HashMap<u64, *mut Pty> = HashMap::new();
            for ws in &mut state.workspaces {
                for tab in &mut ws.tabs {
                    for (pid, pane) in &mut tab.panes {
                        pane_ptys.insert(*pid, &mut pane.pty as *mut Pty);
                    }
                }
            }
            if let Some(response) =
                state
                    .allow_flow
                    .engine
                    .respond(*request_id, termojinal_claude::AllowDecision::Deny)
            {
                allow_flow::AllowFlowUI::write_to_pty(
                    &mut pane_ptys,
                    response.pane_id,
                    &response.pty_write,
                );
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(request_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "deny"})));
                }
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("request not found or already resolved")
            }
        }

        AppIpcRequest::ApproveAll { workspace } => {
            let mut pane_ptys: HashMap<u64, *mut Pty> = HashMap::new();
            for ws in &mut state.workspaces {
                for tab in &mut ws.tabs {
                    for (pid, pane) in &mut tab.panes {
                        pane_ptys.insert(*pid, &mut pane.pty as *mut Pty);
                    }
                }
            }
            let resolved = state
                .allow_flow
                .allow_all_for_workspace(*workspace, &mut pane_ptys);
            for (req_id, _) in &resolved {
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(req_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "allow"})));
                }
            }
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::Notify {
            title,
            body,
            subtitle: _,
            notification_type,
        } => {
            // 1. Send macOS desktop notification.
            let notif_title = title.as_deref().unwrap_or("termojinal");
            let notif_body = body.as_deref().unwrap_or("");
            notification::send_notification(
                notif_title,
                notif_body,
                state.config.notifications.sound,
            );

            // 2. Mark the active workspace as having unread activity.
            if let Some(info) = state.workspace_infos.get_mut(state.active_workspace) {
                info.has_unread = true;
            }

            // 3. If it's a permission_prompt, show Allow Flow pane hint.
            if notification_type.as_deref() == Some("permission_prompt") {
                state.allow_flow.pane_hint_visible = true;
            }

            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ToggleQuickTerminal => {
            toggle_quick_terminal(state);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ShowPalette => {
            state.command_palette.toggle();
            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        // Handled by the early-return above; unreachable in practice.
        AppIpcRequest::PermissionRequest { .. } => unreachable!(),
    })
}

/// Unescape common key sequences: `\n`, `\r`, `\t`, `\xNN`, `\\`.
fn unescape_keys(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('r') => result.push(b'\r'),
                Some('t') => result.push(b'\t'),
                Some('\\') => result.push(b'\\'),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                    }
                }
                Some(other) => {
                    result.push(b'\\');
                    let mut buf = [0u8; 4];
                    result.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => result.push(b'\\'),
            }
        } else {
            let mut buf = [0u8; 4];
            result.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// App-side IPC listener (receives commands from the daemon)
// ---------------------------------------------------------------------------

/// Get the app IPC socket path (matches `termojinal_session::daemon::app_socket_path`).
fn app_ipc_socket_path() -> std::path::PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir.join("termojinal").join("termojinal-app.sock")
}

/// Listen for IPC commands from the daemon (e.g., toggle_quick_terminal).
///
/// Binds a Unix domain socket at `~/.local/share/termojinal/termojinal-app.sock` and
/// dispatches incoming line-delimited commands as `UserEvent`s on the winit
/// event loop.
fn app_ipc_listener(proxy: EventLoopProxy<UserEvent>) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let sock_path = app_ipc_socket_path();

    // Ensure parent directory exists.
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(&sock_path);

    let listener = match UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => {
            log::error!("failed to bind app IPC socket at {}: {e}", sock_path.display());
            return;
        }
    };
    log::info!("app IPC listener started at {}", sock_path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let reader = match stream.try_clone() {
                    Ok(s) => BufReader::new(s),
                    Err(_) => continue,
                };
                // Track the connection alive flag for this client.
                let connection_alive = Arc::new(AtomicBool::new(true));
                let mut had_permission_request = false;

                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            log::debug!("app IPC read error: {e}");
                            break;
                        }
                    };
                    let cmd = line.trim();
                    if cmd.is_empty() {
                        continue;
                    }

                    // Try JSON protocol first
                    if let Ok(request) = serde_json::from_str::<AppIpcRequest>(cmd) {
                        let is_permission_request =
                            matches!(&request, AppIpcRequest::PermissionRequest { .. });
                        let alive_for_event = if is_permission_request {
                            had_permission_request = true;
                            Some(Arc::clone(&connection_alive))
                        } else {
                            None
                        };
                        let (tx, rx) = std_mpsc::channel();
                        if proxy
                            .send_event(UserEvent::AppIpc {
                                request,
                                response_tx: tx,
                                connection_alive: alive_for_event,
                            })
                            .is_err()
                        {
                            break;
                        }
                        // PermissionRequest waits up to 10 minutes (matches hook timeout).
                        let timeout = if is_permission_request {
                            std::time::Duration::from_secs(600)
                        } else {
                            std::time::Duration::from_secs(5)
                        };
                        if let Ok(response) = rx.recv_timeout(timeout) {
                            let json_str =
                                serde_json::to_string(&response).unwrap_or_default();
                            let _ = stream.write_all(json_str.as_bytes());
                            let _ = stream.write_all(b"\n");
                            let _ = stream.flush();
                        }
                    } else {
                        // Legacy text protocol
                        match cmd {
                            "toggle_quick_terminal" => {
                                let _ =
                                    proxy.send_event(UserEvent::ToggleQuickTerminal);
                            }
                            "show_palette" => {
                                log::debug!(
                                    "app IPC: show_palette (not yet wired)"
                                );
                            }
                            _ => {
                                log::debug!("unknown app IPC command: {cmd}");
                            }
                        }
                    }
                }
                // Client disconnected (reader.lines() ended).
                // Mark the connection as dead and notify the GUI to clean up
                // any stale pending permission requests or agent state.
                connection_alive.store(false, Ordering::SeqCst);
                if had_permission_request {
                    let _ = proxy.send_event(UserEvent::IpcClientDisconnected);
                }
            }
            Err(e) => {
                log::debug!("app IPC accept error: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let quick_terminal_mode = std::env::args().any(|a| a == "--quick-terminal");
    if quick_terminal_mode {
        log::info!("quick terminal mode enabled via --quick-terminal flag");
    }

    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    let proxy = event_loop.create_proxy();

    // Start app-side IPC listener for daemon commands (toggle_quick_terminal, etc.)
    {
        let proxy = proxy.clone();
        std::thread::Builder::new()
            .name("app-ipc-listener".into())
            .spawn(move || {
                app_ipc_listener(proxy);
            })
            .expect("failed to spawn app IPC listener");
    }

    // Start global hotkey monitor (Ctrl+`, Cmd+Shift+P, etc.) directly in the app.
    // Running inside the .app bundle lets macOS show the Accessibility permission
    // dialog and remember the grant by bundle ID.
    let _hotkey_handle = {
        use termojinal_session::hotkey::{GlobalHotkey, HotkeyEvent};
        let proxy = proxy.clone();
        match GlobalHotkey::start(move |event| {
            let user_event = match event {
                HotkeyEvent::QuickTerminal => UserEvent::ToggleQuickTerminal,
                HotkeyEvent::CommandPalette | HotkeyEvent::AllowFlowPanel => {
                    // These are handled via normal keybindings when the app is focused.
                    // Global hotkey for these is only useful via daemon when app is hidden.
                    return;
                }
            };
            let _ = proxy.send_event(user_event);
        }) {
            Ok(handle) => {
                log::info!("global hotkey monitor active (Ctrl+` for Quick Terminal)");
                Some(handle)
            }
            Err(e) => {
                log::warn!("global hotkey unavailable: {e}");
                None
            }
        }
    };

    let config = load_config();
    log::info!("config: font.size={}, window={}x{}", config.font.size, config.window.width, config.window.height);
    let mut app = App::new(proxy, config);
    app.quick_terminal_mode = quick_terminal_mode;

    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
    }

    // Clean up the app IPC socket on exit.
    let sock_path = app_ipc_socket_path();
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
        log::info!("removed app IPC socket at {}", sock_path.display());
    }
}
