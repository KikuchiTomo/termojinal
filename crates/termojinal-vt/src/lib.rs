//! VT parser and cell grid for termojinal.
//!
//! Provides a terminal state machine that parses VT escape sequences
//! and maintains a cell grid with full color and attribute support.
//!
//! Image display protocols (Kitty Graphics, iTerm2 Inline Images, Sixel)
//! are provided by the `image` module.

pub mod cell;
pub mod color;
pub mod grid;
pub mod image;
pub mod scrollback;
pub mod term;

pub use cell::{Attrs, Cell, Pen};
pub use color::{Color, NamedColor};
pub use grid::Grid;
pub use image::{
    ApcExtractor, ImagePlacement, ImageStore, KittyAccumulator, TerminalImage,
};
pub use scrollback::ScrollbackBuffer;
pub use term::{
    ClipboardEvent, CommandRecord, CursorShape, Modes, MouseFormat, MouseMode, NamedSnapshot,
    OscState, Terminal, TerminalSnapshot,
};
