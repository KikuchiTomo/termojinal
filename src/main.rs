//! jterm — GPU-accelerated multi-pane terminal emulator.

mod config;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use config::{color_or, format_tab_title, load_config, parse_hex_color, JtermConfig};

use jterm_ipc::keybinding::{Action, KeybindingConfig};
use jterm_layout::{Direction, LayoutTree, PaneId, SplitDirection};
use jterm_pty::{Pty, PtyConfig, PtySize};
use jterm_render::{FontConfig, Renderer};
use jterm_vt::{ClipboardEvent, MouseMode, Terminal};

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
    commands: Vec<PaletteCommand>,
    filtered: Vec<usize>, // Indices into commands
    selected: usize,      // Index into filtered
}

impl CommandPalette {
    fn new() -> Self {
        let commands = vec![
            PaletteCommand {
                name: "Split Right".to_string(),
                description: "Split pane horizontally".to_string(),
                action: Action::SplitRight,
            },
            PaletteCommand {
                name: "Split Down".to_string(),
                description: "Split pane vertically".to_string(),
                action: Action::SplitDown,
            },
            PaletteCommand {
                name: "Close Pane".to_string(),
                description: "Close the focused pane".to_string(),
                action: Action::CloseTab,
            },
            PaletteCommand {
                name: "New Tab".to_string(),
                description: "Open a new tab".to_string(),
                action: Action::NewTab,
            },
            PaletteCommand {
                name: "Zoom Pane".to_string(),
                description: "Toggle pane zoom".to_string(),
                action: Action::ZoomPane,
            },
            PaletteCommand {
                name: "Next Pane".to_string(),
                description: "Focus next pane".to_string(),
                action: Action::NextPane,
            },
            PaletteCommand {
                name: "Previous Pane".to_string(),
                description: "Focus previous pane".to_string(),
                action: Action::PrevPane,
            },
            PaletteCommand {
                name: "New Workspace".to_string(),
                description: "Create a new workspace".to_string(),
                action: Action::NewWorkspace,
            },
            PaletteCommand {
                name: "Next Tab".to_string(),
                description: "Switch to next tab".to_string(),
                action: Action::NextTab,
            },
            PaletteCommand {
                name: "Previous Tab".to_string(),
                description: "Switch to previous tab".to_string(),
                action: Action::PrevTab,
            },
            PaletteCommand {
                name: "Toggle Sidebar".to_string(),
                description: "Show/hide sidebar".to_string(),
                action: Action::ToggleSidebar,
            },
            PaletteCommand {
                name: "Copy".to_string(),
                description: "Copy selection to clipboard".to_string(),
                action: Action::Copy,
            },
            PaletteCommand {
                name: "Paste".to_string(),
                description: "Paste from clipboard".to_string(),
                action: Action::Paste,
            },
            PaletteCommand {
                name: "Search".to_string(),
                description: "Find in terminal".to_string(),
                action: Action::Search,
            },
        ];
        let filtered: Vec<usize> = (0..commands.len()).collect();
        Self {
            visible: false,
            input: String::new(),
            commands,
            filtered,
            selected: 0,
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
    }

    fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
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
}

impl Selection {
    /// Normalize so start <= end in reading order.
    fn ordered(&self) -> (GridPos, GridPos) {
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Extract selected text from the terminal grid.
    fn text(&self, grid: &jterm_vt::Grid) -> String {
        let (s, e) = self.ordered();
        let mut result = String::new();
        for row in s.row..=e.row {
            if row >= grid.rows() {
                break;
            }
            let col_start = if row == s.row { s.col } else { 0 };
            let col_end = if row == e.row {
                e.col + 1
            } else {
                grid.cols()
            };
            for col in col_start..col_end.min(grid.cols()) {
                let cell = grid.cell(col, row);
                if cell.width > 0 && cell.c != '\0' {
                    result.push(cell.c);
                }
            }
            if row != e.row {
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
    fn search(&mut self, grid: &jterm_vt::Grid) {
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
    tab_drag: Option<TabDrag>,
    config: JtermConfig,
    status_cache: StatusCache,
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

/// Update the display title of a tab based on the focused pane's OSC state.
fn update_tab_title(tab: &mut Tab, format: &str, tab_index: usize) {
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get(&focused_id) {
        let title = &pane.terminal.osc.title;
        let cwd = &pane.terminal.osc.cwd;
        let new_title = format_tab_title(format, title, cwd, tab_index);
        if new_title != tab.display_title {
            tab.display_title = new_title;
        }
    }
}

/// Compute the content area that excludes the tab bar, sidebar, and status bar.
/// Returns (content_x, content_y, content_w, content_h) in physical pixels.
fn content_area(state: &AppState, phys_w: f32, phys_h: f32) -> (f32, f32, f32, f32) {
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let tab_bar_h = if tab_bar_visible(state) { state.config.tab_bar.height } else { 0.0 };
    let status_bar_h = if state.config.status_bar.enabled {
        state.config.status_bar.height
    } else {
        0.0
    };
    let content_x = sidebar_w;
    let content_y = tab_bar_h;
    let content_w = (phys_w - sidebar_w).max(1.0);
    let content_h = (phys_h - tab_bar_h - status_bar_h).max(1.0);
    (content_x, content_y, content_w, content_h)
}

/// Get pane rects for the active tab of the active workspace, offset by tab bar + sidebar.
fn active_pane_rects(state: &AppState) -> Vec<(PaneId, jterm_layout::Rect)> {
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
    config: Option<JtermConfig>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, config: JtermConfig) -> Self {
        Self {
            state: None,
            proxy,
            pty_buffers: Arc::new(Mutex::new(HashMap::new())),
            config: Some(config),
        }
    }
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
) -> Result<Pane, jterm_pty::PtyError> {
    let config = PtyConfig {
        size: PtySize { cols, rows },
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

    Ok(Pane {
        id,
        terminal,
        vt_parser,
        pty,
        selection: None,
        preedit: None,
    })
}

// ---------------------------------------------------------------------------
// Keybinding string conversion
// ---------------------------------------------------------------------------

/// Convert a winit KeyEvent + modifiers into the keybinding string format
/// used by jterm-ipc (e.g., "cmd+d", "ctrl+c", "cmd+shift+enter").
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
            NamedKey::Enter => return Some(b"\r".to_vec()),
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
            .with_title("jterm")
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

        // Set renderer fields from config.
        renderer.bg_opacity = opacity;
        renderer.preedit_bg = color_or(&cfg.theme.preedit_bg, [0.15, 0.15, 0.20, 1.0]);
        renderer.scrollbar_thumb_opacity = cfg.pane.scrollbar_thumb_opacity;
        renderer.scrollbar_track_opacity = cfg.pane.scrollbar_track_opacity;

        let size = window.inner_size();
        let phys_w = size.width as f32;
        let phys_h = size.height as f32;
        // No tab bar for the initial single-tab workspace.
        let (cols, rows) = renderer.grid_size_raw(phys_w as u32, phys_h as u32);
        log::info!("window {}x{} -> grid {cols}x{rows}", size.width, size.height);

        // Create the initial pane (id 0) in the first workspace, first tab.
        let initial_id: PaneId = 0;
        let layout = LayoutTree::new(initial_id);

        let pane = match spawn_pane(initial_id, cols, rows, &self.proxy, &self.pty_buffers) {
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
            display_title: "Tab 1".to_string(),
        };

        let initial_workspace = Workspace {
            tabs: vec![initial_tab],
            active_tab: 0,
            name: "Workspace 1".to_string(),
        };

        let keybindings = KeybindingConfig::load();

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
            command_palette: CommandPalette::new(),
            font_size: cfg.font.size,
            search: None,
            workspace_infos: vec![WorkspaceInfo::new()],
            tab_drag: None,
            config: cfg.clone(),
            status_cache: StatusCache::new(),
        });

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
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                // Suppress raw key events during IME composition.
                let focused_id = active_tab(state).layout.focused();
                if active_tab(state).panes.get(&focused_id).map_or(false, |p| p.preedit.is_some()) {
                    return;
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

                // Clear selection on any keypress.
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    if pane.selection.is_some() {
                        pane.selection = None;
                        state.window.request_redraw();
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
                    let cursor_x = position.x as f32;
                    let cursor_y = position.y as f32;
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
                                let local_x = (cursor_x - rect.x).max(0.0);
                                let local_y = (cursor_y - rect.y).max(0.0);
                                let col = (local_x / cell_size.width) as usize;
                                let row = (local_y / cell_size.height) as usize;
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
                                    let local_x = (cursor_x - rect.x).max(0.0);
                                    let local_y = (cursor_y - rect.y).max(0.0);
                                    sel.end = GridPos {
                                        col: (local_x / cell_size.width) as usize,
                                        row: (local_y / cell_size.height) as usize,
                                    };
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
                        handle_sidebar_click(state);
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
                                        let ws = active_ws_mut(state);
                                        let tab_num = ws.tabs.len() + 1;
                                        let new_tab = Tab {
                                            layout: new_layout,
                                            panes: new_panes,
                                            name: format!("Tab {tab_num}"),
                                            display_title: format!("Tab {tab_num}"),
                                        };
                                        ws.tabs.push(new_tab);
                                        ws.active_tab = ws.tabs.len() - 1;
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
                            let local_x = (cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (cursor_pos.1 as f32 - rect.y).max(0.0);
                            let col = (local_x / cell_size.width) as usize;
                            let row = (local_y / cell_size.height) as usize;
                            let pressed = btn_state == ElementState::Pressed;
                            let seq = encode_mouse_sgr(btn_code, col, row, pressed);
                            let _ = pane.pty.write(&seq);
                        }
                    } else {
                        // Selection mode.
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let local_x = (cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (cursor_pos.1 as f32 - rect.y).max(0.0);
                            let pos = GridPos {
                                col: (local_x / cell_size.width) as usize,
                                row: (local_y / cell_size.height) as usize,
                            };

                            if button == MouseButton::Left {
                                match btn_state {
                                    ElementState::Pressed => {
                                        pane.selection = Some(Selection {
                                            start: pos,
                                            end: pos,
                                            active: true,
                                        });
                                    }
                                    ElementState::Released => {
                                        if let Some(ref mut sel) = pane.selection {
                                            sel.active = false;
                                            sel.end = pos;
                                            if sel.start == sel.end {
                                                pane.selection = None;
                                            }
                                        }
                                    }
                                }
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
                                    (cursor_pos.0 as f32 - rect.x).max(0.0);
                                let local_y =
                                    (cursor_pos.1 as f32 - rect.y).max(0.0);
                                let col = (local_x / cell_size.width) as usize;
                                let row = (local_y / cell_size.height) as usize;
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

            WindowEvent::RedrawRequested => {
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
                let mut lock = self.pty_buffers.lock().unwrap();
                let mut total = 0usize;
                let mut found_ws_idx: Option<usize> = None;
                if let Some(q) = lock.get_mut(&pane_id) {
                    'outer_feed: for (wi, ws) in state.workspaces.iter_mut().enumerate() {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
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

                // Update tab display titles after VT feed.
                if total > 0 {
                    let fmt = state.config.tab_bar.format.clone();
                    for ws in &mut state.workspaces {
                        for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                            if tab.panes.contains_key(&pane_id) {
                                update_tab_title(tab, &fmt, ti + 1);
                            }
                        }
                    }
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited(pane_id) => {
                log::info!("pane {pane_id}: shell exited");
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
                                if state.active_workspace >= state.workspaces.len() {
                                    state.active_workspace = state.workspaces.len() - 1;
                                }
                                resize_all_panes(state);
                                state.window.request_redraw();
                            }
                        } else {
                            ws.tabs.remove(tab_idx);
                            if ws.active_tab >= ws.tabs.len() {
                                ws.active_tab = ws.tabs.len() - 1;
                            }
                            resize_all_panes(state);
                            state.window.request_redraw();
                        }
                    }
                }
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
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
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
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
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
            state.window.request_redraw();
            true
        }
        Action::PrevPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Prev);
            state.window.request_redraw();
            true
        }
        Action::NewTab => {
            // Create a new tab in the current workspace.
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
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws = active_ws_mut(state);
                    let tab_num = ws.tabs.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: format!("Tab {tab_num}"),
                        display_title: format!("Tab {tab_num}"),
                    };
                    ws.tabs.push(tab);
                    ws.active_tab = ws.tabs.len() - 1;
                    resize_all_panes(state);
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
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws_num = state.workspaces.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: "Tab 1".to_string(),
                        display_title: "Tab 1".to_string(),
                    };
                    let ws = Workspace {
                        tabs: vec![tab],
                        active_tab: 0,
                        name: format!("Workspace {ws_num}"),
                    };
                    state.workspaces.push(ws);
                    state.active_workspace = state.workspaces.len() - 1;
                    resize_all_panes(state);
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
                state.window.request_redraw();
            }
            true
        }
        Action::PrevTab => {
            let ws = active_ws_mut(state);
            if ws.active_tab > 0 {
                ws.active_tab -= 1;
                resize_all_panes(state);
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
                state.window.request_redraw();
            }
            true
        }
        Action::CommandPalette => {
            state.command_palette.toggle();
            state.window.request_redraw();
            true
        }
        Action::Copy => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                if let Some(ref sel) = pane.selection {
                    let text = sel.text(pane.terminal.grid());
                    if !text.is_empty() {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            let _ = cb.set_text(&text);
                        }
                        pane.selection = None;
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
                state.window.request_redraw();
            }
            true
        }
        Action::PrevWorkspace => {
            if state.active_workspace > 0 {
                state.active_workspace -= 1;
                resize_all_panes(state);
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
        Action::AllowFlowPanel
        | Action::UnreadJump
        | Action::OpenSettings
        | Action::Command(_) => {
            log::debug!("unhandled action: {:?}", action);
            true
        }
    }
}

/// Resize all panes' terminals and PTYs to match current layout rects.
fn resize_all_panes(state: &mut AppState) {
    let pane_rects = active_pane_rects(state);
    for (pid, rect) in &pane_rects {
        let (cols, rows) =
            state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
        let cols = cols.max(1);
        let rows = rows.max(1);
        let tab = active_tab_mut(state);
        if let Some(pane) = tab.panes.get_mut(pid) {
            pane.terminal.resize(cols as usize, rows as usize);
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
    pane_rects: &[(PaneId, jterm_layout::Rect)],
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
                    if state.active_workspace >= state.workspaces.len() {
                        state.active_workspace = state.workspaces.len() - 1;
                    }
                    resize_all_panes(state);
                    state.window.request_redraw();
                }
            } else {
                let tab_idx = ws.active_tab;
                ws.tabs.remove(tab_idx);
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len() - 1;
                }
                resize_all_panes(state);
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
///   - 8px top padding
///   - Each workspace: name line + git info line + optional ports line + 12px gap
///   - Separator line (1px + 8px padding each side)
///   - "New Workspace" button
fn handle_sidebar_click(state: &mut AppState) {
    let cy = state.cursor_pos.1 as f32;
    let cell_h = state.renderer.cell_size().height;
    let top_pad = 8.0_f32;
    let entry_gap = 12.0_f32;
    let info_line_gap = 4.0_f32;

    let mut entry_y = top_pad;
    for (i, _ws) in state.workspaces.iter().enumerate() {
        let info = state.workspace_infos.get(i);
        // Name line
        let mut entry_h = cell_h;
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
        entry_h += entry_gap; // gap between entries

        if cy >= entry_y && cy < entry_y + entry_h {
            if state.active_workspace != i {
                state.active_workspace = i;
                if i < state.workspace_infos.len() {
                    state.workspace_infos[i].has_unread = false;
                }
                resize_all_panes(state);
                state.window.request_redraw();
            }
            return;
        }
        entry_y += entry_h;
    }

    // Check "\u{2295} New Workspace" entry (below separator).
    // Separator: 8px + 1px + 8px = 17px
    entry_y += 17.0;
    if cy >= entry_y && cy < entry_y + cell_h {
        // Trigger new workspace (same as Cmd+N).
        // Left for keybinding handler but we acknowledge the click area.
    }
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
    let bar_h = state.config.tab_bar.height as u32;
    let accent_h = state.config.tab_bar.accent_height;
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
    let text_y = tab_pad_y + (state.config.tab_bar.height - 2.0 * tab_pad_y - cell_h) / 2.0;

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
                tab_x + cell_w * 0.5,
                text_y.max(0.0),
                accent_color,
                bg,
            );
            cell_w * 2.0 // dot + space width
        } else {
            0.0
        };

        // Title text.
        let text_x = tab_x + cell_w + dot_offset;
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
            let close_x = tab_x + tab_w - 1.5 * cell_w;
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
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                4,          // 4px top margin
                1,          // 1px wide
                (bar_h - 8).max(1), // 4px top + 4px bottom margin
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
    let active_entry_bg = color_or(&sc.active_entry_bg, [0.102, 0.102, 0.141, 1.0]);
    let active_fg = color_or(&sc.active_fg, [0.95, 0.95, 0.97, 1.0]);
    let inactive_fg = color_or(&sc.inactive_fg, [0.55, 0.55, 0.60, 1.0]);
    let dim_fg = color_or(&sc.dim_fg, [0.40, 0.40, 0.44, 1.0]);
    let inactive_dot_color = dim_fg;
    let git_branch_fg = color_or(&sc.git_branch_fg, [0.35, 0.70, 0.85, 1.0]);
    let separator_color = color_or(&sc.separator_color, [0.20, 0.20, 0.22, 1.0]);
    let notification_dot = color_or(&sc.notification_dot, [1.0, 0.58, 0.26, 1.0]);
    let yellow_fg = color_or(&sc.git_dirty_color, [0.8, 0.7, 0.3, 1.0]);

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
    let text_left = side_pad + cell_w * 2.0; // space for dot + gap
    let max_chars = ((sidebar_w - text_left - side_pad) / cell_w).max(1.0) as usize;
    let num_workspaces = state.workspaces.len();
    let mut entry_y = top_pad;

    for i in 0..num_workspaces {
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

        // --- Active workspace: subtle highlight background ---
        let bg = if is_active { active_entry_bg } else { sidebar_bg };
        if is_active {
            let highlight_pad = 4.0;
            state.renderer.submit_separator(
                view,
                (side_pad - highlight_pad) as u32,
                (entry_y - highlight_pad) as u32,
                (sidebar_w - 2.0 * (side_pad - highlight_pad)) as u32,
                (content_h + 2.0 * highlight_pad) as u32,
                active_entry_bg,
            );
        }

        // --- Workspace indicator dot ---
        // Active: filled colored circle ●, Inactive: outline circle ○
        let dot_char = if is_active { "\u{25CF}" } else { "\u{25CB}" }; // ● or ○
        let dot_color = if is_active {
            ws_color
        } else if info.map_or(false, |inf| inf.has_unread) {
            notification_dot
        } else {
            inactive_dot_color
        };
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

        // --- Workspace name ---
        let display_name = if let Some(info) = info {
            if !info.name.is_empty() {
                info.name.clone()
            } else {
                state.workspaces[i].name.clone()
            }
        } else {
            state.workspaces[i].name.clone()
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
            let cwd_trimmed: String = cwd_display.chars().take(max_chars).collect();
            let indent = text_left + cell_w;
            state.renderer.render_text(view, &cwd_trimmed, indent, line_y, dim_fg, bg);
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

                let git_display: String = git_parts.chars().take(max_chars).collect();
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
                let indent = text_left + cell_w; // extra indent for sub-info
                state.renderer.render_text(view, &git_display, indent, line_y, git_fg, bg);
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
                let ports_display: String = ports_str.chars().take(max_chars).collect();
                let indent = text_left + cell_w;
                state.renderer.render_text(view, &ports_display, indent, line_y, dim_fg, bg);
            }
        }

        entry_y += content_h + entry_gap;
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
}

/// Build the `StatusContext` for the current frame by collecting variable values.
fn build_status_context(state: &mut AppState) -> StatusContext {
    let cache = &mut state.status_cache;
    let (time, date) = cache.time_date();
    let time = time.to_string();
    let date = date.to_string();
    let user = cache.user.clone();
    let host = cache.host.clone();
    let shell = cache.shell.clone();

    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let tab_idx = ws.active_tab;
    let tab = &ws.tabs[tab_idx];
    let focused_id = tab.layout.focused();

    // CWD from focused pane's OSC state.
    let cwd = tab
        .panes
        .get(&focused_id)
        .map(|p| p.terminal.osc.cwd.clone())
        .unwrap_or_default();
    let cwd_short = if let Ok(home) = std::env::var("HOME") {
        if cwd.starts_with(&home) {
            format!("~{}", &cwd[home.len()..])
        } else {
            cwd.clone()
        }
    } else {
        cwd.clone()
    };

    // Git info from WorkspaceInfo (already refreshed periodically).
    let info = state.workspace_infos.get(ws_idx);
    let git_branch = info
        .and_then(|i| i.git_branch.as_deref())
        .unwrap_or("")
        .to_string();
    let git_status = {
        let mut parts = Vec::new();
        if let Some(i) = info {
            if i.git_ahead > 0 {
                parts.push(format!("\u{21E1}{}", i.git_ahead));
            }
            if i.git_behind > 0 {
                parts.push(format!("\u{21E3}{}", i.git_behind));
            }
            if i.git_dirty > 0 {
                parts.push(format!("!{}", i.git_dirty));
            }
            if i.git_untracked > 0 {
                parts.push(format!("?{}", i.git_untracked));
            }
        }
        parts.join(" ")
    };

    // Ports from WorkspaceInfo.
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
    let pid = tab
        .panes
        .get(&focused_id)
        .map(|p| p.pty.pid().as_raw().to_string())
        .unwrap_or_default();

    // Pane size (cols x rows) of the focused pane.
    let pane_size = tab
        .panes
        .get(&focused_id)
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

    // Git remote — not cached in WorkspaceInfo; leave empty to avoid
    // spawning git commands every frame. Users can add it if they extend WorkspaceInfo.
    let git_remote = String::new();

    StatusContext {
        user,
        host,
        cwd,
        cwd_short,
        git_branch,
        git_status,
        git_remote,
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
    let bar_h = cfg.height;
    let bar_y = phys_h - bar_h;
    let bar_x = sidebar_w;
    let bar_w = (phys_w - sidebar_w).max(0.0);

    // Draw full status bar background.
    let status_bg = parse_hex_color(&cfg.background).unwrap_or([0.1, 0.1, 0.14, 1.0]);
    state.renderer.submit_separator(
        view,
        bar_x as u32,
        bar_y as u32,
        bar_w as u32,
        bar_h as u32,
        status_bg,
    );

    // Draw top border if enabled.
    if cfg.top_border {
        let border_color = color_or(&cfg.top_border_color, [
            status_bg[0] + 0.08,
            status_bg[1] + 0.08,
            status_bg[2] + 0.08,
            1.0,
        ]);
        state.renderer.submit_separator(
            view,
            bar_x as u32,
            bar_y as u32,
            bar_w as u32,
            1,
            border_color,
        );
    }

    let text_y = bar_y + (bar_h - cell_h) / 2.0;
    let pad_x = cfg.padding_x;

    // Render left-aligned segments.
    let mut cursor_x = bar_x + pad_x;
    for seg in &cfg.left {
        let expanded = expand_status_variables(&seg.content, &ctx);
        if segment_is_empty(&expanded) {
            continue;
        }
        let fg = parse_hex_color(&seg.fg).unwrap_or([0.8, 0.8, 0.8, 1.0]);
        let bg = parse_hex_color(&seg.bg).unwrap_or(status_bg);
        let seg_w = expanded.len() as f32 * cell_w;

        // Draw segment background.
        state.renderer.submit_separator(
            view,
            cursor_x as u32,
            bar_y as u32,
            seg_w as u32,
            bar_h as u32,
            bg,
        );

        // Draw segment text.
        state.renderer.render_text(view, &expanded, cursor_x, text_y, fg, bg);
        cursor_x += seg_w;
    }

    // Render right-aligned segments (compute total width first, then draw right-to-left).
    let right_segments: Vec<(String, [f32; 4], [f32; 4])> = cfg
        .right
        .iter()
        .filter_map(|seg| {
            let expanded = expand_status_variables(&seg.content, &ctx);
            if segment_is_empty(&expanded) {
                return None;
            }
            let fg = parse_hex_color(&seg.fg).unwrap_or([0.8, 0.8, 0.8, 1.0]);
            let bg = parse_hex_color(&seg.bg).unwrap_or(status_bg);
            Some((expanded, fg, bg))
        })
        .collect();

    let total_right_w: f32 = right_segments
        .iter()
        .map(|(text, _, _)| text.len() as f32 * cell_w)
        .sum();

    let mut right_x = bar_x + bar_w - total_right_w - pad_x;
    for (text, fg, bg) in &right_segments {
        let seg_w = text.len() as f32 * cell_w;

        // Draw segment background.
        state.renderer.submit_separator(
            view,
            right_x as u32,
            bar_y as u32,
            seg_w as u32,
            bar_h as u32,
            *bg,
        );

        // Draw segment text.
        state.renderer.render_text(view, text, right_x, text_y, *fg, *bg);
        right_x += seg_w;
    }
}

/// Render all panes with tab bar and sidebar.
fn render_frame(state: &mut AppState) -> Result<(), jterm_render::RenderError> {
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
        let status_h = if state.config.status_bar.enabled { state.config.status_bar.height } else { 0.0 };
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

    // Render command palette overlay if visible.
    if state.command_palette.visible {
        render_command_palette(state, &view, phys_w, phys_h);
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

/// Render the command palette as an overlay on top of the terminal.
fn render_command_palette(
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
    let rows_needed = 1 + visible_items; // input row + command rows
    let box_h = ((rows_needed as f32) * cell_h + cell_h).min(max_box_h); // extra padding
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background from config.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    state.renderer.submit_separator(
        view,
        box_x as u32,
        box_y as u32,
        box_w as u32,
        box_h as u32,
        box_bg,
    );

    // Draw a subtle border around the box.
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let b = 1u32;
    let bx = box_x as u32;
    let by = box_y as u32;
    let bw = box_w as u32;
    let bh = box_h as u32;
    state.renderer.submit_separator(view, bx, by, bw, b, border_color); // top
    state.renderer.submit_separator(view, bx, by + bh - b, bw, b, border_color); // bottom
    state.renderer.submit_separator(view, bx, by, b, bh, border_color); // left
    state.renderer.submit_separator(view, bx + bw - b, by, b, bh, border_color); // right

    // 3. Input field at the top of the box.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let prompt = format!("> {}", state.command_palette.input);
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

    for (i, &cmd_idx) in state
        .command_palette
        .filtered
        .iter()
        .enumerate()
        .take(max_visible_items)
    {
        let item_y = list_start_y + (i as f32) * cell_h;
        if item_y + cell_h > box_y + box_h {
            break;
        }

        let is_selected = i == state.command_palette.selected;
        let bg = if is_selected { selected_bg } else { box_bg };

        // Highlight selected row.
        if is_selected {
            state.renderer.submit_separator(
                view,
                box_x as u32 + 1,
                item_y as u32,
                box_w as u32 - 2,
                cell_h as u32,
                selected_bg,
            );
        }

        let cmd = &state.command_palette.commands[cmd_idx];

        // Render name in brighter color, description in dimmer color.
        let name_display: String = cmd.name.chars().take(max_chars).collect();
        let fg = if is_selected { palette_input_fg } else { cmd_fg };
        state.renderer.render_text(
            view,
            &name_display,
            input_x,
            item_y,
            fg,
            bg,
        );

        // Render description after the name.
        let desc_offset = name_display.len() as f32 * cell_w + 2.0 * cell_w;
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

fn sel_bounds_for(pane: &Pane) -> Option<((usize, usize), (usize, usize))> {
    match &pane.selection {
        Some(s) if s.start != s.end => {
            let (start, end) = s.ordered();
            Some(((start.col, start.row), (end.col, end.row)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// macOS window transparency
// ---------------------------------------------------------------------------

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
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    let proxy = event_loop.create_proxy();
    let config = load_config();
    log::info!("config: font.size={}, window={}x{}", config.font.size, config.window.width, config.window.height);
    let mut app = App::new(proxy, config);

    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
    }
}
