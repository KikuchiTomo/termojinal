use serde::{Deserialize, Serialize};

/// Terminal color representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    /// Default foreground or background color (from theme).
    Default,
    /// Named ANSI color (0–15).
    Named(NamedColor),
    /// 256-color palette index (0–255).
    Indexed(u8),
    /// 24-bit true color.
    Rgb(u8, u8, u8),
}

/// The 16 standard ANSI named colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum NamedColor {
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
}

impl NamedColor {
    pub fn from_sgr_fg(code: u16) -> Option<Self> {
        match code {
            30 => Some(Self::Black),
            31 => Some(Self::Red),
            32 => Some(Self::Green),
            33 => Some(Self::Yellow),
            34 => Some(Self::Blue),
            35 => Some(Self::Magenta),
            36 => Some(Self::Cyan),
            37 => Some(Self::White),
            90 => Some(Self::BrightBlack),
            91 => Some(Self::BrightRed),
            92 => Some(Self::BrightGreen),
            93 => Some(Self::BrightYellow),
            94 => Some(Self::BrightBlue),
            95 => Some(Self::BrightMagenta),
            96 => Some(Self::BrightCyan),
            97 => Some(Self::BrightWhite),
            _ => None,
        }
    }

    pub fn from_sgr_bg(code: u16) -> Option<Self> {
        match code {
            40 => Some(Self::Black),
            41 => Some(Self::Red),
            42 => Some(Self::Green),
            43 => Some(Self::Yellow),
            44 => Some(Self::Blue),
            45 => Some(Self::Magenta),
            46 => Some(Self::Cyan),
            47 => Some(Self::White),
            100 => Some(Self::BrightBlack),
            101 => Some(Self::BrightRed),
            102 => Some(Self::BrightGreen),
            103 => Some(Self::BrightYellow),
            104 => Some(Self::BrightBlue),
            105 => Some(Self::BrightMagenta),
            106 => Some(Self::BrightCyan),
            107 => Some(Self::BrightWhite),
            _ => None,
        }
    }
}
