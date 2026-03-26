//! Terminal state machine implementing the VT parser's `Perform` trait.

pub mod command;
pub mod csi;
pub mod dcs;
pub mod modes;
pub mod osc;
pub mod print;
pub mod snapshot;
mod tests;

use crate::cell::Pen;
use crate::grid::Grid;
use crate::image::{ApcExtractor, ImageStore, Iterm2Accumulator, KittyAccumulator};
use crate::scrollback::ScrollbackBuffer;
use std::collections::VecDeque;

pub use command::CommandRecord;
pub use modes::{
    ClipboardEvent, CursorShape, DrcsFontStore, DrcsGlyph, FileTransferEvent, Modes, MouseFormat,
    MouseMode, OscState,
};
pub(crate) use modes::DcsMode;
pub use modes::SavedCursor;
pub use print::char_width;
pub use snapshot::{NamedSnapshot, TerminalSnapshot};

use command::PendingCommand;

/// The full terminal state.
pub struct Terminal {
    /// Main screen grid.
    pub(crate) main_grid: Grid,
    /// Alternate screen grid.
    pub(crate) alt_grid: Grid,
    /// Whether we're on the alternate screen.
    pub(crate) using_alt: bool,
    /// Cursor position.
    pub cursor_col: usize,
    pub cursor_row: usize,
    /// Cursor shape.
    pub cursor_shape: CursorShape,
    /// Current pen (attributes for new characters).
    pub(crate) pen: Pen,
    /// Scroll region top (inclusive).
    pub(crate) scroll_top: usize,
    /// Scroll region bottom (inclusive).
    pub(crate) scroll_bottom: usize,
    /// Saved cursor (main screen).
    pub(crate) saved_cursor_main: Option<SavedCursor>,
    /// Saved cursor (alt screen).
    pub(crate) saved_cursor_alt: Option<SavedCursor>,
    /// Terminal modes.
    pub modes: Modes,
    /// OSC-derived state.
    pub osc: OscState,
    /// Terminal dimensions.
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// Pending wrap: if true, next printable char triggers a newline first.
    pub(crate) wrap_pending: bool,
    /// Tab stops.
    pub(crate) tab_stops: Vec<bool>,
    /// Scrollback buffer with hot (in-memory) and warm (mmap) tiers.
    pub(crate) scrollback: ScrollbackBuffer,
    /// Current scroll offset (0 = live view, >0 = looking at history).
    pub(crate) scroll_offset: usize,
    /// Kitty keyboard protocol flags stack.
    pub(crate) kitty_keyboard_flags: Vec<u32>,
    /// Current hyperlink URI (set by OSC 8).
    pub(crate) current_hyperlink: Option<String>,
    /// Clipboard event from OSC 52 (consumed by application layer).
    pub clipboard_event: Option<ClipboardEvent>,
    /// Image store for Kitty, iTerm2, and Sixel image protocols.
    pub image_store: ImageStore,
    /// APC sequence extractor (Kitty Graphics uses APC, which vte ignores).
    pub(crate) apc_extractor: ApcExtractor,
    /// Kitty Graphics chunked transfer accumulator.
    pub(crate) kitty_accumulator: KittyAccumulator,
    /// iTerm2 multipart image transfer accumulator.
    pub(crate) iterm2_accumulator: Iterm2Accumulator,
    /// DCS data accumulation buffer (for Sixel and DECDLD).
    pub(crate) dcs_data: Vec<u8>,
    /// Current DCS accumulation mode.
    pub(crate) dcs_mode: DcsMode,
    /// DRCS soft font store (DECDLD-defined custom glyphs).
    pub drcs_fonts: DrcsFontStore,
    /// Pending responses to be written back to the PTY.
    ///
    /// Escape sequences like DSR, DA, OSC 10/11/12 color queries produce
    /// response bytes that the application layer must send to the PTY master.
    pub(crate) pending_responses: VecDeque<Vec<u8>>,
    /// File transfer event from iTerm2 OSC 1337 (inline=0).
    pub file_transfer_event: Option<FileTransferEvent>,
    /// Completed command records (time travel history).
    pub(crate) command_history: VecDeque<CommandRecord>,
    /// Next command ID to assign.
    pub(crate) next_command_id: u64,
    /// Command currently being tracked between OSC 133 markers.
    pub(crate) pending_command: Option<PendingCommand>,
    /// Total number of lines that have been scrolled into the scrollback buffer.
    /// Used to compute absolute line numbers: abs_line = total_scrolled_lines + screen_row.
    pub(crate) total_scrolled_lines: usize,
    /// Whether command history tracking is enabled (config: time_travel.command_history).
    pub(crate) command_history_enabled: bool,
    /// Maximum number of command records to keep.
    pub(crate) max_command_history: usize,
    /// Temporary storage for command text extracted at OSC 133 C, used at D.
    pub(crate) pending_command_text: Option<String>,
    /// Whether to use CJK-aware character width calculation.
    /// When true, Unicode East Asian Ambiguous width characters (e.g., ○●■□▲△◆◇★☆◯)
    /// are treated as 2-cell wide instead of 1-cell wide.
    pub cjk_width: bool,
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

    pub(crate) fn grid_mut(&mut self) -> &mut Grid {
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
    pub(crate) fn queue_response(&mut self, data: Vec<u8>) {
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
    pub fn scrollback_row(&self, idx: usize) -> Option<&[crate::cell::Cell]> {
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
            crate::image::process_kitty_command(&cmd, &decoded, &mut self.image_store, col, row);
            // Advance cursor past the image if a new placement was added.
            if self.image_store.placements().len() > before {
                self.image_store.cap_placement_size(self.cols, self.rows);
                let cell_rows = self
                    .image_store
                    .placements()
                    .last()
                    .map(|p| p.cell_rows)
                    .unwrap_or(1);
                self.advance_cursor_past_image(cell_rows);
            }
        }
    }

    pub(crate) fn clamp_col(&self, col: usize) -> usize {
        col.min(self.cols.saturating_sub(1))
    }

    pub(crate) fn clamp_row(&self, row: usize) -> usize {
        row.min(self.rows.saturating_sub(1))
    }

    /// Perform a newline: move cursor down, scroll if at bottom of scroll region.
    pub(crate) fn newline(&mut self) {
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

    pub(crate) fn reverse_index(&mut self) {
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
    pub(crate) fn advance_cursor_past_image(&mut self, cell_rows: usize) {
        self.cursor_col = 0;
        self.wrap_pending = false;
        for _ in 0..cell_rows {
            self.newline();
        }
    }

    /// Enter alternate screen buffer.
    pub(crate) fn enter_alt_screen(&mut self) {
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
    pub(crate) fn leave_alt_screen(&mut self) {
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
}

/// Thin delegation layer for `vte::Perform`.
impl vte::Perform for Terminal {
    fn print(&mut self, c: char) {
        self.do_print(c);
    }

    fn execute(&mut self, byte: u8) {
        self.do_execute(byte);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.do_esc_dispatch(intermediates, ignore, byte);
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        self.do_csi_dispatch(params, intermediates, ignore, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.do_osc_dispatch(params, bell_terminated);
    }

    fn hook(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        self.do_dcs_hook(params, intermediates, ignore, action);
    }

    fn put(&mut self, byte: u8) {
        self.do_dcs_put(byte);
    }

    fn unhook(&mut self) {
        self.do_dcs_unhook();
    }
}
