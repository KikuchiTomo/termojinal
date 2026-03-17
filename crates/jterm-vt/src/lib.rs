//! VT parser and cell grid for jterm.
//!
//! Provides a terminal state machine that parses VT escape sequences
//! and maintains a cell grid with full color and attribute support.

pub mod cell;
pub mod color;
pub mod grid;
pub mod scrollback;
pub mod term;

pub use cell::{Attrs, Cell, Pen};
pub use color::{Color, NamedColor};
pub use grid::Grid;
pub use scrollback::ScrollbackBuffer;
pub use term::{ClipboardEvent, CursorShape, Modes, MouseFormat, MouseMode, OscState, Terminal};
