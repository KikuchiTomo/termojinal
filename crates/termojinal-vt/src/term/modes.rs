//! Terminal mode flags and related type definitions.

use serde::{Deserialize, Serialize};

use crate::cell::Pen;

/// DCS accumulation mode.
#[derive(Debug, PartialEq)]
pub(crate) enum DcsMode {
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedCursor {
    pub col: usize,
    pub row: usize,
    pub pen: Pen,
    pub cursor_visible: bool,
    pub cursor_shape: CursorShape,
}

/// Which mouse events to report to the application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
