//! Terminal state machine implementing the VT parser's `Perform` trait.

use crate::cell::{Attrs, Cell, Pen};
use crate::color::{Color, NamedColor};
use crate::grid::Grid;
use crate::image::{
    self, ApcExtractor, ImageStore, KittyAccumulator,
};
use crate::scrollback::ScrollbackBuffer;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

/// DCS accumulation mode for Sixel graphics.
#[derive(Debug, PartialEq)]
enum DcsMode {
    /// Not accumulating DCS data.
    None,
    /// Accumulating Sixel data (DCS with `q` final character).
    Sixel,
}

/// Cursor shape (DECSCUSR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    BlinkingBlock,
    BlinkingUnderline,
    BlinkingBar,
}

impl Default for CursorShape {
    fn default() -> Self {
        Self::BlinkingBlock
    }
}

/// Saved cursor state (DECSC/DECRC).
#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    col: usize,
    row: usize,
    pen: Pen,
}

/// Which mouse events to report to the application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    None,
    /// Mode 1000 — report button press/release.
    Click,
    /// Mode 1002 — report motion while button held.
    ButtonMotion,
    /// Mode 1003 — report all motion.
    AnyMotion,
}

/// How mouse coordinates are encoded in reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MouseFormat {
    #[default]
    /// Default legacy format.
    X10,
    /// Mode 1005 — UTF-8 encoding.
    Utf8,
    /// Mode 1006 — modern SGR format ESC[<btn;col;row;M/m.
    Sgr,
    /// Mode 1015 — urxvt format.
    Urxvt,
}

/// Clipboard event produced by OSC 52 sequences.
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    /// Set clipboard contents. `data` is already decoded from base64.
    Set { selection: String, data: String },
    /// Query clipboard contents.
    Query { selection: String },
}

/// Terminal mode flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modes {
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
    pub auto_wrap: bool,
    pub origin_mode: bool,
    pub insert_mode: bool,
    pub cursor_visible: bool,
    /// Which mouse events to report.
    pub mouse_mode: MouseMode,
    /// How to encode mouse coordinates.
    pub mouse_format: MouseFormat,
    /// Whether focus in/out events are reported (mode 1004).
    pub focus_events: bool,
}

/// OSC-derived state.
#[derive(Debug, Clone, Default)]
pub struct OscState {
    pub title: String,
    pub cwd: String,
    /// Raw OSC 7 URI (e.g. `file://user@host/path`), preserved for extracting user/host.
    pub cwd_uri: String,
    pub last_notification: Option<String>,
    /// OSC 133 shell integration state.
    pub prompt_start: Option<(usize, usize)>,
    pub command_start: Option<(usize, usize)>,
}

/// A structured record of a single shell command and its output region.
///
/// Built from OSC 133 shell integration markers (A/B/C/D).
/// Enables command-level navigation, timeline UI, and session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    /// Monotonically increasing command ID within a session.
    pub id: u64,
    /// The command text entered by the user (extracted from grid between B and C markers).
    pub command_text: String,
    /// Working directory at the time the command was executed (from OSC 7).
    pub cwd: String,
    /// When the command started executing (OSC 133 C received).
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Duration in milliseconds (computed when OSC 133 D is received).
    pub duration_ms: Option<u64>,
    /// Exit code (parsed from OSC 133 D parameter).
    pub exit_code: Option<i32>,
    /// Absolute scrollback line where command output begins.
    pub scrollback_line_start: usize,
    /// Absolute scrollback line where command output ends (set when next prompt starts).
    pub scrollback_line_end: Option<usize>,
    /// Absolute scrollback line of the prompt for this command.
    pub prompt_line: usize,
}

/// Transient state accumulated between OSC 133 markers while a command is in progress.
#[derive(Debug, Clone)]
struct PendingCommand {
    /// Absolute line of the prompt (OSC 133 A).
    prompt_abs_line: usize,
    /// Absolute line where command input starts (OSC 133 B).
    command_start_abs_line: usize,
    /// Column where command input starts (OSC 133 B).
    command_start_col: usize,
    /// Working directory at command start.
    cwd: String,
    /// When the command was executed (OSC 133 C).
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Absolute line where output begins (OSC 133 C).
    output_start_abs_line: Option<usize>,
}

/// A serializable snapshot of the terminal state, used for session persistence
/// and named snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    /// Main grid cells (row-major).
    pub grid_cells: Vec<Vec<Cell>>,
    /// Cursor position and shape.
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_shape: CursorShape,
    /// Current pen style.
    pub pen: Pen,
    /// Terminal dimensions.
    pub cols: usize,
    pub rows: usize,
    /// Command history records.
    pub command_history: VecDeque<CommandRecord>,
    /// Total scrolled lines counter (for absolute line tracking).
    pub total_scrolled_lines: usize,
    /// Window/tab title.
    pub title: String,
    /// Current working directory.
    pub cwd: String,
}

/// A named snapshot that can be saved and restored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedSnapshot {
    /// User-assigned name for this snapshot.
    pub name: String,
    /// When the snapshot was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The terminal state at snapshot time.
    pub terminal_snapshot: TerminalSnapshot,
}

/// The full terminal state.
pub struct Terminal {
    /// Main screen grid.
    main_grid: Grid,
    /// Alternate screen grid.
    alt_grid: Grid,
    /// Whether we're on the alternate screen.
    using_alt: bool,
    /// Cursor position.
    pub cursor_col: usize,
    pub cursor_row: usize,
    /// Cursor shape.
    pub cursor_shape: CursorShape,
    /// Current pen (attributes for new characters).
    pen: Pen,
    /// Scroll region top (inclusive).
    scroll_top: usize,
    /// Scroll region bottom (inclusive).
    scroll_bottom: usize,
    /// Saved cursor (main screen).
    saved_cursor_main: Option<SavedCursor>,
    /// Saved cursor (alt screen).
    saved_cursor_alt: Option<SavedCursor>,
    /// Terminal modes.
    pub modes: Modes,
    /// OSC-derived state.
    pub osc: OscState,
    /// Terminal dimensions.
    cols: usize,
    rows: usize,
    /// Pending wrap: if true, next printable char triggers a newline first.
    wrap_pending: bool,
    /// Tab stops.
    tab_stops: Vec<bool>,
    /// Scrollback buffer with hot (in-memory) and warm (mmap) tiers.
    scrollback: ScrollbackBuffer,
    /// Current scroll offset (0 = live view, >0 = looking at history).
    scroll_offset: usize,
    /// Kitty keyboard protocol flags stack.
    kitty_keyboard_flags: Vec<u32>,
    /// Current hyperlink URI (set by OSC 8).
    current_hyperlink: Option<String>,
    /// Clipboard event from OSC 52 (consumed by application layer).
    pub clipboard_event: Option<ClipboardEvent>,
    /// Image store for Kitty, iTerm2, and Sixel image protocols.
    pub image_store: ImageStore,
    /// APC sequence extractor (Kitty Graphics uses APC, which vte ignores).
    apc_extractor: ApcExtractor,
    /// Kitty Graphics chunked transfer accumulator.
    kitty_accumulator: KittyAccumulator,
    /// DCS data accumulation buffer (for Sixel).
    dcs_data: Vec<u8>,
    /// Current DCS accumulation mode.
    dcs_mode: DcsMode,
    /// Completed command records (time travel history).
    command_history: VecDeque<CommandRecord>,
    /// Next command ID to assign.
    next_command_id: u64,
    /// Command currently being tracked between OSC 133 markers.
    pending_command: Option<PendingCommand>,
    /// Total number of lines that have been scrolled into the scrollback buffer.
    /// Used to compute absolute line numbers: abs_line = total_scrolled_lines + screen_row.
    total_scrolled_lines: usize,
    /// Whether command history tracking is enabled (config: time_travel.command_history).
    command_history_enabled: bool,
    /// Maximum number of command records to keep.
    max_command_history: usize,
    /// Temporary storage for command text extracted at OSC 133 C, used at D.
    pending_command_text: Option<String>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut tab_stops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            tab_stops[i] = true;
        }

        Self {
            main_grid: Grid::new(cols, rows),
            alt_grid: Grid::new(cols, rows),
            using_alt: false,
            cursor_col: 0,
            cursor_row: 0,
            cursor_shape: CursorShape::default(),
            pen: Pen::default(),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            saved_cursor_main: None,
            saved_cursor_alt: None,
            modes: Modes {
                auto_wrap: true,
                cursor_visible: true,
                ..Modes::default()
            },
            osc: OscState::default(),
            cols,
            rows,
            wrap_pending: false,
            tab_stops,
            scrollback: ScrollbackBuffer::with_defaults(),
            scroll_offset: 0,
            kitty_keyboard_flags: Vec::new(),
            current_hyperlink: None,
            clipboard_event: None,
            image_store: ImageStore::new(),
            apc_extractor: ApcExtractor::new(),
            kitty_accumulator: KittyAccumulator::new(),
            dcs_data: Vec::new(),
            dcs_mode: DcsMode::None,
            command_history: VecDeque::new(),
            next_command_id: 0,
            pending_command: None,
            total_scrolled_lines: 0,
            command_history_enabled: true,
            max_command_history: 10_000,
            pending_command_text: None,
        }
    }

    /// Get the active grid.
    pub fn grid(&self) -> &Grid {
        if self.using_alt {
            &self.alt_grid
        } else {
            &self.main_grid
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.using_alt {
            &mut self.alt_grid
        } else {
            &mut self.main_grid
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Number of lines in the scrollback buffer.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Current scroll offset (0 = live view).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set the scroll offset, clamped to scrollback length.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback.len());
    }

    // -----------------------------------------------------------------------
    // Command history (Time Travel)
    // -----------------------------------------------------------------------

    /// Compute the current absolute line number for a given screen row.
    fn abs_line(&self, screen_row: usize) -> usize {
        self.total_scrolled_lines + screen_row
    }

    /// Get the full command history.
    pub fn command_history(&self) -> &VecDeque<CommandRecord> {
        &self.command_history
    }

    /// Total scrolled lines (for converting between absolute and relative positions).
    pub fn total_scrolled_lines(&self) -> usize {
        self.total_scrolled_lines
    }

    /// Enable or disable command history tracking.
    pub fn set_command_history_enabled(&mut self, enabled: bool) {
        self.command_history_enabled = enabled;
    }

    /// Set maximum command history size.
    pub fn set_max_command_history(&mut self, max: usize) {
        self.max_command_history = max;
    }

    /// Convert an absolute line number to a scroll_offset.
    fn abs_line_to_scroll_offset(&self, abs_line: usize) -> usize {
        if abs_line >= self.total_scrolled_lines {
            0
        } else {
            let scrollback_idx = self.total_scrolled_lines - 1 - abs_line;
            scrollback_idx + 1
        }
    }

    /// Compute the absolute line at the current viewport top.
    fn viewport_top_abs(&self) -> usize {
        if self.scroll_offset == 0 {
            self.total_scrolled_lines + self.rows
        } else {
            self.total_scrolled_lines.saturating_sub(self.scroll_offset)
        }
    }

    /// Binary-search: find the last command whose prompt_line < target (S2).
    fn find_prev_command_idx(&self, target_abs: usize) -> Option<usize> {
        if self.command_history.is_empty() {
            return None;
        }
        let pos = self.command_history.partition_point(|cmd| cmd.prompt_line < target_abs);
        if pos == 0 { None } else { Some(pos - 1) }
    }

    /// Binary-search: find the first command whose prompt_line > target (S2).
    fn find_next_command_idx(&self, target_abs: usize) -> Option<usize> {
        let pos = self.command_history.partition_point(|cmd| cmd.prompt_line <= target_abs);
        if pos < self.command_history.len() { Some(pos) } else { None }
    }

    /// Jump to the previous command's output from the current scroll position.
    pub fn jump_to_prev_command(&mut self) -> Option<&CommandRecord> {
        let current_abs = self.viewport_top_abs();
        let idx = self.find_prev_command_idx(current_abs)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Jump to the next command's output from the current scroll position.
    pub fn jump_to_next_command(&mut self) -> Option<&CommandRecord> {
        if self.scroll_offset == 0 {
            return None;
        }
        let current_abs = self.viewport_top_abs();
        let idx = self.find_next_command_idx(current_abs)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Jump to a specific command by ID.
    pub fn jump_to_command(&mut self, id: u64) -> Option<&CommandRecord> {
        let idx = self.command_history.iter().position(|cmd| cmd.id == id)?;
        let target_line = self.command_history[idx].prompt_line;
        self.scroll_offset = self.abs_line_to_scroll_offset(target_line);
        Some(&self.command_history[idx])
    }

    /// Return the command that is currently visible at the top of the viewport.
    pub fn current_visible_command(&self) -> Option<(usize, &CommandRecord)> {
        let view_abs = self.viewport_top_abs();
        let pos = self.command_history.partition_point(|cmd| cmd.prompt_line <= view_abs);
        if pos == 0 { return None; }
        Some((pos - 1, &self.command_history[pos - 1]))
    }

    /// Extract command text from the grid. Returns empty if start has scrolled off (C4).
    fn extract_command_text(&self, start_abs_line: usize, start_col: usize) -> String {
        if start_abs_line < self.total_scrolled_lines {
            return String::new();
        }
        let start_row = start_abs_line - self.total_scrolled_lines;
        let end_col = self.cursor_col;
        let end_row = self.cursor_row;
        let grid = self.grid();
        if start_row >= grid.rows() {
            return String::new();
        }
        let mut text = String::new();
        for row in start_row..=end_row.min(grid.rows().saturating_sub(1)) {
            let col_start = if row == start_row { start_col } else { 0 };
            let col_end = if row == end_row { end_col } else { grid.cols() };
            for col in col_start..col_end.min(grid.cols()) {
                let cell = grid.cell(col, row);
                if cell.width > 0 {
                    text.push(cell.c);
                }
            }
            if row != end_row {
                text.push('\n');
            }
        }
        text.trim().to_string()
    }

    // -----------------------------------------------------------------------
    // Session Persistence (Snapshots)
    // -----------------------------------------------------------------------

    /// Create a serializable snapshot of the current terminal state.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let grid = self.grid();
        let mut grid_cells = Vec::with_capacity(grid.rows());
        for row in 0..grid.rows() {
            let mut row_cells = Vec::with_capacity(grid.cols());
            for col in 0..grid.cols() {
                row_cells.push(*grid.cell(col, row));
            }
            grid_cells.push(row_cells);
        }

        TerminalSnapshot {
            grid_cells,
            cursor_col: self.cursor_col,
            cursor_row: self.cursor_row,
            cursor_shape: self.cursor_shape,
            pen: self.pen,
            cols: self.cols,
            rows: self.rows,
            command_history: self.command_history.iter().cloned().collect(),
            total_scrolled_lines: self.total_scrolled_lines,
            title: self.osc.title.clone(),
            cwd: self.osc.cwd.clone(),
        }
    }

    /// Restore a terminal from a snapshot.
    ///
    /// Creates a new Terminal with the snapshot's state applied.
    /// The scrollback buffer is fresh (warm tier files must be preserved
    /// separately for full scrollback restoration).
    pub fn restore_from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        let mut term = Self::new(snapshot.cols, snapshot.rows);

        // Restore grid cells.
        let grid = &mut term.main_grid;
        for (row_idx, row_cells) in snapshot.grid_cells.iter().enumerate() {
            if row_idx >= grid.rows() {
                break;
            }
            for (col_idx, cell) in row_cells.iter().enumerate() {
                if col_idx >= grid.cols() {
                    break;
                }
                *grid.cell_mut(col_idx, row_idx) = *cell;
            }
        }

        term.cursor_col = snapshot.cursor_col.min(snapshot.cols.saturating_sub(1));
        term.cursor_row = snapshot.cursor_row.min(snapshot.rows.saturating_sub(1));
        term.cursor_shape = snapshot.cursor_shape;
        term.pen = snapshot.pen;
        // W5: Scrollback is empty after restore, so reset total_scrolled_lines
        // to 0 and only keep command records that reference on-screen lines
        // (i.e., those whose prompt_line >= snapshot.total_scrolled_lines).
        // Adjust their absolute line numbers to be relative to the new epoch.
        term.total_scrolled_lines = 0;
        term.command_history = VecDeque::new();
        // Preserve command history for display in timeline (text, timestamps, exit codes)
        // but mark their scrollback positions as unreachable.
        for cmd in &snapshot.command_history {
            let mut adjusted = cmd.clone();
            // All historical line references are invalid without scrollback;
            // set them to 0 so navigation won't jump to wrong positions.
            adjusted.scrollback_line_start = 0;
            adjusted.scrollback_line_end = Some(0);
            adjusted.prompt_line = 0;
            term.command_history.push_back(adjusted);
        }
        term.next_command_id = snapshot
            .command_history
            .back()
            .map_or(0, |c| c.id + 1);
        term.osc.title = snapshot.title.clone();
        term.osc.cwd = snapshot.cwd.clone();

        term
    }

    /// Create a named snapshot of the current state.
    pub fn create_named_snapshot(&self, name: &str) -> NamedSnapshot {
        NamedSnapshot {
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            terminal_snapshot: self.snapshot(),
        }
    }

    /// Push a command record to history with O(1) eviction (C2/W1: shared helper).
    fn push_command_record(&mut self, record: CommandRecord) {
        self.command_history.push_back(record);
        while self.command_history.len() > self.max_command_history {
            self.command_history.pop_front();
        }
    }

    /// Finalize a pending command and add it to history (A→A path, no D received).
    fn finalize_pending_command(&mut self, current_abs_line: usize) {
        if let Some(pending) = self.pending_command.take() {
            if let (Some(started_at), Some(output_start)) =
                (pending.started_at, pending.output_start_abs_line)
            {
                let id = self.next_command_id;
                self.next_command_id += 1;
                // C1: consume stored command text
                let cmd_text = self.pending_command_text.take().unwrap_or_default();
                let record = CommandRecord {
                    id,
                    command_text: cmd_text,
                    cwd: pending.cwd,
                    timestamp: started_at,
                    duration_ms: None,
                    exit_code: None,
                    scrollback_line_start: output_start,
                    scrollback_line_end: Some(current_abs_line),
                    prompt_line: pending.prompt_abs_line,
                };
                self.push_command_record(record);
            }
        }
    }

    /// Get the current Kitty keyboard protocol flags (0 if none set).
    pub fn kitty_keyboard_mode(&self) -> u32 {
        self.kitty_keyboard_flags.last().copied().unwrap_or(0)
    }

    /// Get a row from scrollback history. Index 0 is the most recent scrollback line.
    pub fn scrollback_row(&self, idx: usize) -> Option<&[Cell]> {
        self.scrollback.get(idx)
    }

    /// Clear the screen and scrollback buffer (for Cmd+K).
    pub fn clear_all(&mut self) {
        self.scrollback.clear();
        self.scroll_offset = 0;
        self.grid_mut().clear();
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.wrap_pending = false;
        self.image_store.delete_all();
    }

    /// Resize the terminal.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.main_grid.resize(cols, rows);
        self.alt_grid.resize(cols, rows);
        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.wrap_pending = false;

        self.tab_stops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            self.tab_stops[i] = true;
        }

        // S5: Resize invalidates pending command screen positions.
        self.pending_command = None;
        self.pending_command_text = None;
    }

    /// Feed raw bytes from the PTY through the VT parser.
    ///
    /// APC sequences (used by Kitty Graphics Protocol) are extracted before
    /// the data reaches vte, since vte 0.13 ignores APC content.
    pub fn feed(&mut self, parser: &mut vte::Parser, data: &[u8]) {
        let result = self.apc_extractor.process(data);

        // Process any extracted APC payloads (Kitty Graphics).
        for payload in &result.apc_payloads {
            self.handle_apc_payload(payload);
        }

        // Feed remaining bytes to the vte parser.
        for &byte in &result.passthrough {
            parser.advance(self, byte);
        }
    }

    /// Handle a complete APC payload (Kitty Graphics Protocol).
    ///
    /// Kitty graphics APC format: `G<header>;<base64data>` or `G<header>`
    fn handle_apc_payload(&mut self, payload: &[u8]) {
        // Kitty graphics payloads start with 'G'.
        if payload.is_empty() || payload[0] != b'G' {
            log::trace!("APC: non-Kitty payload (first byte: {:?})", payload.first());
            return;
        }

        let body = &payload[1..]; // Skip the 'G' prefix.
        let body_str = match std::str::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                log::trace!("APC: invalid UTF-8 in Kitty payload");
                return;
            }
        };

        // Split into header and base64 data at ';'.
        let (header, b64_data) = match body_str.find(';') {
            Some(idx) => (&body_str[..idx], &body_str[idx + 1..]),
            None => (body_str, ""),
        };

        // Feed to the chunked accumulator.
        if let Some((cmd, decoded)) = self.kitty_accumulator.feed(header, b64_data) {
            let col = self.cursor_col;
            let row = self.cursor_row;
            image::process_kitty_command(&cmd, &decoded, &mut self.image_store, col, row);
        }
    }

    fn clamp_col(&self, col: usize) -> usize {
        col.min(self.cols.saturating_sub(1))
    }

    fn clamp_row(&self, row: usize) -> usize {
        row.min(self.rows.saturating_sub(1))
    }

    /// Perform a newline: move cursor down, scroll if at bottom of scroll region.
    fn newline(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            // Save the top row to scrollback before scrolling, but only
            // when using the main screen with scroll_top == 0.
            if self.scroll_top == 0 && !self.using_alt {
                let row = self.grid().row_cells(0);
                self.scrollback.push(row);
                self.total_scrolled_lines += 1;
            }
            let top = self.scroll_top;
            let bottom = self.scroll_bottom;
            let bg = self.pen.bg;
            self.grid_mut().scroll_up_with_bg(top, bottom, 1, bg);
        } else {
            self.cursor_row = self.clamp_row(self.cursor_row + 1);
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            let top = self.scroll_top;
            let bottom = self.scroll_bottom;
            let bg = self.pen.bg;
            self.grid_mut().scroll_down_with_bg(top, bottom, 1, bg);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    /// Enter alternate screen buffer.
    fn enter_alt_screen(&mut self) {
        if !self.using_alt {
            self.saved_cursor_main = Some(SavedCursor {
                col: self.cursor_col,
                row: self.cursor_row,
                pen: self.pen,
            });
            self.using_alt = true;
            self.modes.alternate_screen = true;
            self.alt_grid.clear();
            self.cursor_col = 0;
            self.cursor_row = 0;
        }
    }

    /// Leave alternate screen buffer.
    fn leave_alt_screen(&mut self) {
        if self.using_alt {
            self.using_alt = false;
            self.modes.alternate_screen = false;
            self.alt_grid.clear();
            if let Some(saved) = self.saved_cursor_main.take() {
                self.cursor_col = saved.col;
                self.cursor_row = saved.row;
                self.pen = saved.pen;
            }
        }
    }

    /// Process SGR (Select Graphic Rendition) parameters.
    fn handle_sgr(&mut self, params: &vte::Params) {
        let mut iter = params.iter();

        let first = match iter.next() {
            Some(sub) => sub,
            None => {
                self.pen.reset();
                return;
            }
        };

        let mut pending: Option<&[u16]> = Some(first);

        while let Some(sub) = pending.take().or_else(|| iter.next()) {
            let code = sub[0];
            match code {
                0 => self.pen.reset(),
                1 => self.pen.attrs.insert(Attrs::BOLD),
                2 => self.pen.attrs.insert(Attrs::DIM),
                3 => self.pen.attrs.insert(Attrs::ITALIC),
                4 => {
                    if sub.len() > 1 {
                        match sub[1] {
                            0 => {
                                self.pen.attrs.remove(
                                    Attrs::UNDERLINE
                                        | Attrs::DOUBLE_UNDERLINE
                                        | Attrs::CURLY_UNDERLINE
                                        | Attrs::DOTTED_UNDERLINE
                                        | Attrs::DASHED_UNDERLINE,
                                );
                            }
                            1 => self.pen.attrs.insert(Attrs::UNDERLINE),
                            2 => self.pen.attrs.insert(Attrs::DOUBLE_UNDERLINE),
                            3 => self.pen.attrs.insert(Attrs::CURLY_UNDERLINE),
                            4 => self.pen.attrs.insert(Attrs::DOTTED_UNDERLINE),
                            5 => self.pen.attrs.insert(Attrs::DASHED_UNDERLINE),
                            _ => {}
                        }
                    } else {
                        self.pen.attrs.insert(Attrs::UNDERLINE);
                    }
                }
                5 | 6 => self.pen.attrs.insert(Attrs::BLINK),
                7 => self.pen.attrs.insert(Attrs::REVERSE),
                8 => self.pen.attrs.insert(Attrs::HIDDEN),
                9 => self.pen.attrs.insert(Attrs::STRIKETHROUGH),
                21 => self.pen.attrs.insert(Attrs::DOUBLE_UNDERLINE),
                22 => self.pen.attrs.remove(Attrs::BOLD | Attrs::DIM),
                23 => self.pen.attrs.remove(Attrs::ITALIC),
                24 => {
                    self.pen.attrs.remove(
                        Attrs::UNDERLINE
                            | Attrs::DOUBLE_UNDERLINE
                            | Attrs::CURLY_UNDERLINE
                            | Attrs::DOTTED_UNDERLINE
                            | Attrs::DASHED_UNDERLINE,
                    );
                }
                25 => self.pen.attrs.remove(Attrs::BLINK),
                27 => self.pen.attrs.remove(Attrs::REVERSE),
                28 => self.pen.attrs.remove(Attrs::HIDDEN),
                29 => self.pen.attrs.remove(Attrs::STRIKETHROUGH),
                30..=37 | 90..=97 => {
                    if let Some(c) = NamedColor::from_sgr_fg(code) {
                        self.pen.fg = Color::Named(c);
                    }
                }
                38 => {
                    self.pen.fg = parse_extended_color(&mut iter);
                }
                39 => self.pen.fg = Color::Default,
                40..=47 | 100..=107 => {
                    if let Some(c) = NamedColor::from_sgr_bg(code) {
                        self.pen.bg = Color::Named(c);
                    }
                }
                48 => {
                    self.pen.bg = parse_extended_color(&mut iter);
                }
                49 => self.pen.bg = Color::Default,
                58 => {
                    self.pen.underline_color = parse_extended_color(&mut iter);
                }
                59 => self.pen.underline_color = Color::Default,
                _ => {
                    log::trace!("unhandled SGR code: {code}");
                }
            }
        }
    }

    /// Handle private mode set/reset (DECSET/DECRST).
    fn handle_private_mode(&mut self, code: u16, enable: bool) {
        match code {
            1 => {}
            7 => self.modes.auto_wrap = enable,
            12 => {}
            25 => self.modes.cursor_visible = enable,
            47 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            // Mouse tracking modes — only one can be active at a time.
            1000 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::Click
                } else {
                    MouseMode::None
                };
                log::trace!("mouse mode 1000 (click) {}", if enable { "on" } else { "off" });
            }
            1002 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::ButtonMotion
                } else {
                    MouseMode::None
                };
                log::trace!(
                    "mouse mode 1002 (button motion) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1003 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::AnyMotion
                } else {
                    MouseMode::None
                };
                log::trace!(
                    "mouse mode 1003 (any motion) {}",
                    if enable { "on" } else { "off" }
                );
            }
            // Focus events (mode 1004).
            1004 => {
                self.modes.focus_events = enable;
                log::trace!("focus events {}", if enable { "on" } else { "off" });
            }
            // Mouse format modes.
            1005 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Utf8
                } else {
                    MouseFormat::X10
                };
                log::trace!("mouse format 1005 (utf8) {}", if enable { "on" } else { "off" });
            }
            1006 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Sgr
                } else {
                    MouseFormat::X10
                };
                log::trace!("mouse format 1006 (sgr) {}", if enable { "on" } else { "off" });
            }
            1015 => {
                self.modes.mouse_format = if enable {
                    MouseFormat::Urxvt
                } else {
                    MouseFormat::X10
                };
                log::trace!(
                    "mouse format 1015 (urxvt) {}",
                    if enable { "on" } else { "off" }
                );
            }
            1049 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            2004 => self.modes.bracketed_paste = enable,
            _ => {
                log::trace!(
                    "unhandled private mode: {code} {}",
                    if enable { "set" } else { "reset" }
                );
            }
        }
    }
}

/// Parse extended color (38/48/58 ; 5;N or 38/48/58 ; 2;R;G;B).
fn parse_extended_color<'a>(iter: &mut impl Iterator<Item = &'a [u16]>) -> Color {
    match iter.next() {
        Some(&[5]) => {
            if let Some(&[n]) = iter.next() {
                Color::Indexed(n as u8)
            } else {
                Color::Default
            }
        }
        Some(&[2]) => {
            let r = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            let g = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            let b = iter.next().map(|s| s[0] as u8).unwrap_or(0);
            Color::Rgb(r, g, b)
        }
        Some(sub) if sub.len() >= 2 && sub[0] == 2 => {
            let r = sub.get(1).copied().unwrap_or(0) as u8;
            let g = sub.get(2).copied().unwrap_or(0) as u8;
            let b = sub.get(3).copied().unwrap_or(0) as u8;
            Color::Rgb(r, g, b)
        }
        Some(sub) if sub.len() >= 2 && sub[0] == 5 => Color::Indexed(sub[1] as u8),
        _ => Color::Default,
    }
}

/// Returns true for zero-width combining codepoints that should not occupy a cell.
fn is_zero_width_combining(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0xFE00..=0xFE0F        // Variation selectors
        | 0x200D               // ZWJ (Zero Width Joiner)
        | 0x200B..=0x200F      // Zero-width space, ZWNJ, ZWJ, LRM, RLM
        | 0x1F3FB..=0x1F3FF    // Skin tone modifiers
        | 0xE0020..=0xE007F    // Tag characters (flag subdivisions)
        | 0xE0001              // Language tag
        | 0x20E3               // Combining enclosing keycap
        | 0x2060..=0x2064      // Word joiner, invisible separators
        | 0xFEFF               // BOM / zero-width no-break space
    )
}

impl vte::Perform for Terminal {
    fn print(&mut self, c: char) {
        // Skip zero-width combining characters that should not occupy a cell.
        // These include variation selectors, ZWJ, skin tone modifiers, and tags.
        if is_zero_width_combining(c) {
            return;
        }

        let char_width = UnicodeWidthChar::width(c).unwrap_or(1);

        if self.wrap_pending {
            self.wrap_pending = false;
            if self.modes.auto_wrap {
                self.cursor_col = 0;
                self.newline();
            }
        }

        if char_width == 2 && self.cursor_col >= self.cols - 1 {
            if self.modes.auto_wrap {
                self.cursor_col = 0;
                self.newline();
            } else {
                return;
            }
        }

        let col = self.cursor_col;
        let row = self.cursor_row;
        let pen = self.pen;

        let has_hyperlink = self.current_hyperlink.is_some();

        // Fix ghost characters: clean up wide character fragments.
        //
        // Case 1: If we are overwriting the continuation cell (width==0)
        // of a wide character, the leading cell must be cleared.
        if col > 0 {
            let prev_width = self.grid().cell(col, row).width;
            if prev_width == 0 {
                // This cell is a continuation — clear the leading cell to the left.
                self.grid_mut().cell_mut(col - 1, row).reset();
            }
        }

        // Case 2: If we are overwriting the leading cell (width==2) of a wide
        // character, the continuation cell to the right must be cleared.
        {
            let old_width = self.grid().cell(col, row).width;
            if old_width == 2 && col + 1 < self.cols {
                self.grid_mut().cell_mut(col + 1, row).reset();
            }
        }

        // Case 3: If we are writing a wide character, the continuation cell
        // at col+1 might be the leading cell of another wide character. If so,
        // clear *that* wide character's continuation at col+2.
        if char_width == 2 && col + 1 < self.cols {
            let next_width = self.grid().cell(col + 1, row).width;
            if next_width == 2 && col + 2 < self.cols {
                self.grid_mut().cell_mut(col + 2, row).reset();
            }
        }

        {
            let cell = self.grid_mut().cell_mut(col, row);
            cell.c = c;
            cell.fg = pen.fg;
            cell.bg = pen.bg;
            cell.attrs = pen.attrs;
            cell.underline_color = pen.underline_color;
            cell.width = char_width as u8;
            cell.hyperlink = has_hyperlink;
        }

        if char_width == 2 && col + 1 < self.cols {
            let cell = self.grid_mut().cell_mut(col + 1, row);
            cell.c = '\0';
            cell.width = 0;
            cell.fg = pen.fg;
            cell.bg = pen.bg;
            cell.attrs = pen.attrs;
            cell.hyperlink = has_hyperlink;
        }

        self.cursor_col += char_width;
        if self.cursor_col >= self.cols {
            self.cursor_col = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {
                log::trace!("BEL");
            }
            0x08 => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
                self.wrap_pending = false;
            }
            0x09 => {
                let cur = self.cursor_col;
                let next_tab = if cur + 1 < self.cols {
                    self.tab_stops[cur + 1..]
                        .iter()
                        .position(|&t| t)
                        .map(|p| cur + 1 + p)
                        .unwrap_or(self.cols - 1)
                } else {
                    self.cols - 1
                };
                self.cursor_col = next_tab.min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x0A | 0x0B | 0x0C => {
                self.newline();
                self.wrap_pending = false;
            }
            0x0D => {
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            0x0E | 0x0F => {}
            _ => {
                log::trace!("unhandled execute: 0x{byte:02x}");
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            ([], b'7') => {
                let saved = SavedCursor {
                    col: self.cursor_col,
                    row: self.cursor_row,
                    pen: self.pen,
                };
                if self.using_alt {
                    self.saved_cursor_alt = Some(saved);
                } else {
                    self.saved_cursor_main = Some(saved);
                }
            }
            ([], b'8') => {
                let saved = if self.using_alt {
                    self.saved_cursor_alt
                } else {
                    self.saved_cursor_main
                };
                if let Some(s) = saved {
                    self.cursor_col = s.col;
                    self.cursor_row = s.row;
                    self.pen = s.pen;
                }
            }
            ([], b'M') => {
                self.reverse_index();
            }
            ([], b'D') => {
                self.newline();
            }
            ([], b'E') => {
                self.cursor_col = 0;
                self.newline();
            }
            ([], b'H') => {
                if self.cursor_col < self.cols {
                    self.tab_stops[self.cursor_col] = true;
                }
            }
            ([], b'c') => {
                let cols = self.cols;
                let rows = self.rows;
                *self = Terminal::new(cols, rows);
            }
            _ => {
                log::trace!(
                    "unhandled ESC: intermediates={intermediates:?}, byte=0x{byte:02x}"
                );
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let p: Vec<Vec<u16>> = params.iter().map(|s| s.to_vec()).collect();
        let param = |idx: usize, default: u16| -> u16 {
            p.get(idx)
                .and_then(|s| s.first().copied())
                .filter(|&v| v != 0)
                .unwrap_or(default)
        };

        match (intermediates, action) {
            ([], 'A') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.wrap_pending = false;
            }
            ([], 'B') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(self.cursor_row + n);
                self.wrap_pending = false;
            }
            ([], 'C') => {
                let n = param(0, 1) as usize;
                self.cursor_col = self.clamp_col(self.cursor_col + n);
                self.wrap_pending = false;
            }
            ([], 'D') => {
                let n = param(0, 1) as usize;
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.wrap_pending = false;
            }
            ([], 'E') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(self.cursor_row + n);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            ([], 'F') => {
                let n = param(0, 1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            ([], 'G') => {
                let col = param(0, 1) as usize;
                self.cursor_col = self.clamp_col(col.saturating_sub(1));
                self.wrap_pending = false;
            }
            ([], 'H') | ([], 'f') => {
                let row = param(0, 1) as usize;
                let col = param(1, 1) as usize;
                self.cursor_row = self.clamp_row(row.saturating_sub(1));
                self.cursor_col = self.clamp_col(col.saturating_sub(1));
                self.wrap_pending = false;
            }
            // ED — Erase in Display (BCE: use current pen background).
            ([], 'J') => {
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                match param(0, 0) {
                    0 => self.grid_mut().erase_below_with_bg(col, row, bg),
                    1 => self.grid_mut().erase_above_with_bg(col, row, bg),
                    2 | 3 => self.grid_mut().clear_with_bg(bg),
                    _ => {}
                }
            }
            // EL — Erase in Line (BCE: use current pen background).
            ([], 'K') => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                let bg = self.pen.bg;
                match param(0, 0) {
                    0 => self.grid_mut().clear_to_eol_with_bg(col, row, bg),
                    1 => self.grid_mut().clear_from_bol_with_bg(col, row, bg),
                    2 => self.grid_mut().clear_row_with_bg(row, bg),
                    _ => {}
                }
            }
            // IL — Insert Lines (BCE).
            ([], 'L') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().insert_lines_with_bg(row, n, bottom, bg);
            }
            // DL — Delete Lines (BCE).
            ([], 'M') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().delete_lines_with_bg(row, n, bottom, bg);
            }
            // DCH — Delete Characters (BCE).
            ([], 'P') => {
                let n = param(0, 1) as usize;
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                self.grid_mut().delete_cells_with_bg(col, row, n, bg);
            }
            // SU — Scroll Up (C3: track scrolled lines for command history) (BCE).
            ([], 'S') => {
                let n = param(0, 1) as usize;
                let top = self.scroll_top;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                // Save scrolled-off rows to scrollback (same guard as newline).
                if top == 0 && !self.using_alt {
                    for i in 0..n.min(bottom + 1) {
                        let row = self.grid().row_cells(i);
                        self.scrollback.push(row);
                    }
                    self.total_scrolled_lines += n.min(bottom + 1);
                }
                self.grid_mut().scroll_up_with_bg(top, bottom, n, bg);
            }
            // SD — Scroll Down (BCE).
            ([], 'T') => {
                let n = param(0, 1) as usize;
                let top = self.scroll_top;
                let bottom = self.scroll_bottom;
                let bg = self.pen.bg;
                self.grid_mut().scroll_down_with_bg(top, bottom, n, bg);
            }
            // ECH — Erase Characters (BCE: use current pen background).
            ([], 'X') => {
                let n = param(0, 1) as usize;
                let row = self.cursor_row;
                let col = self.cursor_col;
                let cols = self.cols;
                let bg = self.pen.bg;
                for i in 0..n {
                    let c = col + i;
                    if c < cols {
                        self.grid_mut().cell_mut(c, row).reset_with_bg(bg);
                    }
                }
            }
            // ICH — Insert Characters (BCE).
            ([], '@') => {
                let n = param(0, 1) as usize;
                let col = self.cursor_col;
                let row = self.cursor_row;
                let bg = self.pen.bg;
                self.grid_mut().insert_cells_with_bg(col, row, n, bg);
            }
            // VPA — Vertical Line Position Absolute.
            ([], 'd') => {
                let row = param(0, 1) as usize;
                self.cursor_row = self.clamp_row(row.saturating_sub(1));
                self.wrap_pending = false;
            }
            // SGR — Select Graphic Rendition.
            ([], 'm') => {
                self.handle_sgr(params);
            }
            // DSR — Device Status Report.
            ([], 'n') => {
                log::trace!("DSR request: {}", param(0, 0));
            }
            // DECSTBM — Set Top and Bottom Margins.
            ([], 'r') => {
                let top = param(0, 1) as usize;
                let bottom = param(1, self.rows as u16) as usize;
                self.scroll_top = top.saturating_sub(1);
                self.scroll_bottom = (bottom.saturating_sub(1)).min(self.rows - 1);
                self.cursor_col = 0;
                self.cursor_row = 0;
                self.wrap_pending = false;
            }
            // CBT — Cursor Backward Tabulation.
            ([], 'Z') => {
                let n = param(0, 1) as usize;
                for _ in 0..n {
                    if self.cursor_col == 0 {
                        break;
                    }
                    self.cursor_col -= 1;
                    while self.cursor_col > 0 && !self.tab_stops[self.cursor_col] {
                        self.cursor_col -= 1;
                    }
                }
                self.wrap_pending = false;
            }
            // TBC — Tabulation Clear.
            ([], 'g') => match param(0, 0) {
                0 => {
                    if self.cursor_col < self.cols {
                        self.tab_stops[self.cursor_col] = false;
                    }
                }
                3 => {
                    for t in &mut self.tab_stops {
                        *t = false;
                    }
                }
                _ => {}
            },
            // DECSCUSR — Set Cursor Shape.
            ([b' '], 'q') => {
                self.cursor_shape = match param(0, 1) {
                    0 | 1 => CursorShape::BlinkingBlock,
                    2 => CursorShape::Block,
                    3 => CursorShape::BlinkingUnderline,
                    4 => CursorShape::Underline,
                    5 => CursorShape::BlinkingBar,
                    6 => CursorShape::Bar,
                    _ => CursorShape::BlinkingBlock,
                };
            }
            // DECSET — Private mode set.
            ([b'?'], 'h') => {
                for sub in params.iter() {
                    self.handle_private_mode(sub[0], true);
                }
            }
            // DECRST — Private mode reset.
            ([b'?'], 'l') => {
                for sub in params.iter() {
                    self.handle_private_mode(sub[0], false);
                }
            }
            // SM — Set Mode.
            ([], 'h') => {
                if param(0, 0) == 4 {
                    self.modes.insert_mode = true;
                }
            }
            // RM — Reset Mode.
            ([], 'l') => {
                if param(0, 0) == 4 {
                    self.modes.insert_mode = false;
                }
            }
            // Kitty keyboard protocol: CSI > flags u — push keyboard mode.
            ([b'>'], 'u') => {
                let flags = param(0, 0) as u32;
                self.kitty_keyboard_flags.push(flags);
                log::trace!("kitty keyboard: push flags={flags}");
            }
            // Kitty keyboard protocol: CSI < number u — pop keyboard mode(s).
            ([b'<'], 'u') => {
                let count = param(0, 1).max(1) as usize;
                for _ in 0..count {
                    if self.kitty_keyboard_flags.pop().is_none() {
                        break;
                    }
                }
                log::trace!("kitty keyboard: pop {count}");
            }
            // Kitty keyboard protocol: CSI ? u — query current keyboard mode.
            ([b'?'], 'u') => {
                log::trace!(
                    "kitty keyboard: query (current={})",
                    self.kitty_keyboard_mode()
                );
            }
            _ => {
                log::trace!(
                    "unhandled CSI: intermediates={intermediates:?}, action={action}, params={p:?}"
                );
            }
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        let cmd = std::str::from_utf8(params[0]).unwrap_or("");

        match cmd {
            "0" | "2" => {
                if let Some(title) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(title) {
                        self.osc.title = s.to_string();
                        log::debug!("title: {s}");
                    }
                }
            }
            "7" => {
                if let Some(uri) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(uri) {
                        self.osc.cwd_uri = s.to_string();
                        let path = s
                            .strip_prefix("file://")
                            .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
                            .unwrap_or(s);
                        self.osc.cwd = path.to_string();
                        log::debug!("cwd: {path}");
                    }
                }
            }
            "9" => {
                if let Some(msg) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 9): {s}");
                    }
                }
            }
            "99" => {
                if let Some(msg) = params.last() {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 99): {s}");
                    }
                }
            }
            "133" => {
                if let Some(sub) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(sub) {
                        match s.chars().next() {
                            Some('A') => {
                                self.osc.prompt_start =
                                    Some((self.cursor_col, self.cursor_row));
                                log::debug!("OSC 133: prompt start");

                                if self.command_history_enabled {
                                    let current_abs = self.abs_line(self.cursor_row);
                                    // Finalize previous pending command if any
                                    self.finalize_pending_command(current_abs);
                                    // Start tracking a new command
                                    self.pending_command = Some(PendingCommand {
                                        prompt_abs_line: current_abs,
                                        command_start_abs_line: 0,
                                        command_start_col: 0,
                                        cwd: self.osc.cwd.clone(),
                                        started_at: None,
                                        output_start_abs_line: None,
                                    });
                                }
                            }
                            Some('B') => {
                                self.osc.command_start =
                                    Some((self.cursor_col, self.cursor_row));
                                log::debug!("OSC 133: command start");

                                if self.command_history_enabled {
                                    let abs = self.abs_line(self.cursor_row);
                                    let col = self.cursor_col;
                                    if let Some(ref mut pending) = self.pending_command {
                                        pending.command_start_abs_line = abs;
                                        pending.command_start_col = col;
                                    }
                                }
                            }
                            Some('C') => {
                                log::debug!("OSC 133: command executed");

                                if self.command_history_enabled {
                                    // Extract command text from grid (between B and C)
                                    // C4: pass abs_line directly; extract_command_text handles bounds
                                    let cmd_text = if let Some(ref pending) = self.pending_command {
                                        let start_abs = pending.command_start_abs_line;
                                        let start_col = pending.command_start_col;
                                        self.extract_command_text(start_abs, start_col)
                                    } else {
                                        String::new()
                                    };

                                    let abs = self.abs_line(self.cursor_row);
                                    if let Some(ref mut pending) = self.pending_command {
                                        pending.started_at = Some(chrono::Utc::now());
                                        pending.output_start_abs_line = Some(abs);
                                    }

                                    self.pending_command_text = Some(cmd_text);
                                }
                            }
                            Some('D') => {
                                log::debug!("OSC 133: command finished");

                                if self.command_history_enabled {
                                    // Parse exit code from parameters (e.g., "D;0" or "D;1")
                                    // W4: robust exit code parsing via strip_prefix
                                    let exit_code = s.strip_prefix('D')
                                        .and_then(|r| r.strip_prefix(';'))
                                        .and_then(|r| r.parse::<i32>().ok());

                                    let current_abs = self.abs_line(self.cursor_row);

                                    // W1: use shared helper for record creation
                                    if let Some(pending) = self.pending_command.take() {
                                        if let (Some(started_at), Some(output_start)) =
                                            (pending.started_at, pending.output_start_abs_line)
                                        {
                                            let duration_ms = chrono::Utc::now()
                                                .signed_duration_since(started_at)
                                                .num_milliseconds()
                                                .max(0) as u64;
                                            let id = self.next_command_id;
                                            self.next_command_id += 1;
                                            let cmd_text = self.pending_command_text.take()
                                                .unwrap_or_default();
                                            let record = CommandRecord {
                                                id,
                                                command_text: cmd_text,
                                                cwd: pending.cwd,
                                                timestamp: started_at,
                                                duration_ms: Some(duration_ms),
                                                exit_code,
                                                scrollback_line_start: output_start,
                                                scrollback_line_end: Some(current_abs),
                                                prompt_line: pending.prompt_abs_line,
                                            };
                                            self.push_command_record(record);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            // OSC 8 — Hyperlinks: OSC 8 ; params ; URI ST
            "8" => {
                // params[1] = hyperlink params (e.g. "id=xyz"), params[2] = URI
                let uri = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if uri.is_empty() {
                    // End hyperlink.
                    self.current_hyperlink = None;
                    log::trace!("OSC 8: end hyperlink");
                } else {
                    // Start hyperlink.
                    self.current_hyperlink = Some(uri.to_string());
                    log::trace!("OSC 8: start hyperlink uri={uri}");
                }
            }
            // OSC 52 — Clipboard: OSC 52 ; selection ; data ST
            "52" => {
                let selection = params
                    .get(1)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("c")
                    .to_string();
                let raw_data = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if raw_data == "?" {
                    self.clipboard_event = Some(ClipboardEvent::Query {
                        selection,
                    });
                    log::trace!("OSC 52: query clipboard selection={}", &self.clipboard_event.as_ref().map(|e| match e { ClipboardEvent::Query { selection } => selection.as_str(), _ => "" }).unwrap_or(""));
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(raw_data) {
                        Ok(bytes) => {
                            let decoded = String::from_utf8_lossy(&bytes).to_string();
                            log::trace!("OSC 52: set clipboard selection={selection}");
                            self.clipboard_event = Some(ClipboardEvent::Set {
                                selection,
                                data: decoded,
                            });
                        }
                        Err(e) => {
                            log::trace!("OSC 52: invalid base64: {e}");
                        }
                    }
                }
            }
            "777" => {
                if let Some(msg) = params.get(2) {
                    if let Ok(s) = std::str::from_utf8(msg) {
                        self.osc.last_notification = Some(s.to_string());
                        log::debug!("notification (OSC 777): {s}");
                    }
                }
            }
            // OSC 1337 — iTerm2 proprietary sequences (inline images, etc.)
            "1337" => {
                if let Some(payload_bytes) = params.get(1) {
                    if let Ok(payload) = std::str::from_utf8(payload_bytes) {
                        if payload.starts_with("File=") {
                            let col = self.cursor_col;
                            let row = self.cursor_row;
                            image::parse_iterm2_image(
                                payload,
                                &mut self.image_store,
                                col,
                                row,
                            );
                        } else {
                            log::trace!("OSC 1337: unhandled sub-command: {}", &payload[..payload.len().min(30)]);
                        }
                    }
                }
            }
            _ => {
                log::trace!("unhandled OSC: {cmd}");
            }
        }
    }

    fn hook(
        &mut self,
        _params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'q' => {
                // Sixel DCS: ESC P <params> q <sixel_data> ST
                log::trace!("DCS hook: Sixel");
                self.dcs_mode = DcsMode::Sixel;
                self.dcs_data.clear();
            }
            _ => {
                log::trace!("DCS hook: action={action}");
                self.dcs_mode = DcsMode::None;
            }
        }
    }

    fn put(&mut self, byte: u8) {
        match self.dcs_mode {
            DcsMode::Sixel => {
                self.dcs_data.push(byte);
            }
            DcsMode::None => {}
        }
    }

    fn unhook(&mut self) {
        match self.dcs_mode {
            DcsMode::Sixel => {
                log::trace!("DCS unhook: Sixel ({} bytes)", self.dcs_data.len());
                let data = std::mem::take(&mut self.dcs_data);
                let col = self.cursor_col;
                let row = self.cursor_row;
                image::process_sixel(&data, &mut self.image_store, col, row);
            }
            DcsMode::None => {
                log::trace!("DCS unhook");
            }
        }
        self.dcs_mode = DcsMode::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_str(term: &mut Terminal, parser: &mut vte::Parser, s: &str) {
        term.feed(parser, s.as_bytes());
    }

    #[test]
    fn test_print_basic() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "Hello");
        assert_eq!(term.grid().cell(0, 0).c, 'H');
        assert_eq!(term.grid().cell(4, 0).c, 'o');
        assert_eq!(term.cursor_col, 5);
    }

    #[test]
    fn test_newline() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        // LF only moves cursor down; CR returns to column 0.
        feed_str(&mut term, &mut parser, "Line1\r\nLine2");
        assert_eq!(term.grid().cell(0, 0).c, 'L');
        assert_eq!(term.grid().cell(0, 1).c, 'L');
        assert_eq!(term.cursor_row, 1);
    }

    #[test]
    fn test_cursor_movement() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);
    }

    #[test]
    fn test_erase_in_line() {
        let mut term = Terminal::new(10, 1);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "ABCDEFGHIJ");
        feed_str(&mut term, &mut parser, "\x1b[6G\x1b[K");
        assert_eq!(term.grid().cell(4, 0).c, 'E');
        assert_eq!(term.grid().cell(5, 0).c, ' ');
    }

    #[test]
    fn test_alternate_screen() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "Main");
        assert_eq!(term.grid().cell(0, 0).c, 'M');

        feed_str(&mut term, &mut parser, "\x1b[?1049h");
        assert!(term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, ' ');

        feed_str(&mut term, &mut parser, "Alt");
        assert_eq!(term.grid().cell(0, 0).c, 'A');

        feed_str(&mut term, &mut parser, "\x1b[?1049l");
        assert!(!term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, 'M');
    }

    #[test]
    fn test_sgr_colors() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[1;31;44mX");
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, 'X');
        assert_eq!(cell.fg, Color::Named(NamedColor::Red));
        assert_eq!(cell.bg, Color::Named(NamedColor::Blue));
        assert!(cell.attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn test_sgr_truecolor() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[38;2;255;128;0mX");
        assert_eq!(term.grid().cell(0, 0).fg, Color::Rgb(255, 128, 0));
    }

    #[test]
    fn test_sgr_256color() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[38;5;196mX");
        assert_eq!(term.grid().cell(0, 0).fg, Color::Indexed(196));
    }

    #[test]
    fn test_scroll_region() {
        let mut term = Terminal::new(80, 10);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[3;7r");
        assert_eq!(term.scroll_top, 2);
        assert_eq!(term.scroll_bottom, 6);
    }

    #[test]
    fn test_bracketed_paste_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.bracketed_paste);
        feed_str(&mut term, &mut parser, "\x1b[?2004h");
        assert!(term.modes.bracketed_paste);
        feed_str(&mut term, &mut parser, "\x1b[?2004l");
        assert!(!term.modes.bracketed_paste);
    }

    #[test]
    fn test_cursor_save_restore() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        feed_str(&mut term, &mut parser, "\x1b7");
        feed_str(&mut term, &mut parser, "\x1b[1;1H");
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 0);
        feed_str(&mut term, &mut parser, "\x1b8");
        assert_eq!(term.cursor_col, 9);
        assert_eq!(term.cursor_row, 4);
    }

    #[test]
    fn test_wide_char() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "A漢B");
        assert_eq!(term.grid().cell(0, 0).c, 'A');
        assert_eq!(term.grid().cell(0, 0).width, 1);
        assert_eq!(term.grid().cell(1, 0).c, '漢');
        assert_eq!(term.grid().cell(1, 0).width, 2);
        assert_eq!(term.grid().cell(2, 0).width, 0);
        assert_eq!(term.grid().cell(3, 0).c, 'B');
    }

    #[test]
    fn test_emoji() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        feed_str(&mut term, &mut parser, "A😀B");
        assert_eq!(term.grid().cell(0, 0).c, 'A');
        let emoji_cell = term.grid().cell(1, 0);
        eprintln!("emoji cell: c={:?} U+{:04X} width={}", emoji_cell.c, emoji_cell.c as u32, emoji_cell.width);
        assert_eq!(emoji_cell.c, '😀');
        assert_eq!(emoji_cell.width, 2);
        assert_eq!(term.grid().cell(2, 0).width, 0); // continuation
        assert_eq!(term.grid().cell(3, 0).c, 'B');
    }

    // --- Mouse tracking mode tests ---

    #[test]
    fn test_mouse_mode_click() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_mode, MouseMode::None);

        // Enable mode 1000 (click tracking).
        feed_str(&mut term, &mut parser, "\x1b[?1000h");
        assert_eq!(term.modes.mouse_mode, MouseMode::Click);

        // Disable mode 1000.
        feed_str(&mut term, &mut parser, "\x1b[?1000l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_mode_button_motion() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1002h");
        assert_eq!(term.modes.mouse_mode, MouseMode::ButtonMotion);

        feed_str(&mut term, &mut parser, "\x1b[?1002l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_mode_any_motion() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1003h");
        assert_eq!(term.modes.mouse_mode, MouseMode::AnyMotion);

        feed_str(&mut term, &mut parser, "\x1b[?1003l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    #[test]
    fn test_mouse_format_sgr() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);

        feed_str(&mut term, &mut parser, "\x1b[?1006h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Sgr);

        feed_str(&mut term, &mut parser, "\x1b[?1006l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    #[test]
    fn test_mouse_format_utf8() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1005h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Utf8);

        feed_str(&mut term, &mut parser, "\x1b[?1005l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    #[test]
    fn test_mouse_format_urxvt() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[?1015h");
        assert_eq!(term.modes.mouse_format, MouseFormat::Urxvt);

        feed_str(&mut term, &mut parser, "\x1b[?1015l");
        assert_eq!(term.modes.mouse_format, MouseFormat::X10);
    }

    // --- Focus events mode test ---

    #[test]
    fn test_focus_events_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.focus_events);

        feed_str(&mut term, &mut parser, "\x1b[?1004h");
        assert!(term.modes.focus_events);

        feed_str(&mut term, &mut parser, "\x1b[?1004l");
        assert!(!term.modes.focus_events);
    }

    // --- Kitty keyboard protocol tests ---

    #[test]
    fn test_kitty_keyboard_push_pop() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.kitty_keyboard_mode(), 0);

        // Push flags=1.
        feed_str(&mut term, &mut parser, "\x1b[>1u");
        assert_eq!(term.kitty_keyboard_mode(), 1);

        // Push flags=3.
        feed_str(&mut term, &mut parser, "\x1b[>3u");
        assert_eq!(term.kitty_keyboard_mode(), 3);

        // Pop one.
        feed_str(&mut term, &mut parser, "\x1b[<1u");
        assert_eq!(term.kitty_keyboard_mode(), 1);

        // Pop one more.
        feed_str(&mut term, &mut parser, "\x1b[<1u");
        assert_eq!(term.kitty_keyboard_mode(), 0);
    }

    #[test]
    fn test_kitty_keyboard_pop_multiple() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\x1b[>1u");
        feed_str(&mut term, &mut parser, "\x1b[>2u");
        feed_str(&mut term, &mut parser, "\x1b[>3u");
        assert_eq!(term.kitty_keyboard_mode(), 3);

        // Pop 2 at once.
        feed_str(&mut term, &mut parser, "\x1b[<2u");
        assert_eq!(term.kitty_keyboard_mode(), 1);
    }

    #[test]
    fn test_kitty_keyboard_pop_empty_stack() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Popping from empty stack should be safe.
        feed_str(&mut term, &mut parser, "\x1b[<5u");
        assert_eq!(term.kitty_keyboard_mode(), 0);
    }

    #[test]
    fn test_kitty_keyboard_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query should not crash (just logs).
        feed_str(&mut term, &mut parser, "\x1b[?u");
        assert_eq!(term.kitty_keyboard_mode(), 0);

        feed_str(&mut term, &mut parser, "\x1b[>5u");
        feed_str(&mut term, &mut parser, "\x1b[?u");
        assert_eq!(term.kitty_keyboard_mode(), 5);
    }

    // --- OSC 8 hyperlinks tests ---

    #[test]
    fn test_osc8_hyperlink() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Start hyperlink: OSC 8 ; ; https://example.com ST
        feed_str(
            &mut term,
            &mut parser,
            "\x1b]8;;https://example.com\x1b\\",
        );
        // Print some text while hyperlink is active.
        feed_str(&mut term, &mut parser, "link");

        assert!(term.grid().cell(0, 0).hyperlink);
        assert!(term.grid().cell(1, 0).hyperlink);
        assert!(term.grid().cell(2, 0).hyperlink);
        assert!(term.grid().cell(3, 0).hyperlink);

        // End hyperlink: OSC 8 ; ; ST
        feed_str(&mut term, &mut parser, "\x1b]8;;\x1b\\");
        // Print text after hyperlink ends.
        feed_str(&mut term, &mut parser, "text");

        assert!(!term.grid().cell(4, 0).hyperlink);
        assert!(!term.grid().cell(5, 0).hyperlink);
    }

    #[test]
    fn test_osc8_hyperlink_not_set_by_default() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "hello");
        assert!(!term.grid().cell(0, 0).hyperlink);
        assert!(!term.grid().cell(4, 0).hyperlink);
    }

    // --- OSC 52 clipboard tests ---

    #[test]
    fn test_osc52_set_clipboard() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(term.clipboard_event.is_none());

        // "hello" in base64 is "aGVsbG8="
        // OSC 52 ; c ; aGVsbG8= ST
        feed_str(&mut term, &mut parser, "\x1b]52;c;aGVsbG8=\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Set { selection, data }) => {
                assert_eq!(selection, "c");
                assert_eq!(data, "hello");
            }
            other => panic!("expected ClipboardEvent::Set, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_query_clipboard() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // OSC 52 ; c ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]52;c;?\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Query { selection }) => {
                assert_eq!(selection, "c");
            }
            other => panic!("expected ClipboardEvent::Query, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_primary_selection() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // "test" base64 = "dGVzdA=="
        feed_str(&mut term, &mut parser, "\x1b]52;p;dGVzdA==\x1b\\");

        match &term.clipboard_event {
            Some(ClipboardEvent::Set { selection, data }) => {
                assert_eq!(selection, "p");
                assert_eq!(data, "test");
            }
            other => panic!("expected ClipboardEvent::Set, got {other:?}"),
        }
    }

    #[test]
    fn test_osc52_invalid_base64() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Invalid base64 — should not set clipboard_event.
        feed_str(&mut term, &mut parser, "\x1b]52;c;!!!invalid!!!\x1b\\");

        assert!(term.clipboard_event.is_none());
    }
}
