use crate::color::Color;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    /// Cell rendering attributes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Attrs: u16 {
        const BOLD           = 1 << 0;
        const DIM            = 1 << 1;
        const ITALIC         = 1 << 2;
        const UNDERLINE      = 1 << 3;
        const BLINK          = 1 << 4;
        const REVERSE        = 1 << 5;
        const HIDDEN         = 1 << 6;
        const STRIKETHROUGH  = 1 << 7;
        const DOUBLE_UNDERLINE = 1 << 8;
        const CURLY_UNDERLINE  = 1 << 9;
        const DOTTED_UNDERLINE = 1 << 10;
        const DASHED_UNDERLINE = 1 << 11;
    }
}

impl Serialize for Attrs {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bits().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Attrs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bits = u16::deserialize(deserializer)?;
        Ok(Self::from_bits_truncate(bits))
    }
}

/// A single terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// The character displayed in this cell (NUL for empty).
    pub c: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Rendering attributes.
    pub attrs: Attrs,
    /// Underline color (separate from fg, used by neovim diagnostics).
    pub underline_color: Color,
    /// Display width: 1 for normal, 2 for wide (CJK/emoji), 0 for continuation.
    pub width: u8,
    /// Whether this cell is part of a hyperlink (OSC 8).
    pub hyperlink: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
            underline_color: Color::Default,
            width: 1,
            hyperlink: false,
        }
    }
}

impl Cell {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The "pen" — current style attributes applied to new characters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub underline_color: Color,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
            underline_color: Color::Default,
        }
    }
}

impl Pen {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
