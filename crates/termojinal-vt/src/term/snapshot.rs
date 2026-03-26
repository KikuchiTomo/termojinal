//! Terminal state snapshots for session persistence.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::cell::{Cell, Pen};

use super::command::CommandRecord;
use super::modes::{CursorShape, Modes, SavedCursor};
use super::Terminal;

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
    /// Terminal modes (bracketed paste, mouse, cursor keys, etc.).
    #[serde(default)]
    pub modes: Modes,
    /// Scroll region top (inclusive).
    #[serde(default)]
    pub scroll_top: usize,
    /// Scroll region bottom (inclusive).
    #[serde(default)]
    pub scroll_bottom: usize,
    /// Alternate screen grid cells (row-major).
    #[serde(default)]
    pub alt_grid_cells: Option<Vec<Vec<Cell>>>,
    /// Whether the terminal is currently on the alternate screen.
    #[serde(default)]
    pub using_alt: bool,
    /// Saved cursor for main screen.
    #[serde(default)]
    pub saved_cursor_main: Option<SavedCursor>,
    /// Saved cursor for alternate screen.
    #[serde(default)]
    pub saved_cursor_alt: Option<SavedCursor>,
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

impl Terminal {
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

        // Capture alternate screen grid.
        let alt_grid_cells = {
            let ag = &self.alt_grid;
            let mut cells = Vec::with_capacity(ag.rows());
            for row in 0..ag.rows() {
                let mut row_cells = Vec::with_capacity(ag.cols());
                for col in 0..ag.cols() {
                    row_cells.push(*ag.cell(col, row));
                }
                cells.push(row_cells);
            }
            Some(cells)
        };

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
            modes: self.modes,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            alt_grid_cells,
            using_alt: self.using_alt,
            saved_cursor_main: self.saved_cursor_main,
            saved_cursor_alt: self.saved_cursor_alt,
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
        term.next_command_id = snapshot.command_history.back().map_or(0, |c| c.id + 1);
        term.osc.title = snapshot.title.clone();
        term.osc.cwd = snapshot.cwd.clone();

        // Restore modes.
        term.modes = snapshot.modes;
        term.scroll_top = snapshot.scroll_top;
        term.scroll_bottom = snapshot.scroll_bottom.min(snapshot.rows.saturating_sub(1));
        term.using_alt = snapshot.using_alt;
        term.saved_cursor_main = snapshot.saved_cursor_main;
        term.saved_cursor_alt = snapshot.saved_cursor_alt;

        // Restore alternate screen grid.
        if let Some(ref alt_cells) = snapshot.alt_grid_cells {
            for (row_idx, row_cells) in alt_cells.iter().enumerate() {
                if row_idx >= term.alt_grid.rows() {
                    break;
                }
                for (col_idx, cell) in row_cells.iter().enumerate() {
                    if col_idx >= term.alt_grid.cols() {
                        break;
                    }
                    *term.alt_grid.cell_mut(col_idx, row_idx) = *cell;
                }
            }
        }

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
}
