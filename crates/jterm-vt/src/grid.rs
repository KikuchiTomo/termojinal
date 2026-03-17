use crate::cell::Cell;

/// The terminal screen buffer: a 2D grid of cells.
#[derive(Debug, Clone)]
pub struct Grid {
    cols: usize,
    rows: usize,
    /// Row-major cell storage.
    cells: Vec<Cell>,
    /// Per-row dirty flags: true means the row has been modified since last clear.
    /// Uses interior mutability so dirty flags can be cleared with only `&self`,
    /// enabling the renderer to clear them after drawing without `&mut` access.
    dirty_rows: Vec<std::cell::Cell<bool>>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            dirty_rows: vec![std::cell::Cell::new(true); rows],
        }
    }

    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    #[inline]
    fn idx(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    #[inline]
    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        &self.cells[self.idx(col, row)]
    }

    #[inline]
    pub fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        self.dirty_rows[row].set(true);
        let i = self.idx(col, row);
        &mut self.cells[i]
    }

    /// Clear a row with default cells.
    pub fn clear_row(&mut self, row: usize) {
        self.dirty_rows[row].set(true);
        let start = self.idx(0, row);
        for i in start..start + self.cols {
            self.cells[i] = Cell::default();
        }
    }

    /// Clear the entire grid.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
        for d in &self.dirty_rows {
            d.set(true);
        }
    }

    /// Clear cells from (col, row) to end of line.
    pub fn clear_to_eol(&mut self, col: usize, row: usize) {
        self.dirty_rows[row].set(true);
        let start = self.idx(col, row);
        let end = self.idx(0, row) + self.cols;
        for i in start..end {
            self.cells[i] = Cell::default();
        }
    }

    /// Clear cells from start of line to (col, row) inclusive.
    pub fn clear_from_bol(&mut self, col: usize, row: usize) {
        self.dirty_rows[row].set(true);
        let row_start = self.idx(0, row);
        let end = self.idx(col, row) + 1;
        for i in row_start..end {
            self.cells[i] = Cell::default();
        }
    }

    /// Scroll lines up within a region [top, bottom].
    /// The top line is removed and a blank line is inserted at the bottom.
    pub fn scroll_up(&mut self, top: usize, bottom: usize, count: usize) {
        for _ in 0..count {
            // Shift rows up by one within the region.
            for row in top..bottom {
                let src_start = self.idx(0, row + 1);
                let dst_start = self.idx(0, row);
                // Copy row+1 into row.
                for c in 0..self.cols {
                    self.cells[dst_start + c] = self.cells[src_start + c];
                }
                self.dirty_rows[row].set(true);
            }
            self.clear_row(bottom);
        }
    }

    /// Scroll lines down within a region [top, bottom].
    /// The bottom line is removed and a blank line is inserted at the top.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, count: usize) {
        for _ in 0..count {
            for row in (top + 1..=bottom).rev() {
                let src_start = self.idx(0, row - 1);
                let dst_start = self.idx(0, row);
                for c in 0..self.cols {
                    self.cells[dst_start + c] = self.cells[src_start + c];
                }
                self.dirty_rows[row].set(true);
            }
            self.clear_row(top);
        }
    }

    /// Insert blank lines at the given row, shifting subsequent lines down.
    pub fn insert_lines(&mut self, row: usize, count: usize, bottom: usize) {
        let n = count.min(bottom - row + 1);
        self.scroll_down(row, bottom, n);
    }

    /// Delete lines at the given row, shifting subsequent lines up.
    pub fn delete_lines(&mut self, row: usize, count: usize, bottom: usize) {
        let n = count.min(bottom - row + 1);
        self.scroll_up(row, bottom, n);
    }

    /// Insert blank cells at (col, row), shifting existing cells right.
    pub fn insert_cells(&mut self, col: usize, row: usize, count: usize) {
        self.dirty_rows[row].set(true);
        let row_start = self.idx(0, row);
        let n = count.min(self.cols - col);
        // Shift right.
        for c in (col + n..self.cols).rev() {
            self.cells[row_start + c] = self.cells[row_start + c - n];
        }
        // Clear inserted cells.
        for c in col..col + n {
            self.cells[row_start + c] = Cell::default();
        }
    }

    /// Delete cells at (col, row), shifting remaining cells left.
    pub fn delete_cells(&mut self, col: usize, row: usize, count: usize) {
        self.dirty_rows[row].set(true);
        let row_start = self.idx(0, row);
        let n = count.min(self.cols - col);
        for c in col..self.cols - n {
            self.cells[row_start + c] = self.cells[row_start + c + n];
        }
        for c in (self.cols - n)..self.cols {
            self.cells[row_start + c] = Cell::default();
        }
    }

    /// Resize the grid, preserving content where possible.
    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let mut new_cells = vec![Cell::default(); new_cols * new_rows];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);
        for row in 0..copy_rows {
            for col in 0..copy_cols {
                new_cells[row * new_cols + col] = self.cells[row * self.cols + col];
            }
        }
        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.dirty_rows = vec![std::cell::Cell::new(true); new_rows];
    }

    /// Copy a row's cells for scrollback storage.
    pub fn row_cells(&self, row: usize) -> Vec<Cell> {
        let start = self.idx(0, row);
        self.cells[start..start + self.cols].to_vec()
    }

    /// Erase characters from (col, row) to end of screen.
    pub fn erase_below(&mut self, col: usize, row: usize) {
        self.clear_to_eol(col, row);
        for r in (row + 1)..self.rows {
            self.clear_row(r);
        }
    }

    /// Erase characters from start of screen to (col, row).
    pub fn erase_above(&mut self, col: usize, row: usize) {
        self.clear_from_bol(col, row);
        for r in 0..row {
            self.clear_row(r);
        }
    }

    /// Returns true if the given row has been modified since the last `clear_dirty()`.
    #[inline]
    pub fn is_row_dirty(&self, row: usize) -> bool {
        self.dirty_rows[row].get()
    }

    /// Returns true if any row has been modified since the last `clear_dirty()`.
    pub fn any_dirty(&self) -> bool {
        self.dirty_rows.iter().any(|d| d.get())
    }

    /// Mark all rows as clean. Uses interior mutability so this works with `&self`.
    pub fn clear_dirty(&self) {
        for d in &self.dirty_rows {
            d.set(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_grid() {
        let grid = Grid::new(80, 24);
        assert_eq!(grid.cols(), 80);
        assert_eq!(grid.rows(), 24);
        assert_eq!(grid.cell(0, 0).c, ' ');
    }

    #[test]
    fn test_scroll_up() {
        let mut grid = Grid::new(4, 4);
        grid.cell_mut(0, 0).c = 'A';
        grid.cell_mut(0, 1).c = 'B';
        grid.cell_mut(0, 2).c = 'C';
        grid.cell_mut(0, 3).c = 'D';
        grid.scroll_up(0, 3, 1);
        assert_eq!(grid.cell(0, 0).c, 'B');
        assert_eq!(grid.cell(0, 1).c, 'C');
        assert_eq!(grid.cell(0, 2).c, 'D');
        assert_eq!(grid.cell(0, 3).c, ' ');
    }

    #[test]
    fn test_resize() {
        let mut grid = Grid::new(4, 4);
        grid.cell_mut(0, 0).c = 'X';
        grid.resize(8, 6);
        assert_eq!(grid.cols(), 8);
        assert_eq!(grid.rows(), 6);
        assert_eq!(grid.cell(0, 0).c, 'X');
    }
}
