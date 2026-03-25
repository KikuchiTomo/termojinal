//! Terminal state machine implementing the VT parser's `Perform` trait.

use crate::cell::{Attrs, Cell, Pen};
use crate::color::{Color, NamedColor};
use crate::grid::Grid;
use crate::image::{
    self, ApcExtractor, ImageStore, Iterm2Accumulator, KittyAccumulator,
};
use crate::scrollback::ScrollbackBuffer;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

/// DCS accumulation mode.
#[derive(Debug, PartialEq)]
enum DcsMode {
    /// Not accumulating DCS data.
    None,
    /// Accumulating Sixel data (DCS with `q` final character).
    Sixel,
    /// Accumulating DECDLD (soft font definition) data.
    Decdld {
        /// Font number (Pfn).
        font_number: u8,
        /// Starting character code (Pcn).
        start_char: u8,
        /// Character cell width in pixels (Pcmw or derived from Ps).
        cell_width: u8,
        /// Character cell height in pixels (derived from Ps or Pe).
        cell_height: u8,
        /// Erase control (Pe): 0 = erase all, 1 = erase only chars being loaded, 2 = erase all.
        erase_control: u8,
    },
}

/// A single DRCS (Dynamically Redefinable Character Set) glyph.
///
/// DECDLD allows programs to define custom character glyphs as bitmaps
/// that can be assigned to G0/G1 character sets.
#[derive(Debug, Clone)]
pub struct DrcsGlyph {
    /// 1-bit-per-pixel bitmap data (MSB first, row-major).
    pub bitmap: Vec<u8>,
    /// Width in pixels.
    pub width: u8,
    /// Height in pixels.
    pub height: u8,
}

/// Store for DRCS soft fonts loaded via DECDLD.
#[derive(Debug, Clone, Default)]
pub struct DrcsFontStore {
    /// Fonts keyed by (font_number, character_code).
    glyphs: std::collections::HashMap<(u8, u8), DrcsGlyph>,
}

impl DrcsFontStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a glyph for the given font number and character code.
    pub fn set_glyph(&mut self, font_number: u8, char_code: u8, glyph: DrcsGlyph) {
        self.glyphs.insert((font_number, char_code), glyph);
    }

    /// Look up a glyph by font number and character code.
    pub fn get_glyph(&self, font_number: u8, char_code: u8) -> Option<&DrcsGlyph> {
        self.glyphs.get(&(font_number, char_code))
    }

    /// Erase all glyphs for a given font number.
    pub fn erase_font(&mut self, font_number: u8) {
        self.glyphs.retain(|&(fn_, _), _| fn_ != font_number);
    }

    /// Erase all DRCS glyphs.
    pub fn erase_all(&mut self) {
        self.glyphs.clear();
    }

    /// Check if any glyphs are defined.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Get all defined glyphs.
    pub fn glyphs(&self) -> &std::collections::HashMap<(u8, u8), DrcsGlyph> {
        &self.glyphs
    }
}

/// File transfer event produced by iTerm2 OSC 1337 with inline=0.
///
/// When an application sends a file via `OSC 1337 ; File=inline=0;name=...;size=...:BASE64 ST`,
/// the terminal should offer the file for saving rather than displaying inline.
#[derive(Debug, Clone)]
pub struct FileTransferEvent {
    /// Decoded file name (from base64-encoded `name=` parameter).
    pub name: String,
    /// Raw file data (decoded from base64 payload).
    pub data: Vec<u8>,
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
    cursor_visible: bool,
    cursor_shape: CursorShape,
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
    /// DECCKM: when true, cursor keys send application sequences (ESC O A)
    /// instead of normal sequences (ESC [ A).
    pub application_cursor_keys: bool,
    /// LNM: when true, LF/VT/FF also perform CR (auto carriage return).
    pub linefeed_mode: bool,
    /// DECSDM (mode 80): Sixel display mode.
    /// When set, sixel images are displayed at the upper-left corner of the screen.
    pub sixel_display_mode: bool,
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
    /// iTerm2 multipart image transfer accumulator.
    iterm2_accumulator: Iterm2Accumulator,
    /// DCS data accumulation buffer (for Sixel and DECDLD).
    dcs_data: Vec<u8>,
    /// Current DCS accumulation mode.
    dcs_mode: DcsMode,
    /// DRCS soft font store (DECDLD-defined custom glyphs).
    pub drcs_fonts: DrcsFontStore,
    /// Pending responses to be written back to the PTY.
    ///
    /// Escape sequences like DSR, DA, OSC 10/11/12 color queries produce
    /// response bytes that the application layer must send to the PTY master.
    pending_responses: VecDeque<Vec<u8>>,
    /// File transfer event from iTerm2 OSC 1337 (inline=0).
    pub file_transfer_event: Option<FileTransferEvent>,
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
    /// Whether to use CJK-aware character width calculation.
    /// When true, Unicode East Asian Ambiguous width characters (e.g., ○●■□▲△◆◇★☆◯)
    /// are treated as 2-cell wide instead of 1-cell wide.
    pub cjk_width: bool,
}

/// Calculate the display width of a character, respecting CJK ambiguous width.
///
/// When `cjk` is true, characters with East Asian Width "Ambiguous" are treated
/// as 2-cell wide (standard CJK terminal behavior). Otherwise they are 1-cell wide.
#[inline]
pub fn char_width(c: char, cjk: bool) -> usize {
    if cjk {
        UnicodeWidthChar::width_cjk(c).unwrap_or(1)
    } else {
        UnicodeWidthChar::width(c).unwrap_or(1)
    }
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
            iterm2_accumulator: Iterm2Accumulator::new(),
            dcs_data: Vec::new(),
            dcs_mode: DcsMode::None,
            drcs_fonts: DrcsFontStore::new(),
            pending_responses: VecDeque::new(),
            file_transfer_event: None,
            command_history: VecDeque::new(),
            next_command_id: 0,
            pending_command: None,
            total_scrolled_lines: 0,
            command_history_enabled: true,
            max_command_history: 10_000,
            pending_command_text: None,
            cjk_width: false,
        }
    }

    /// Set whether to use CJK-aware character width calculation.
    pub fn set_cjk_width(&mut self, cjk: bool) {
        self.cjk_width = cjk;
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

    // -----------------------------------------------------------------------
    // PTY Response Queue
    // -----------------------------------------------------------------------

    /// Queue a response to be sent back to the PTY.
    ///
    /// Used by escape sequences that require a reply (DSR, DA, OSC 10/11/12
    /// color queries, etc.).  The application layer must drain these with
    /// `drain_responses()` and write them to the PTY master.
    fn queue_response(&mut self, data: Vec<u8>) {
        self.pending_responses.push_back(data);
    }

    /// Drain all pending responses that should be written to the PTY.
    ///
    /// Returns an iterator of byte buffers.  The application layer should
    /// call this after each `feed()` and write each buffer to the PTY.
    pub fn drain_responses(&mut self) -> std::collections::vec_deque::Drain<'_, Vec<u8>> {
        self.pending_responses.drain(..)
    }

    /// Check whether there are pending PTY responses.
    pub fn has_pending_responses(&self) -> bool {
        !self.pending_responses.is_empty()
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
        // Ensure scroll region is valid after resize.
        if self.scroll_top >= self.scroll_bottom && rows > 1 {
            self.scroll_bottom = rows - 1;
        }
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
            let before = self.image_store.placements().len();
            image::process_kitty_command(&cmd, &decoded, &mut self.image_store, col, row);
            // Advance cursor past the image if a new placement was added.
            if self.image_store.placements().len() > before {
                self.image_store.cap_placement_size(self.cols, self.rows);
                let cell_rows = self.image_store.placements().last()
                    .map(|p| p.cell_rows).unwrap_or(1);
                self.advance_cursor_past_image(cell_rows);
            }
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
                // Scroll image placements up so they track the grid content.
                self.image_store.scroll_up(1);
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

    /// Advance the cursor past an inline image.
    ///
    /// Moves the cursor to column 0 and down by `cell_rows` lines, performing
    /// newline scrolls as needed so the image occupies space in the terminal
    /// grid and subsequent text flows below it.
    fn advance_cursor_past_image(&mut self, cell_rows: usize) {
        self.cursor_col = 0;
        self.wrap_pending = false;
        for _ in 0..cell_rows {
            self.newline();
        }
    }

    /// Enter alternate screen buffer.
    fn enter_alt_screen(&mut self) {
        if !self.using_alt {
            self.saved_cursor_main = Some(SavedCursor {
                col: self.cursor_col,
                row: self.cursor_row,
                pen: self.pen,
                cursor_visible: self.modes.cursor_visible,
                cursor_shape: self.cursor_shape,
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
                self.modes.cursor_visible = saved.cursor_visible;
                self.cursor_shape = saved.cursor_shape;
            } else {
                // No saved state: ensure cursor is visible with default shape
                // to prevent cursor loss when alt screen apps exit abnormally.
                self.modes.cursor_visible = true;
                self.cursor_shape = CursorShape::default();
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
            // DECCKM — Application cursor keys.
            1 => {
                self.modes.application_cursor_keys = enable;
                log::trace!("DECCKM {}", if enable { "on" } else { "off" });
            }
            // DECOM — Origin mode.
            6 => {
                self.modes.origin_mode = enable;
                // When origin mode changes, cursor moves to the origin.
                if enable {
                    self.cursor_col = 0;
                    self.cursor_row = self.scroll_top;
                } else {
                    self.cursor_col = 0;
                    self.cursor_row = 0;
                }
                self.wrap_pending = false;
                log::trace!("DECOM {}", if enable { "on" } else { "off" });
            }
            7 => self.modes.auto_wrap = enable,
            // X10 mouse reporting (mode 9).
            9 => {
                self.modes.mouse_mode = if enable {
                    MouseMode::Click
                } else {
                    MouseMode::None
                };
                log::trace!("X10 mouse mode 9 {}", if enable { "on" } else { "off" });
            }
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
            // DECSDM — Sixel display mode (mode 80).
            80 => {
                self.modes.sixel_display_mode = enable;
                log::trace!("DECSDM {}", if enable { "on" } else { "off" });
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
            // Mode 1047 — Alternate screen buffer (without cursor save/restore).
            1047 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            // Mode 1048 — Save/restore cursor (DECSC/DECRC).
            1048 => {
                if enable {
                    // Save cursor.
                    let saved = SavedCursor {
                        col: self.cursor_col,
                        row: self.cursor_row,
                        pen: self.pen,
                        cursor_visible: self.modes.cursor_visible,
                        cursor_shape: self.cursor_shape,
                    };
                    if self.using_alt {
                        self.saved_cursor_alt = Some(saved);
                    } else {
                        self.saved_cursor_main = Some(saved);
                    }
                } else {
                    // Restore cursor.
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

        // Guard against zero-size terminal (can happen during resize transitions).
        if self.cols == 0 || self.rows == 0 {
            return;
        }

        let char_width = char_width(c, self.cjk_width);

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
                if self.cols == 0 { return; }
                let cur = self.cursor_col;
                let next_tab = if cur + 1 < self.cols {
                    self.tab_stops[cur + 1..]
                        .iter()
                        .position(|&t| t)
                        .map(|p| cur + 1 + p)
                        .unwrap_or(self.cols.saturating_sub(1))
                } else {
                    self.cols.saturating_sub(1)
                };
                self.cursor_col = next_tab.min(self.cols.saturating_sub(1));
                self.wrap_pending = false;
            }
            0x0A | 0x0B | 0x0C => {
                self.newline();
                // LNM (mode 20): when set, LF also performs CR.
                if self.modes.linefeed_mode {
                    self.cursor_col = 0;
                }
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
                    cursor_visible: self.modes.cursor_visible,
                    cursor_shape: self.cursor_shape,
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
                    self.modes.cursor_visible = s.cursor_visible;
                    self.cursor_shape = s.cursor_shape;
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
                    // Scroll image placements so they track the grid content.
                    self.image_store.scroll_up(n);
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
                // Handle wide char fragment at start of erased region.
                if col < cols && self.grid().cell(col, row).width == 0 && col > 0 {
                    self.grid_mut().cell_mut(col - 1, row).reset_with_bg(bg);
                }
                // Handle wide char fragment at end of erased region.
                let end = (col + n).min(cols);
                if end > 0 && end < cols && self.grid().cell(end, row).width == 0 {
                    // Continuation of a wide char whose leading cell was erased.
                    self.grid_mut().cell_mut(end, row).reset_with_bg(bg);
                }
                if end > 0 && end <= cols {
                    let last_erased = end - 1;
                    if self.grid().cell(last_erased, row).width == 2 && last_erased + 1 < cols {
                        // Leading cell of wide char partially erased; clear continuation.
                        // (handled below by the reset loop)
                    }
                }
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
                match param(0, 0) {
                    5 => {
                        // Status report — respond "OK".
                        self.queue_response(b"[0n".to_vec());
                        log::trace!("DSR: status report -> OK");
                    }
                    6 => {
                        // Cursor position report.
                        let row = self.cursor_row + 1;
                        let col = self.cursor_col + 1;
                        let response = format!("[{row};{col}R");
                        self.queue_response(response.into_bytes());
                        log::trace!("DSR: cursor position -> {row};{col}");
                    }
                    _ => {
                        log::trace!("DSR: unhandled request {}", param(0, 0));
                    }
                }
            }
            // DECSTBM — Set Top and Bottom Margins.
            ([], 'r') => {
                if self.rows == 0 { return; }
                let top = param(0, 1) as usize;
                let bottom = param(1, self.rows as u16) as usize;
                self.scroll_top = top.saturating_sub(1);
                self.scroll_bottom = (bottom.saturating_sub(1)).min(self.rows.saturating_sub(1));
                if self.scroll_top >= self.scroll_bottom {
                    // Invalid region (top >= bottom); reset to full screen.
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows.saturating_sub(1);
                }
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
            // SM — Set Mode (ANSI modes).
            ([], 'h') => {
                match param(0, 0) {
                    4 => self.modes.insert_mode = true,
                    20 => {
                        self.modes.linefeed_mode = true;
                        log::trace!("LNM on");
                    }
                    _ => {}
                }
            }
            // RM — Reset Mode (ANSI modes).
            ([], 'l') => {
                match param(0, 0) {
                    4 => self.modes.insert_mode = false,
                    20 => {
                        self.modes.linefeed_mode = false;
                        log::trace!("LNM off");
                    }
                    _ => {}
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
            // DA — Device Attributes (Primary).
            ([], 'c') => {
                if param(0, 0) == 0 {
                    // Respond as VT220 with Sixel, DRCS support.
                    // Attributes: 62=VT220, 4=Sixel, 22=ANSI color
                    self.queue_response(b"[?62;4;22c".to_vec());
                    log::trace!("DA: primary device attributes");
                }
            }
            // DA2 — Secondary Device Attributes.
            ([b'>'], 'c') => {
                if param(0, 0) == 0 {
                    // Respond as VT220, firmware version 1, ROM cartridge 0.
                    self.queue_response(b"[>1;1;0c".to_vec());
                    log::trace!("DA2: secondary device attributes");
                }
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
            // OSC 1 — Set icon name (often treated same as title).
            "1" => {
                if let Some(name) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(name) {
                        log::trace!("icon name: {s}");
                        // Many terminals treat icon name = title.
                        // We store it in the title for simplicity.
                    }
                }
            }
            // OSC 10 — Query/set default foreground color.
            "10" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with current foreground color.
                            // Use a sensible default (white-ish for dark themes).
                            log::trace!("OSC 10 query -> default fg");
                            self.queue_response(
                                b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\".to_vec()
                            );
                        } else {
                            log::trace!("OSC 10 set fg: {s}");
                            // Setting foreground color — store for theme-aware apps.
                            // Actual color application depends on the renderer.
                        }
                    }
                }
            }
            // OSC 11 — Query/set default background color.
            "11" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with current background color.
                            // Use a sensible default (dark for dark themes).
                            log::trace!("OSC 11 query -> default bg");
                            self.queue_response(
                                b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\".to_vec()
                            );
                        } else {
                            log::trace!("OSC 11 set bg: {s}");
                        }
                    }
                }
            }
            // OSC 12 — Query/set cursor color.
            "12" => {
                if let Some(color_param) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(color_param) {
                        if s == "?" {
                            // Query: respond with cursor color.
                            log::trace!("OSC 12 query -> cursor color");
                            self.queue_response(
                                b"\x1b]12;rgb:cccc/cccc/cccc\x1b\\".to_vec()
                            );
                        } else {
                            log::trace!("OSC 12 set cursor color: {s}");
                        }
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
            //
            // vte splits OSC payloads at ';', so an iTerm2 sequence like:
            //   OSC 1337 ; File=inline=1;size=123:BASE64 ST
            // arrives as params = ["1337", "File=inline=1", "size=123:BASE64"].
            // We must rejoin params[1..] with ';' to reconstruct the full payload.
            "1337" => {
                if params.len() < 2 {
                    return;
                }
                // Rejoin all params after "1337" to reconstruct the original payload.
                let payload = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");

                if payload.starts_with("File=") {
                    // Check if this is a file transfer (inline=0) or inline image.
                    let is_inline = payload.contains("inline=1");
                    let is_non_inline = payload.contains("inline=0") || !payload.contains("inline=");

                    if is_inline {
                        // Legacy single-sequence inline image.
                        let col = self.cursor_col;
                        let row = self.cursor_row;
                        let before = self.image_store.placements().len();
                        image::parse_iterm2_image(
                            &payload,
                            &mut self.image_store,
                            col,
                            row,
                        );
                        // Advance cursor past the image so text flows below it.
                        if self.image_store.placements().len() > before {
                            self.image_store.cap_placement_size(self.cols, self.rows);
                            let cell_rows = self.image_store.placements().last()
                                .map(|p| p.cell_rows).unwrap_or(1);
                            self.advance_cursor_past_image(cell_rows);
                        }
                    } else if is_non_inline {
                        // File transfer: decode and emit a FileTransferEvent.
                        if let Some(rest) = payload.strip_prefix("File=") {
                            if let Some(colon_idx) = rest.rfind(':') {
                                let params_str = &rest[..colon_idx];
                                let b64_data = &rest[colon_idx + 1..];

                                // Parse file name from params.
                                let mut file_name = String::new();
                                for kv in params_str.split(';') {
                                    let mut parts = kv.splitn(2, '=');
                                    let key = parts.next().unwrap_or("");
                                    let value = parts.next().unwrap_or("");
                                    if key == "name" {
                                        file_name = base64::engine::general_purpose::STANDARD
                                            .decode(value)
                                            .ok()
                                            .and_then(|b| String::from_utf8(b).ok())
                                            .unwrap_or_default();
                                    }
                                }

                                // Decode file data.
                                if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(b64_data) {
                                    self.file_transfer_event = Some(FileTransferEvent {
                                        name: file_name,
                                        data,
                                    });
                                    log::debug!("iTerm2 file transfer: received file");
                                }
                            }
                        }
                    }
                } else if payload.starts_with("MultipartFile=") {
                    // Multipart transfer: begin (metadata, no pixel data).
                    self.iterm2_accumulator.begin(&payload);
                } else if payload.starts_with("FilePart=") {
                    // Multipart transfer: data chunk.
                    self.iterm2_accumulator.add_part(&payload);
                } else if payload == "FileEnd" {
                    // Multipart transfer: finalize.
                    let col = self.cursor_col;
                    let row = self.cursor_row;
                    let placed = self.iterm2_accumulator.finish(
                        &mut self.image_store,
                        col,
                        row,
                    );
                    if placed {
                        self.image_store.cap_placement_size(self.cols, self.rows);
                        let cell_rows = self.image_store.placements().last()
                            .map(|p| p.cell_rows).unwrap_or(1);
                        self.advance_cursor_past_image(cell_rows);
                    }
                } else {
                    log::trace!("OSC 1337: unhandled sub-command: {}", &payload[..payload.len().min(30)]);
                }
            }
            _ => {
                log::trace!("unhandled OSC: {cmd}");
            }
        }
    }

    fn hook(
        &mut self,
        params: &vte::Params,
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
            // DECDLD — Dynamically Redefinable Character Set (soft fonts).
            //
            // Format: DCS Pfn ; Pcn ; Pe ; Pcmw ; Pss ; Pt ; Pcmh ; Pcss { Dscs <data> ST
            //   Pfn = font number (0-3)
            //   Pcn = starting character code (0-95, added to 0x20)
            //   Pe  = erase control (0=erase all, 1=erase loaded chars only, 2=erase all)
            //   Pcmw = character cell width (0 = default)
            //   Pss  = font set size (0=80-col, 1=132-col, 2=both)
            //   Pt   = text/full cell (0=text, 1=full cell, 2=text)
            //   Pcmh = character cell height (0 = default)
            //   Pcss = character set size (0=94, 1=96)
            //
            // The '{' final byte introduces the font data.
            '{' => {
                let p: Vec<u16> = params.iter()
                    .map(|s| s.first().copied().unwrap_or(0))
                    .collect();
                let pfn = p.first().copied().unwrap_or(0).min(3) as u8;
                let pcn = p.get(1).copied().unwrap_or(0).min(95) as u8;
                let pe = p.get(2).copied().unwrap_or(0).min(2) as u8;
                let pcmw = p.get(3).copied().unwrap_or(0) as u8;
                let pcmh = p.get(6).copied().unwrap_or(0) as u8;

                // Default cell dimensions based on font size.
                let cell_width = if pcmw == 0 { 10 } else { pcmw };
                let cell_height = if pcmh == 0 { 20 } else { pcmh };

                log::debug!(
                    "DCS hook: DECDLD font={pfn} start_char={pcn} erase={pe}                      cell={}x{}",
                    cell_width, cell_height
                );

                // Erase existing glyphs per the erase control parameter.
                match pe {
                    0 | 2 => self.drcs_fonts.erase_font(pfn),
                    1 => {} // Only erase chars being loaded (handled during glyph parsing).
                    _ => {}
                }

                self.dcs_mode = DcsMode::Decdld {
                    font_number: pfn,
                    start_char: pcn,
                    cell_width,
                    cell_height,
                    erase_control: pe,
                };
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
            DcsMode::Sixel | DcsMode::Decdld { .. } => {
                self.dcs_data.push(byte);
            }
            DcsMode::None => {}
        }
    }

    fn unhook(&mut self) {
        match std::mem::replace(&mut self.dcs_mode, DcsMode::None) {
            DcsMode::Sixel => {
                log::trace!("DCS unhook: Sixel ({} bytes)", self.dcs_data.len());
                let data = std::mem::take(&mut self.dcs_data);
                let col = self.cursor_col;
                let row = self.cursor_row;
                let before = self.image_store.placements().len();
                image::process_sixel(&data, &mut self.image_store, col, row);
                if self.image_store.placements().len() > before {
                    self.image_store.cap_placement_size(self.cols, self.rows);
                    let cell_rows = self.image_store.placements().last()
                        .map(|p| p.cell_rows).unwrap_or(1);
                    self.advance_cursor_past_image(cell_rows);
                }
            }
            DcsMode::Decdld {
                font_number,
                start_char,
                cell_width,
                cell_height,
                ..
            } => {
                let data = std::mem::take(&mut self.dcs_data);
                log::debug!(
                    "DCS unhook: DECDLD font={font_number} ({} bytes)",
                    data.len()
                );
                // Skip the Dscs (designator) byte(s) if present.
                // The data format after Dscs is: rows of sixel-like data separated by ';'.
                // Each glyph row is separated by '/'.
                let body = if let Some(_pos) = data.iter().position(|&b| b == b'/') {
                    // There may be leading designator chars before the first data.
                    // Actually, the Dscs is a single character set designator like 'B' or '@'.
                    // For simplicity, we treat the entire data as glyph definitions.
                    &data[..]
                } else {
                    &data[..]
                };

                // Parse glyph data: glyphs are separated by ';'.
                // Within each glyph, sixel rows are separated by '/'.
                let mut char_code = start_char;
                for glyph_data in body.split(|&b| b == b';') {
                    if glyph_data.is_empty() {
                        char_code = char_code.wrapping_add(1);
                        continue;
                    }

                    let w = cell_width as usize;
                    let h = cell_height as usize;
                    // Each row is a sequence of sixel-encoded columns.
                    // Rows within a glyph are separated by '/'.
                    let mut bitmap = vec![0u8; (w * h + 7) / 8];
                    let mut pixel_y = 0usize;

                    for row_data in glyph_data.split(|&b| b == b'/') {
                        let mut pixel_x = 0usize;
                        let mut i = 0;
                        while i < row_data.len() {
                            let b = row_data[i];
                            if b == b'!' {
                                // Repeat: !<count><char>
                                i += 1;
                                let mut count = 0usize;
                                while i < row_data.len() && row_data[i].is_ascii_digit() {
                                    count = count * 10 + (row_data[i] - b'0') as usize;
                                    i += 1;
                                }
                                if i < row_data.len() && row_data[i] >= 0x3F && row_data[i] <= 0x7E {
                                    let val = row_data[i] - 0x3F;
                                    for _ in 0..count {
                                        for bit in 0..6u8 {
                                            if val & (1 << bit) != 0 {
                                                let py = pixel_y + bit as usize;
                                                if pixel_x < w && py < h {
                                                    let bit_idx = py * w + pixel_x;
                                                    bitmap[bit_idx / 8] |= 1 << (7 - (bit_idx % 8));
                                                }
                                            }
                                        }
                                        pixel_x += 1;
                                    }
                                    i += 1;
                                }
                                continue;
                            }
                            if b >= 0x3F && b <= 0x7E {
                                let val = b - 0x3F;
                                for bit in 0..6u8 {
                                    if val & (1 << bit) != 0 {
                                        let py = pixel_y + bit as usize;
                                        if pixel_x < w && py < h {
                                            let bit_idx = py * w + pixel_x;
                                            bitmap[bit_idx / 8] |= 1 << (7 - (bit_idx % 8));
                                        }
                                    }
                                }
                                pixel_x += 1;
                            }
                            i += 1;
                        }
                        pixel_y += 6;
                    }

                    self.drcs_fonts.set_glyph(
                        font_number,
                        char_code,
                        DrcsGlyph {
                            bitmap,
                            width: cell_width,
                            height: cell_height,
                        },
                    );
                    log::trace!(
                        "DECDLD: defined glyph font={font_number} char=0x{:02X}",
                        0x20 + char_code
                    );
                    char_code = char_code.wrapping_add(1);
                }
            }
            DcsMode::None => {
                log::trace!("DCS unhook");
            }
        }
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

    #[test]
    fn test_char_width_basic() {
        // ASCII characters are always 1-cell wide.
        assert_eq!(char_width('A', false), 1);
        assert_eq!(char_width('A', true), 1);

        // CJK ideographs are always 2-cell wide.
        assert_eq!(char_width('あ', false), 2);
        assert_eq!(char_width('あ', true), 2);
        assert_eq!(char_width('漢', false), 2);
        assert_eq!(char_width('漢', true), 2);

        // Fullwidth forms are always 2-cell wide.
        assert_eq!(char_width('！', false), 2);
        assert_eq!(char_width('！', true), 2);
    }

    #[test]
    fn test_char_width_ambiguous() {
        // U+25EF LARGE CIRCLE — East Asian Width: Ambiguous.
        // Narrow in Western locales, wide in CJK locales.
        assert_eq!(char_width('\u{25EF}', false), 1);
        assert_eq!(char_width('\u{25EF}', true), 2);

        // Other common ambiguous-width characters:
        // ○ U+25CB WHITE CIRCLE
        assert_eq!(char_width('\u{25CB}', false), 1);
        assert_eq!(char_width('\u{25CB}', true), 2);

        // ● U+25CF BLACK CIRCLE
        assert_eq!(char_width('\u{25CF}', false), 1);
        assert_eq!(char_width('\u{25CF}', true), 2);

        // ■ U+25A0 BLACK SQUARE
        assert_eq!(char_width('\u{25A0}', false), 1);
        assert_eq!(char_width('\u{25A0}', true), 2);

        // △ U+25B3 WHITE UP-POINTING TRIANGLE
        assert_eq!(char_width('\u{25B3}', false), 1);
        assert_eq!(char_width('\u{25B3}', true), 2);

        // ★ U+2605 BLACK STAR
        assert_eq!(char_width('\u{2605}', false), 1);
        assert_eq!(char_width('\u{2605}', true), 2);

        // ① U+2460 CIRCLED DIGIT ONE
        assert_eq!(char_width('\u{2460}', false), 1);
        assert_eq!(char_width('\u{2460}', true), 2);
    }

    #[test]
    fn test_cjk_width_terminal_print() {
        // Test that ◯ (U+25EF) is stored as width-2 when cjk_width is enabled.
        let mut term = Terminal::new(80, 24);
        term.set_cjk_width(true);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\u{25EF}");

        // The character should be stored in cell (0, 0) with width 2.
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, '\u{25EF}');
        assert_eq!(cell.width, 2);

        // Cell (1, 0) should be a continuation cell (width 0, NUL char).
        let cont = term.grid().cell(1, 0);
        assert_eq!(cont.c, '\0');
        assert_eq!(cont.width, 0);

        // Cursor should be at column 2 (after the 2-cell wide character).
        assert_eq!(term.cursor_col, 2);
    }

    #[test]
    fn test_narrow_width_terminal_print() {
        // Test that ◯ (U+25EF) is stored as width-1 when cjk_width is disabled.
        let mut term = Terminal::new(80, 24);
        term.set_cjk_width(false);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "\u{25EF}");

        // The character should be stored in cell (0, 0) with width 1.
        let cell = term.grid().cell(0, 0);
        assert_eq!(cell.c, '\u{25EF}');
        assert_eq!(cell.width, 1);

        // Cursor should be at column 1.
        assert_eq!(term.cursor_col, 1);
    }

    // =========================================================================
    // Tests for Issue 9, 16, 17 — new terminal protocol features
    // =========================================================================

    // --- DECCKM (mode 1) ---

    #[test]
    fn test_decckm_application_cursor_keys() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.application_cursor_keys);

        // Enable DECCKM.
        feed_str(&mut term, &mut parser, "\x1b[?1h");
        assert!(term.modes.application_cursor_keys);

        // Disable DECCKM.
        feed_str(&mut term, &mut parser, "\x1b[?1l");
        assert!(!term.modes.application_cursor_keys);
    }

    // --- DECOM (mode 6) ---

    #[test]
    fn test_decom_origin_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.origin_mode);

        // Set scroll region.
        feed_str(&mut term, &mut parser, "\x1b[5;20r");
        // Move cursor somewhere.
        feed_str(&mut term, &mut parser, "\x1b[10;10H");
        assert_eq!(term.cursor_row, 9);
        assert_eq!(term.cursor_col, 9);

        // Enable origin mode — cursor should move to scroll region origin.
        feed_str(&mut term, &mut parser, "\x1b[?6h");
        assert!(term.modes.origin_mode);
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 4); // scroll_top = 4 (row 5, 0-indexed)

        // Disable origin mode — cursor should move to absolute origin.
        feed_str(&mut term, &mut parser, "\x1b[?6l");
        assert!(!term.modes.origin_mode);
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 0);
    }

    // --- X10 mouse (mode 9) ---

    #[test]
    fn test_x10_mouse_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert_eq!(term.modes.mouse_mode, MouseMode::None);

        feed_str(&mut term, &mut parser, "\x1b[?9h");
        assert_eq!(term.modes.mouse_mode, MouseMode::Click);

        feed_str(&mut term, &mut parser, "\x1b[?9l");
        assert_eq!(term.modes.mouse_mode, MouseMode::None);
    }

    // --- DECSDM (mode 80) ---

    #[test]
    fn test_decsdm_sixel_display_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.sixel_display_mode);

        feed_str(&mut term, &mut parser, "\x1b[?80h");
        assert!(term.modes.sixel_display_mode);

        feed_str(&mut term, &mut parser, "\x1b[?80l");
        assert!(!term.modes.sixel_display_mode);
    }

    // --- Mode 1047 (alt screen without cursor save) ---

    #[test]
    fn test_mode_1047_alt_screen() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        feed_str(&mut term, &mut parser, "Main");
        assert_eq!(term.grid().cell(0, 0).c, 'M');

        // Enter alt screen via mode 1047.
        feed_str(&mut term, &mut parser, "\x1b[?1047h");
        assert!(term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, ' '); // Alt screen is clear.

        // Leave alt screen via mode 1047.
        feed_str(&mut term, &mut parser, "\x1b[?1047l");
        assert!(!term.modes.alternate_screen);
        assert_eq!(term.grid().cell(0, 0).c, 'M'); // Main screen restored.
    }

    // --- Mode 1048 (save/restore cursor) ---

    #[test]
    fn test_mode_1048_save_restore_cursor() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Move cursor to (9, 4).
        feed_str(&mut term, &mut parser, "\x1b[5;10H");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);

        // Save cursor via mode 1048.
        feed_str(&mut term, &mut parser, "\x1b[?1048h");

        // Move cursor elsewhere.
        feed_str(&mut term, &mut parser, "\x1b[1;1H");
        assert_eq!(term.cursor_row, 0);
        assert_eq!(term.cursor_col, 0);

        // Restore cursor via mode 1048.
        feed_str(&mut term, &mut parser, "\x1b[?1048l");
        assert_eq!(term.cursor_row, 4);
        assert_eq!(term.cursor_col, 9);
    }

    // --- LNM (ANSI mode 20) ---

    #[test]
    fn test_lnm_linefeed_mode() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.modes.linefeed_mode);

        // Set LNM.
        feed_str(&mut term, &mut parser, "\x1b[20h");
        assert!(term.modes.linefeed_mode);

        // Move cursor to column 5.
        feed_str(&mut term, &mut parser, "\x1b[1;6H");
        assert_eq!(term.cursor_col, 5);

        // LF should also do CR when LNM is set.
        feed_str(&mut term, &mut parser, "\n");
        assert_eq!(term.cursor_col, 0);
        assert_eq!(term.cursor_row, 1);

        // Reset LNM.
        feed_str(&mut term, &mut parser, "\x1b[20l");
        assert!(!term.modes.linefeed_mode);

        // Move cursor to column 5 again.
        feed_str(&mut term, &mut parser, "\x1b[3;6H");
        assert_eq!(term.cursor_col, 5);

        // LF should NOT do CR when LNM is off.
        feed_str(&mut term, &mut parser, "\n");
        assert_eq!(term.cursor_col, 5);
    }

    // --- OSC 10/11/12 Color Queries ---

    #[test]
    fn test_osc10_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query foreground color: OSC 10 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]10;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).unwrap();
        assert!(resp.contains("]10;rgb:"));
    }

    #[test]
    fn test_osc11_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query background color: OSC 11 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]11;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).unwrap();
        assert!(resp.contains("]11;rgb:"));
    }

    #[test]
    fn test_osc12_color_query() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Query cursor color: OSC 12 ; ? ST
        feed_str(&mut term, &mut parser, "\x1b]12;?\x1b\\");

        assert!(term.has_pending_responses());
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).unwrap();
        assert!(resp.contains("]12;rgb:"));
    }

    #[test]
    fn test_osc10_set_color_no_response() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Setting a color should not produce a response.
        feed_str(&mut term, &mut parser, "\x1b]10;rgb:ffff/0000/0000\x1b\\");

        assert!(!term.has_pending_responses());
    }

    // --- DSR (Device Status Report) ---

    #[test]
    fn test_dsr_status_report() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // DSR status request (5).
        feed_str(&mut term, &mut parser, "\x1b[5n");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[0n");
    }

    #[test]
    fn test_dsr_cursor_position_report() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Move cursor to (9, 4).
        feed_str(&mut term, &mut parser, "\x1b[5;10H");

        // DSR cursor position request (6).
        feed_str(&mut term, &mut parser, "\x1b[6n");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[5;10R");
    }

    // --- DA (Device Attributes) ---

    #[test]
    fn test_da_primary() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Primary DA: CSI 0 c
        feed_str(&mut term, &mut parser, "\x1b[0c");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).unwrap();
        assert!(resp.starts_with("\x1b[?"));
        // Should indicate VT220, Sixel support.
        assert!(resp.contains("62"));
        assert!(resp.contains("4"));
    }

    #[test]
    fn test_da_secondary() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Secondary DA: CSI > 0 c
        feed_str(&mut term, &mut parser, "\x1b[>0c");

        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        let resp = std::str::from_utf8(&responses[0]).unwrap();
        assert!(resp.starts_with("\x1b[>"));
    }

    // --- DRCS / DECDLD ---

    #[test]
    fn test_drcs_font_store() {
        let mut store = DrcsFontStore::new();
        assert!(store.is_empty());

        store.set_glyph(0, 0, DrcsGlyph {
            bitmap: vec![0xFF, 0x00],
            width: 10,
            height: 20,
        });

        assert!(!store.is_empty());
        assert!(store.get_glyph(0, 0).is_some());
        assert!(store.get_glyph(0, 1).is_none());
        assert!(store.get_glyph(1, 0).is_none());

        let glyph = store.get_glyph(0, 0).unwrap();
        assert_eq!(glyph.width, 10);
        assert_eq!(glyph.height, 20);

        // Erase font 0.
        store.erase_font(0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_drcs_font_store_erase_all() {
        let mut store = DrcsFontStore::new();
        store.set_glyph(0, 0, DrcsGlyph {
            bitmap: vec![0xFF],
            width: 8,
            height: 16,
        });
        store.set_glyph(1, 0, DrcsGlyph {
            bitmap: vec![0xFF],
            width: 8,
            height: 16,
        });
        assert_eq!(store.glyphs().len(), 2);

        store.erase_all();
        assert!(store.is_empty());
    }

    #[test]
    fn test_decdld_basic_dcs_hook() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Send DECDLD: DCS 0;0;0;10;0;0;20;0 { B <data> ST
        // This defines font 0, starting at char 0, 10x20 cell.
        // Minimal sixel data: one glyph with a single column of all-on pixels.
        // DCS format: ESC P <params> { <Dscs> <data> ESC backslash
        feed_str(&mut term, &mut parser, "\x1bP0;0;0;10;0;0;20;0{B~\x1b\\");

        // The DRCS font store should now have at least one glyph.
        assert!(!term.drcs_fonts.is_empty());
    }

    // --- Response queue ---

    #[test]
    fn test_response_queue_drain() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();
        assert!(!term.has_pending_responses());

        // Trigger two responses.
        feed_str(&mut term, &mut parser, "\x1b[5n"); // DSR status
        assert!(term.has_pending_responses());

        // Drain.
        let responses: Vec<Vec<u8>> = term.drain_responses().collect();
        assert_eq!(responses.len(), 1);
        assert!(!term.has_pending_responses());
    }

    // --- Multiple modes in single sequence ---

    #[test]
    fn test_multiple_private_modes() {
        let mut term = Terminal::new(80, 24);
        let mut parser = vte::Parser::new();

        // Set multiple modes at once: DECCKM + DECAWM + bracketed paste.
        feed_str(&mut term, &mut parser, "\x1b[?1;7;2004h");
        assert!(term.modes.application_cursor_keys);
        assert!(term.modes.auto_wrap);
        assert!(term.modes.bracketed_paste);

        // Reset them.
        feed_str(&mut term, &mut parser, "\x1b[?1;7;2004l");
        assert!(!term.modes.application_cursor_keys);
        assert!(!term.modes.auto_wrap);
        assert!(!term.modes.bracketed_paste);
    }
}
