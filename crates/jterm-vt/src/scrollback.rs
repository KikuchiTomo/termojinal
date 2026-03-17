//! Scrollback buffer with Hot (in-memory) and Warm (mmap) tiers.
//!
//! Hot tier: recent lines kept in a `VecDeque` for fast access.
//! Warm tier: overflow lines written to a memory-mapped file at
//! `~/.local/share/jterm/scrollback/{session-id}.bin`.

use crate::cell::{Attrs, Cell};
use crate::color::{Color, NamedColor};
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Default hot tier capacity.
pub const DEFAULT_HOT_CAPACITY: usize = 10_000;

/// Size of a serialized Cell in bytes.
/// char(4) + fg(4) + bg(4) + attrs(2) + underline_color(4) + width(1) + hyperlink(1) = 20
const CELL_BYTES: usize = 20;

/// Size of the row header (column count as u16).
const ROW_HEADER_BYTES: usize = 2;

/// A two-tier scrollback buffer.
///
/// Lines are pushed into the hot tier (in-memory `VecDeque`). When the hot
/// tier exceeds its capacity, the oldest lines are evicted to the warm tier
/// (an mmap'd binary file). Reading checks hot first, then warm.
pub struct ScrollbackBuffer {
    /// Hot tier: recent lines in memory (front = oldest, back = newest).
    hot: VecDeque<Vec<Cell>>,
    /// Max lines in hot tier before eviction to warm.
    hot_capacity: usize,
    /// Warm tier: older lines in a memory-mapped file.
    /// Wrapped in UnsafeCell because `get()` needs to call `WarmTier::read_row`
    /// (which requires `&mut`) through a `&self` reference. This is safe because
    /// Terminal is single-threaded.
    warm: UnsafeCell<Option<WarmTier>>,
    /// Session ID used for the warm tier file name.
    session_id: String,
    /// Whether the warm tier has been initialized (lazy creation).
    warm_initialized: bool,
    /// Read cache for warm tier rows. Uses UnsafeCell for interior mutability
    /// so that `get()` can work with `&self`. This is safe because Terminal
    /// is single-threaded and the cache is only overwritten on the next call.
    warm_read_cache: UnsafeCell<WarmReadCache>,
}

/// Cache for the most recently read warm-tier row.
struct WarmReadCache {
    /// The warm-tier line index that is currently cached, or None.
    cached_index: Option<usize>,
    /// The deserialized row data.
    data: Vec<Cell>,
}

struct WarmTier {
    /// Path to the scrollback file.
    path: PathBuf,
    /// The file handle (kept open for appending).
    file: fs::File,
    /// Memory-mapped view for reading.
    mmap: Option<memmap2::Mmap>,
    /// Number of lines stored in the warm tier.
    line_count: usize,
    /// Byte offset index: line_offsets[i] is the byte offset where line i starts.
    line_offsets: Vec<u64>,
    /// Current file size in bytes (for tracking where to append).
    file_size: u64,
}

impl WarmTier {
    fn new(path: PathBuf) -> std::io::Result<Self> {
        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .read(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            mmap: None,
            line_count: 0,
            line_offsets: Vec::new(),
            file_size: 0,
        })
    }

    /// Append a row to the warm tier file.
    fn push_row(&mut self, row: &[Cell]) -> std::io::Result<()> {
        let offset = self.file_size;
        self.line_offsets.push(offset);

        let cols = row.len() as u16;
        let mut buf = Vec::with_capacity(ROW_HEADER_BYTES + row.len() * CELL_BYTES);

        // Row header: column count.
        buf.extend_from_slice(&cols.to_le_bytes());

        // Serialize each cell.
        for cell in row {
            serialize_cell(cell, &mut buf);
        }

        self.file.write_all(&buf)?;
        self.file_size += buf.len() as u64;
        self.line_count += 1;

        // Invalidate the current mmap so it gets refreshed on next read.
        self.mmap = None;

        Ok(())
    }

    /// Ensure the mmap is up to date.
    fn ensure_mmap(&mut self) -> std::io::Result<()> {
        if self.mmap.is_none() && self.file_size > 0 {
            self.file.flush()?;
            // Safety: the file is only appended to by this process. The mmap
            // is read-only and the file contents are stable for all bytes up
            // to file_size at the time the mmap was created.
            let mmap = unsafe { memmap2::Mmap::map(&self.file)? };
            self.mmap = Some(mmap);
        }
        Ok(())
    }

    /// Read a row from the warm tier by line index (0 = oldest warm-tier line).
    fn read_row(&mut self, line_idx: usize) -> Option<Vec<Cell>> {
        if line_idx >= self.line_count {
            return None;
        }

        if self.ensure_mmap().is_err() {
            return None;
        }

        let mmap = self.mmap.as_ref()?;
        let offset = self.line_offsets[line_idx] as usize;

        if offset + ROW_HEADER_BYTES > mmap.len() {
            return None;
        }

        let cols = u16::from_le_bytes([mmap[offset], mmap[offset + 1]]) as usize;
        let data_start = offset + ROW_HEADER_BYTES;
        let data_end = data_start + cols * CELL_BYTES;

        if data_end > mmap.len() {
            return None;
        }

        let mut cells = Vec::with_capacity(cols);
        for i in 0..cols {
            let cell_start = data_start + i * CELL_BYTES;
            let cell_bytes = &mmap[cell_start..cell_start + CELL_BYTES];
            cells.push(deserialize_cell(cell_bytes));
        }

        Some(cells)
    }

    /// Clear the warm tier, removing all data.
    fn clear(&mut self) -> std::io::Result<()> {
        self.mmap = None;
        self.line_count = 0;
        self.line_offsets.clear();
        self.file_size = 0;
        self.file.set_len(0)?;
        Ok(())
    }
}

impl Drop for WarmTier {
    fn drop(&mut self) {
        // Drop the mmap before removing the file.
        self.mmap = None;
        let _ = fs::remove_file(&self.path);
    }
}

/// Serialize a Color to 4 bytes: [type, a, b, c].
fn serialize_color(color: &Color, buf: &mut Vec<u8>) {
    match color {
        Color::Default => buf.extend_from_slice(&[0x00, 0, 0, 0]),
        Color::Named(n) => buf.extend_from_slice(&[0x01, *n as u8, 0, 0]),
        Color::Indexed(n) => buf.extend_from_slice(&[0x02, *n, 0, 0]),
        Color::Rgb(r, g, b) => buf.extend_from_slice(&[0x03, *r, *g, *b]),
    }
}

/// Deserialize a Color from 4 bytes.
fn deserialize_color(bytes: &[u8]) -> Color {
    match bytes[0] {
        0x01 => {
            let n = bytes[1];
            // Map u8 back to NamedColor.
            match n {
                0 => Color::Named(NamedColor::Black),
                1 => Color::Named(NamedColor::Red),
                2 => Color::Named(NamedColor::Green),
                3 => Color::Named(NamedColor::Yellow),
                4 => Color::Named(NamedColor::Blue),
                5 => Color::Named(NamedColor::Magenta),
                6 => Color::Named(NamedColor::Cyan),
                7 => Color::Named(NamedColor::White),
                8 => Color::Named(NamedColor::BrightBlack),
                9 => Color::Named(NamedColor::BrightRed),
                10 => Color::Named(NamedColor::BrightGreen),
                11 => Color::Named(NamedColor::BrightYellow),
                12 => Color::Named(NamedColor::BrightBlue),
                13 => Color::Named(NamedColor::BrightMagenta),
                14 => Color::Named(NamedColor::BrightCyan),
                15 => Color::Named(NamedColor::BrightWhite),
                _ => Color::Default,
            }
        }
        0x02 => Color::Indexed(bytes[1]),
        0x03 => Color::Rgb(bytes[1], bytes[2], bytes[3]),
        _ => Color::Default,
    }
}

/// Serialize a Cell to CELL_BYTES bytes.
fn serialize_cell(cell: &Cell, buf: &mut Vec<u8>) {
    // char: 4 bytes (u32 LE)
    buf.extend_from_slice(&(cell.c as u32).to_le_bytes());
    // fg: 4 bytes
    serialize_color(&cell.fg, buf);
    // bg: 4 bytes
    serialize_color(&cell.bg, buf);
    // attrs: 2 bytes (u16 LE)
    buf.extend_from_slice(&cell.attrs.bits().to_le_bytes());
    // underline_color: 4 bytes
    serialize_color(&cell.underline_color, buf);
    // width: 1 byte
    buf.push(cell.width);
    // hyperlink: 1 byte
    buf.push(if cell.hyperlink { 1 } else { 0 });
}

/// Deserialize a Cell from CELL_BYTES bytes.
fn deserialize_cell(bytes: &[u8]) -> Cell {
    let c_u32 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let c = char::from_u32(c_u32).unwrap_or(' ');
    let fg = deserialize_color(&bytes[4..8]);
    let bg = deserialize_color(&bytes[8..12]);
    let attrs_bits = u16::from_le_bytes([bytes[12], bytes[13]]);
    let attrs = Attrs::from_bits_truncate(attrs_bits);
    let underline_color = deserialize_color(&bytes[14..18]);
    let width = bytes[18];
    let hyperlink = bytes[19] != 0;

    Cell {
        c,
        fg,
        bg,
        attrs,
        underline_color,
        width,
        hyperlink,
    }
}

/// Get the scrollback directory path.
fn scrollback_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("jterm")
        .join("scrollback")
}

impl ScrollbackBuffer {
    /// Create a new scrollback buffer with the given session ID and hot tier capacity.
    pub fn new(session_id: &str, hot_capacity: usize) -> Self {
        Self {
            hot: VecDeque::new(),
            hot_capacity,
            warm: UnsafeCell::new(None),
            session_id: session_id.to_string(),
            warm_initialized: false,
            warm_read_cache: UnsafeCell::new(WarmReadCache {
                cached_index: None,
                data: Vec::new(),
            }),
        }
    }

    /// Create a new scrollback buffer with a generated session ID.
    pub fn with_defaults() -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self::new(&session_id, DEFAULT_HOT_CAPACITY)
    }

    /// Get a mutable reference to the warm tier (for &mut self methods).
    fn warm_mut(&mut self) -> &mut Option<WarmTier> {
        self.warm.get_mut()
    }

    /// Get a reference to the warm tier line count (safe via UnsafeCell for &self).
    fn warm_line_count(&self) -> usize {
        // Safety: we only read the line_count field; no concurrent mutation.
        unsafe { (*self.warm.get()).as_ref().map_or(0, |w| w.line_count) }
    }

    /// Total number of lines across both tiers.
    pub fn len(&self) -> usize {
        self.warm_line_count() + self.hot.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Push a new line (called when grid scrolls up).
    /// Lines are appended to the hot tier. If hot tier exceeds capacity,
    /// the oldest lines are evicted to the warm tier.
    pub fn push(&mut self, row: Vec<Cell>) {
        self.hot.push_back(row);

        // Evict from hot to warm if over capacity.
        while self.hot.len() > self.hot_capacity {
            if let Some(evicted) = self.hot.pop_front() {
                self.evict_to_warm(&evicted);
            }
        }
    }

    /// Evict a row to the warm tier.
    fn evict_to_warm(&mut self, row: &[Cell]) {
        // Lazy initialization of warm tier.
        if !self.warm_initialized {
            self.warm_initialized = true;
            let path = scrollback_dir().join(format!("{}.bin", self.session_id));
            match WarmTier::new(path) {
                Ok(warm) => *self.warm_mut() = Some(warm),
                Err(e) => {
                    log::warn!("failed to create warm tier scrollback file: {e}");
                    // Fall back to hot-only mode: just drop the evicted row.
                    return;
                }
            }
        }

        if let Some(ref mut warm) = self.warm_mut() {
            if let Err(e) = warm.push_row(row) {
                log::warn!("failed to write to warm tier: {e}");
            }
        }
    }

    /// Get a line by index (0 = most recent, len()-1 = oldest).
    /// Hot tier is checked first, then warm tier.
    ///
    /// For hot tier lines, returns a direct reference.
    /// For warm tier lines, deserializes from the mmap into an internal cache
    /// and returns a reference to the cached data.
    ///
    /// # Safety
    /// Uses interior mutability (UnsafeCell) for the warm tier and read cache.
    /// This is safe because Terminal is single-threaded and the returned
    /// reference is only valid until the next call to `get()`.
    pub fn get(&self, index: usize) -> Option<&[Cell]> {
        let total = self.len();
        if index >= total {
            return None;
        }

        let hot_len = self.hot.len();

        if index < hot_len {
            // Hot tier: index 0 = most recent = back of VecDeque.
            let deque_idx = hot_len - 1 - index;
            Some(&self.hot[deque_idx])
        } else {
            // Warm tier: convert to warm-tier line index.
            // index hot_len corresponds to the newest warm-tier line (last appended).
            // Warm lines are stored oldest-first (line 0 = oldest).
            let warm_count = self.warm_line_count();
            let warm_relative = index - hot_len;
            // warm_relative 0 = the most recent warm line = warm_count - 1
            let warm_idx = warm_count - 1 - warm_relative;

            // Safety: single-threaded access. We need &mut to call read_row
            // (which may refresh the mmap) and to update the read cache.
            let cache = unsafe { &mut *self.warm_read_cache.get() };
            if cache.cached_index != Some(warm_idx) {
                let warm = unsafe { &mut *self.warm.get() };
                let warm_ref = warm.as_mut()?;
                cache.data = warm_ref.read_row(warm_idx)?;
                cache.cached_index = Some(warm_idx);
            }
            Some(&cache.data)
        }
    }

    /// Clear all scrollback data.
    pub fn clear(&mut self) {
        self.hot.clear();
        if let Some(ref mut warm) = self.warm_mut() {
            if let Err(e) = warm.clear() {
                log::warn!("failed to clear warm tier: {e}");
            }
        }
        // Reset the warm read cache.
        let cache = self.warm_read_cache.get_mut();
        cache.cached_index = None;
        cache.data.clear();
    }
}

// Safety: ScrollbackBuffer is only accessed from a single thread (the main
// terminal processing thread). The UnsafeCell is used solely for interior
// mutability of the warm read cache and is never accessed concurrently.
unsafe impl Send for ScrollbackBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(c: char, cols: usize) -> Vec<Cell> {
        let mut row = vec![Cell::default(); cols];
        row[0].c = c;
        row
    }

    #[test]
    fn test_hot_only_push_get() {
        let mut buf = ScrollbackBuffer::new("test-hot", 100);
        buf.push(make_row('A', 10));
        buf.push(make_row('B', 10));
        buf.push(make_row('C', 10));

        assert_eq!(buf.len(), 3);
        // Index 0 = most recent = 'C'
        assert_eq!(buf.get(0).unwrap()[0].c, 'C');
        assert_eq!(buf.get(1).unwrap()[0].c, 'B');
        assert_eq!(buf.get(2).unwrap()[0].c, 'A');
        assert!(buf.get(3).is_none());
    }

    #[test]
    fn test_hot_capacity_eviction() {
        let mut buf = ScrollbackBuffer::new("test-evict", 3);
        buf.push(make_row('A', 10));
        buf.push(make_row('B', 10));
        buf.push(make_row('C', 10));
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.hot.len(), 3);

        // Push a 4th line, which should evict 'A' to warm.
        buf.push(make_row('D', 10));
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.hot.len(), 3);

        // Index 0 = most recent = 'D'
        assert_eq!(buf.get(0).unwrap()[0].c, 'D');
        assert_eq!(buf.get(1).unwrap()[0].c, 'C');
        assert_eq!(buf.get(2).unwrap()[0].c, 'B');
        // 'A' is in the warm tier.
        assert_eq!(buf.get(3).unwrap()[0].c, 'A');
    }

    #[test]
    fn test_warm_tier_multiple_evictions() {
        let mut buf = ScrollbackBuffer::new("test-warm-multi", 2);
        for i in 0..10u32 {
            let c = char::from_u32('A' as u32 + i).unwrap();
            buf.push(make_row(c, 5));
        }

        assert_eq!(buf.len(), 10);
        assert_eq!(buf.hot.len(), 2);

        // Most recent = 'J' (index 0), oldest = 'A' (index 9).
        assert_eq!(buf.get(0).unwrap()[0].c, 'J');
        assert_eq!(buf.get(1).unwrap()[0].c, 'I');
        assert_eq!(buf.get(2).unwrap()[0].c, 'H');
        assert_eq!(buf.get(9).unwrap()[0].c, 'A');
    }

    #[test]
    fn test_cell_serialization_roundtrip() {
        let cell = Cell {
            c: '\u{1F600}', // emoji
            fg: Color::Rgb(255, 128, 0),
            bg: Color::Named(NamedColor::Blue),
            attrs: Attrs::BOLD | Attrs::ITALIC,
            underline_color: Color::Indexed(42),
            width: 2,
            hyperlink: true,
        };

        let mut buf = Vec::new();
        serialize_cell(&cell, &mut buf);
        assert_eq!(buf.len(), CELL_BYTES);

        let deserialized = deserialize_cell(&buf);
        assert_eq!(deserialized.c, cell.c);
        assert_eq!(deserialized.fg, cell.fg);
        assert_eq!(deserialized.bg, cell.bg);
        assert_eq!(deserialized.attrs, cell.attrs);
        assert_eq!(deserialized.underline_color, cell.underline_color);
        assert_eq!(deserialized.width, cell.width);
        assert_eq!(deserialized.hyperlink, cell.hyperlink);
    }

    #[test]
    fn test_color_serialization_roundtrip() {
        let colors = vec![
            Color::Default,
            Color::Named(NamedColor::Red),
            Color::Named(NamedColor::BrightWhite),
            Color::Indexed(0),
            Color::Indexed(128),
            Color::Indexed(255),
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            Color::Rgb(100, 200, 50),
        ];

        for color in &colors {
            let mut buf = Vec::new();
            serialize_color(color, &mut buf);
            assert_eq!(buf.len(), 4);
            let deserialized = deserialize_color(&buf);
            assert_eq!(&deserialized, color, "roundtrip failed for {:?}", color);
        }
    }

    #[test]
    fn test_clear() {
        let mut buf = ScrollbackBuffer::new("test-clear", 3);
        for i in 0..10u32 {
            let c = char::from_u32('A' as u32 + i).unwrap();
            buf.push(make_row(c, 5));
        }
        assert_eq!(buf.len(), 10);

        buf.clear();
        assert_eq!(buf.len(), 0);
        assert!(buf.get(0).is_none());
    }

    #[test]
    fn test_empty_buffer() {
        let buf = ScrollbackBuffer::new("test-empty", 100);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        assert!(buf.get(0).is_none());
    }

    #[test]
    fn test_default_cell_roundtrip() {
        let cell = Cell::default();
        let mut buf = Vec::new();
        serialize_cell(&cell, &mut buf);
        let deserialized = deserialize_cell(&buf);
        assert_eq!(deserialized.c, cell.c);
        assert_eq!(deserialized.fg, cell.fg);
        assert_eq!(deserialized.bg, cell.bg);
        assert_eq!(deserialized.attrs, cell.attrs);
        assert_eq!(deserialized.width, cell.width);
        assert_eq!(deserialized.hyperlink, cell.hyperlink);
    }

    #[test]
    fn test_with_defaults() {
        let buf = ScrollbackBuffer::with_defaults();
        assert_eq!(buf.hot_capacity, DEFAULT_HOT_CAPACITY);
        assert!(!buf.session_id.is_empty());
    }

    #[test]
    fn test_warm_tier_boundary_access() {
        // Test accessing lines right at the hot/warm boundary.
        let mut buf = ScrollbackBuffer::new("test-boundary", 5);
        for i in 0..8u32 {
            let c = char::from_u32('A' as u32 + i).unwrap();
            buf.push(make_row(c, 3));
        }

        // 8 lines total, 5 hot, 3 warm.
        assert_eq!(buf.len(), 8);
        assert_eq!(buf.hot.len(), 5);

        // Hot tier: D(4), E(5), F(6), G(7), H(8) -> indices 0..4
        assert_eq!(buf.get(0).unwrap()[0].c, 'H');
        assert_eq!(buf.get(4).unwrap()[0].c, 'D');

        // Warm tier: A(1), B(2), C(3) -> indices 5..7
        assert_eq!(buf.get(5).unwrap()[0].c, 'C');
        assert_eq!(buf.get(6).unwrap()[0].c, 'B');
        assert_eq!(buf.get(7).unwrap()[0].c, 'A');
    }
}
