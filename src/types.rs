//! Core type definitions.

use crate::config::TermojinalConfig;
use crate::allow_flow;
use crate::command_ui::CommandExecution;
use crate::status::{AsyncStatusCollector, PaneGitCache, StatusCache};
use crate::workspace::{AgentSessionInfo, AsyncWorkspaceRefresher, DaemonSessionInfo, WorkspaceInfo};
// DaemonSessionInfo is defined in workspace.rs
use crate::dir_tree::DirectoryTreeState;
use crate::palette::{CommandPalette, QuickLaunchState, UpdateChecker};
use crate::quick_terminal::QuickTerminalState;
use crate::ClaudesDashboard;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use termojinal_ipc::app_protocol::{AppIpcRequest, AppIpcResponse};
use termojinal_ipc::command_loader::LoadedCommand;
use termojinal_ipc::keybinding::KeybindingConfig;
use termojinal_layout::{LayoutTree, PaneId, SplitDirection};
use termojinal_render::Renderer;
use termojinal_vt::Terminal;
use winit::keyboard::ModifiersState;
use winit::window::Window;

pub(crate) enum UserEvent {
    PtyOutput(PaneId),
    PtyExited(PaneId),
    /// Snapshot data received from daemon on re-attach.
    SnapshotReceived(PaneId, Vec<u8>),
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
pub(crate) struct GridPos {
    pub(crate) col: usize,
    pub(crate) row: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Selection {
    pub(crate) start: GridPos,
    pub(crate) end: GridPos,
    pub(crate) active: bool, // mouse is still held
    /// The terminal scroll_offset at the time each endpoint was recorded.
    /// Stored so we can convert screen-relative rows to absolute positions
    /// that survive scrolling. Absolute row = screen_row - scroll_offset
    /// (invariant under scrolling). At render time we convert back with
    /// screen_row = abs_row + current_scroll_offset.
    pub(crate) scroll_offset_at_start: usize,
    pub(crate) scroll_offset_at_end: usize,
}

impl Selection {
    /// Absolute row for an endpoint: invariant under scrolling.
    /// Computed as screen_row - scroll_offset (can be negative).
    pub(crate) fn abs_row_start(&self) -> isize {
        self.start.row as isize - self.scroll_offset_at_start as isize
    }
    pub(crate) fn abs_row_end(&self) -> isize {
        self.end.row as isize - self.scroll_offset_at_end as isize
    }

    /// Normalize so start <= end in reading order, using absolute rows.
    /// Returns ((col, abs_row), (col, abs_row)).
    pub(crate) fn ordered_abs(&self) -> ((usize, isize), (usize, isize)) {
        let abs_s = self.abs_row_start();
        let abs_e = self.abs_row_end();
        if abs_s < abs_e || (abs_s == abs_e && self.start.col <= self.end.col) {
            ((self.start.col, abs_s), (self.end.col, abs_e))
        } else {
            ((self.end.col, abs_e), (self.start.col, abs_s))
        }
    }

    /// Whether start and end refer to the same cell (using absolute coords).
    pub(crate) fn is_empty(&self) -> bool {
        self.abs_row_start() == self.abs_row_end() && self.start.col == self.end.col
    }

    /// Extract selected text from the terminal.
    /// Uses absolute row coordinates to read the correct cells even when
    /// the selection spans scrollback content.
    pub(crate) fn text(&self, terminal: &termojinal_vt::Terminal) -> String {
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

    /// Extract selected cells with their color/formatting attributes.
    /// Returns a vector of rows, where each row is a vector of `(char, Cell)` tuples.
    /// Used for rich-text (RTF) copy with colors preserved.
    pub(crate) fn cells(&self, terminal: &termojinal_vt::Terminal) -> Vec<Vec<termojinal_vt::Cell>> {
        let ((sc, abs_sr), (ec, abs_er)) = self.ordered_abs();
        let grid = terminal.grid();
        let cols = grid.cols();
        let grid_rows = grid.rows() as isize;
        let mut rows: Vec<Vec<termojinal_vt::Cell>> = Vec::new();

        for abs_row in abs_sr..=abs_er {
            let col_start = if abs_row == abs_sr { sc } else { 0 };
            let col_end = if abs_row == abs_er { ec + 1 } else { cols };
            let mut row_cells: Vec<termojinal_vt::Cell> = Vec::new();

            if abs_row < 0 {
                let sb_idx = (-abs_row - 1) as usize;
                if let Some(cells) = terminal.scrollback_row(sb_idx) {
                    for col in col_start..col_end.min(cells.len()) {
                        let cell = cells[col];
                        if cell.width > 0 && cell.c != '\0' {
                            row_cells.push(cell);
                        }
                    }
                }
            } else if abs_row < grid_rows {
                let grow = abs_row as usize;
                for col in col_start..col_end.min(cols) {
                    let cell = *grid.cell(col, grow);
                    if cell.width > 0 && cell.c != '\0' {
                        row_cells.push(cell);
                    }
                }
            } else {
                break;
            }

            // Trim trailing spaces.
            while row_cells.last().map_or(false, |c| {
                c.c == ' '
                    && c.fg == termojinal_vt::Color::Default
                    && c.bg == termojinal_vt::Color::Default
            }) {
                row_cells.pop();
            }
            rows.push(row_cells);
        }
        rows
    }
}

// ---------------------------------------------------------------------------
// Search state (Feature 5: Cmd+F)
// ---------------------------------------------------------------------------

pub(crate) struct SearchState {
    pub(crate) query: String,
    /// Matches: (row, col_start, col_end) - col_end is inclusive.
    pub(crate) matches: Vec<(usize, usize, usize)>,
    /// Index into matches for the current match.
    pub(crate) current: usize,
}

impl SearchState {
    pub(crate) fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn search(&mut self, grid: &termojinal_vt::Grid) {
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
            let row_lower: Vec<char> = row_chars
                .iter()
                .map(|c| {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    s.to_lowercase().chars().next().unwrap_or(*c)
                })
                .collect();
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

    pub(crate) fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + 1) % self.matches.len();
        }
    }

    pub(crate) fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace info for rich sidebar (Feature 3)
// ---------------------------------------------------------------------------


pub(crate) struct DragResize {
    /// Which direction the separator runs (Horizontal = vertical separator, etc.)
    pub(crate) direction: SplitDirection,
    /// The pane on the "first" side of the separator (used for resize calculation)
    pub(crate) pane_id: PaneId,
    /// Last mouse position along the drag axis
    pub(crate) last_pos: f64,
}

// ---------------------------------------------------------------------------
// Tab bar drag state (Feature 4: tab reordering)
// ---------------------------------------------------------------------------

pub(crate) struct TabDrag {
    /// Index of the tab being dragged.
    pub(crate) tab_idx: usize,
    /// Mouse x position when drag started.
    pub(crate) start_x: f64,
}

/// Pending tab click state: records a mouse-down on a tab before we know
/// whether it will become a click (tab switch) or a drag.
pub(crate) struct PendingTabClick {
    /// Index of the tab that was pressed.
    pub(crate) tab_idx: usize,
    /// Mouse position when mouse-down occurred.
    pub(crate) start_x: f64,
    pub(crate) start_y: f64,
}

// ---------------------------------------------------------------------------
// Drop zone for tab-to-pane drag
// ---------------------------------------------------------------------------

/// Which side of a pane the dragged tab will be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropZone {
    Top,
    Bottom,
    Left,
    Right,
}

/// State for a tab being dragged into a pane area (pane split mode).
pub(crate) struct TabPaneDrag {
    /// Index of the tab being dragged (in the active workspace).
    pub(crate) tab_idx: usize,
    /// Target pane ID under the cursor.
    pub(crate) target_pane: PaneId,
    /// Which drop zone the cursor is in.
    pub(crate) zone: DropZone,
}

// ---------------------------------------------------------------------------
// Scrollbar drag state
// ---------------------------------------------------------------------------

pub(crate) struct ScrollbarDrag {
    /// The pane whose scrollbar is being dragged.
    pub(crate) pane_id: PaneId,
    /// Offset in pixels from the top of the thumb to the mouse position at drag start.
    pub(crate) grab_offset_px: f32,
    /// The pane rect (viewport) at drag start, for coordinate conversion.
    pub(crate) pane_rect: termojinal_layout::Rect,
}

// ---------------------------------------------------------------------------
// Pane — holds per-pane terminal + PTY state
// ---------------------------------------------------------------------------

pub(crate) struct Pane {
    #[allow(dead_code)]
    pub(crate) id: PaneId,
    pub(crate) terminal: Terminal,
    pub(crate) vt_parser: vte::Parser,
    /// Daemon session ID (PTY is owned by the daemon).
    pub(crate) session_id: String,
    /// The shell command used to spawn this pane's PTY (e.g. `/bin/zsh`).
    pub(crate) shell: String,
    /// Shell PID (reported by daemon on session creation).
    pub(crate) shell_pid: i32,
    /// Channel for sending key input and resize commands to the daemon reader thread.
    #[allow(dead_code)]
    pub(crate) write_tx: std::sync::mpsc::Sender<termojinal_ipc::daemon_connection::WriteCommand>,
    pub(crate) selection: Option<Selection>,
    pub(crate) preedit: Option<String>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        // Send shutdown to the daemon writer thread, which closes the socket
        // and causes the reader thread to exit. This removes the client from
        // the daemon's session clients list so is_attached() returns false.
        let _ = self.write_tx.send(
            termojinal_ipc::daemon_connection::WriteCommand::Shutdown,
        );
    }
}

// ---------------------------------------------------------------------------
// Tab — a single tab within a workspace, containing a layout tree + panes
// ---------------------------------------------------------------------------

pub(crate) struct Tab {
    pub(crate) layout: LayoutTree,
    pub(crate) panes: HashMap<PaneId, Pane>,
    #[allow(dead_code)]
    pub(crate) name: String,
    /// Computed display title (from OSC title, CWD, or fallback).
    pub(crate) display_title: String,
}

// ---------------------------------------------------------------------------
// Workspace — contains multiple tabs
// ---------------------------------------------------------------------------

pub(crate) struct Workspace {
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_tab: usize,
    pub(crate) name: String,
}

// ---------------------------------------------------------------------------
// AppState — the single top-level struct
// ---------------------------------------------------------------------------

pub(crate) struct AppState {
    pub(crate) window: Arc<Window>,
    pub(crate) renderer: Renderer,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) active_workspace: usize,
    pub(crate) keybindings: KeybindingConfig,
    pub(crate) modifiers: ModifiersState,
    pub(crate) cursor_pos: (f64, f64),
    pub(crate) drag_resize: Option<DragResize>,
    pub(crate) scrollbar_drag: Option<ScrollbarDrag>,
    pub(crate) next_pane_id: PaneId,
    pub(crate) sidebar_visible: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) sidebar_drag: bool,
    pub(crate) command_palette: CommandPalette,
    pub(crate) font_size: f32,
    pub(crate) search: Option<SearchState>,
    pub(crate) workspace_infos: Vec<WorkspaceInfo>,
    /// Per-workspace AI agent session info for sidebar display.
    pub(crate) agent_infos: Vec<AgentSessionInfo>,
    /// Application start time for animation calculations.
    pub(crate) app_start_time: std::time::Instant,
    pub(crate) tab_drag: Option<TabDrag>,
    /// Link hover highlight: (row, col_start, col_end) inclusive, shown when modifier+hover over a link.
    pub(crate) link_hover_cells: Option<(usize, usize, usize)>,
    /// Pending tab click: mouse-down recorded but not yet committed as click or drag.
    pub(crate) pending_tab_click: Option<PendingTabClick>,
    /// Tab-to-pane drag state: active when a tab is being dragged into a pane area.
    pub(crate) tab_pane_drag: Option<TabPaneDrag>,
    pub(crate) config: TermojinalConfig,
    pub(crate) status_cache: StatusCache,
    /// Per-pane git info cache (updated from async collector).
    pub(crate) pane_git_cache: PaneGitCache,
    /// Background thread that collects git/SSH/CWD info without blocking render.
    pub(crate) status_collector: AsyncStatusCollector,
    /// Background thread that refreshes workspace info (git, ports, daemon sessions).
    pub(crate) workspace_refresher: AsyncWorkspaceRefresher,
    /// Background thread that detects and monitors Claude Code sessions.
    pub(crate) claude_monitor: termojinal_claude::monitor::ClaudeSessionMonitor,
    /// Current display scale factor (e.g. 2.0 for Retina, 1.0 for FHD).
    pub(crate) scale_factor: f64,
    /// Allow Flow UI state for AI agent permission management.
    pub(crate) allow_flow: allow_flow::AllowFlowUI,
    /// Deferred IPC responses for PermissionRequest hooks.
    /// Maps AllowFlow request ID → sender to reply when user decides.
    pub(crate) pending_ipc_responses: HashMap<u64, (std_mpsc::Sender<AppIpcResponse>, Arc<AtomicBool>)>,
    /// Maps Claude Code session IDs to workspace indices so that IPC requests
    /// are routed to the correct workspace even when the user has switched away.
    pub(crate) session_to_workspace: HashMap<String, usize>,
    /// Active command execution (None when showing the palette action list).
    pub(crate) command_execution: Option<CommandExecution>,
    /// Loaded external commands (cached at startup).
    pub(crate) external_commands: Vec<LoadedCommand>,
    /// Quick Terminal runtime state.
    pub(crate) quick_terminal: QuickTerminalState,
    /// Whether the "About Termojinal" overlay is visible.
    pub(crate) about_visible: bool,
    /// Scroll offset for the about overlay content.
    pub(crate) about_scroll: usize,
    /// Per-workspace directory tree states.
    pub(crate) dir_trees: Vec<DirectoryTreeState>,
    // -- Time Travel state --
    /// Whether the command timeline overlay is visible.
    pub(crate) timeline_visible: bool,
    /// Filter input for the timeline overlay.
    pub(crate) timeline_input: String,
    /// Currently selected item in the timeline.
    pub(crate) timeline_selected: usize,
    /// Scroll offset for the timeline list.
    pub(crate) timeline_scroll_offset: usize,
    /// S4: Pane ID the timeline was opened for (prevents switching on pane focus change).
    pub(crate) timeline_pane_id: Option<PaneId>,
    /// Whether the Claudes summary panel in the sidebar is collapsed.
    pub(crate) claudes_collapsed: bool,
    /// Claudes Dashboard (multi-agent overlay) state.
    pub(crate) claudes_dashboard: ClaudesDashboard,
    /// Whether the sessions summary panel in the sidebar is collapsed.
    #[allow(dead_code)]
    pub(crate) sessions_collapsed: bool,
    /// Cached session list from the daemon (refreshed by background thread).
    pub(crate) daemon_sessions: Vec<DaemonSessionInfo>,
    /// Pending close confirmation: pane has running child process.
    /// Stores (process_name, pane_id) to ensure the correct pane is closed.
    pub(crate) pending_close_confirm: Option<(String, PaneId)>,
    /// Set to `true` during RedrawRequested when continuous animation is needed
    /// (pulse indicator, command polling, etc.).  Consumed in `about_to_wait`
    /// to schedule the next frame via `WaitUntil` instead of an immediate
    /// `request_redraw()`, which would otherwise spin the event loop at 100% CPU.
    pub(crate) needs_animation_frame: bool,
    /// Timestamp of the last animation-driven redraw, used to throttle
    /// continuous redraws to ~30 fps.
    pub(crate) last_animation_redraw: std::time::Instant,
    /// Accumulated fractional scroll delta (pixels) from trackpad.
    /// Trackpad sends many small PixelDelta events that individually round to
    /// 0 lines.  We accumulate until a full line is reached.
    pub(crate) scroll_accum: f64,
    /// Whether auto-scroll during drag selection is active (cursor outside viewport).
    pub(crate) selection_auto_scroll: Option<i32>,
    /// Quick Launch overlay state.
    pub(crate) quick_launch: QuickLaunchState,
    /// Session Picker overlay state (for attach-or-new on Cmd+T / split).
    pub(crate) session_picker: crate::session_picker::SessionPicker,
    /// Homebrew update checker state.
    pub(crate) update_checker: UpdateChecker,
    /// Shared result from background brew update check thread.
    pub(crate) update_check_result: Arc<Mutex<Option<String>>>,
    /// Global shutdown flag — set to true when window closes.
    /// Background threads check this to exit their loops.
    pub(crate) shutdown: Arc<AtomicBool>,
}

/// Update session_to_workspace mapping after a workspace at `removed_idx` is removed.
/// Removes entries pointing to the removed workspace and decrements indices above it.
pub(crate) fn cleanup_session_to_workspace(state: &mut AppState, removed_idx: usize) {
    state
        .session_to_workspace
        .retain(|_, idx| *idx != removed_idx);
    for idx in state.session_to_workspace.values_mut() {
        if *idx > removed_idx {
            *idx -= 1;
        }
    }
}
