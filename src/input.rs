//! Keyboard input encoding.

use winit::keyboard::{Key, ModifiersState, NamedKey};

pub(crate) fn key_to_binding_string(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> Option<String> {
    let mut parts = Vec::new();

    if modifiers.super_key() {
        parts.push("cmd");
    }
    if modifiers.control_key() {
        parts.push("ctrl");
    }
    if modifiers.alt_key() {
        parts.push("alt");
    }
    if modifiers.shift_key() {
        parts.push("shift");
    }

    let key_name = match &event.logical_key {
        Key::Character(c) => {
            // Return lowercase character as the key name.
            let s = c.to_lowercase();
            Some(s)
        }
        Key::Named(named) => {
            let name = match named {
                NamedKey::Enter => "enter",
                NamedKey::Tab => "tab",
                NamedKey::Space => "space",
                NamedKey::Escape => "escape",
                NamedKey::Backspace => "backspace",
                NamedKey::Delete => "delete",
                NamedKey::ArrowUp => "up",
                NamedKey::ArrowDown => "down",
                NamedKey::ArrowLeft => "left",
                NamedKey::ArrowRight => "right",
                NamedKey::Home => "home",
                NamedKey::End => "end",
                NamedKey::PageUp => "pageup",
                NamedKey::PageDown => "pagedown",
                NamedKey::Insert => "insert",
                NamedKey::F1 => "f1",
                NamedKey::F2 => "f2",
                NamedKey::F3 => "f3",
                NamedKey::F4 => "f4",
                NamedKey::F5 => "f5",
                NamedKey::F6 => "f6",
                NamedKey::F7 => "f7",
                NamedKey::F8 => "f8",
                NamedKey::F9 => "f9",
                NamedKey::F10 => "f10",
                NamedKey::F11 => "f11",
                NamedKey::F12 => "f12",
                _ => return None, // Modifier-only key or unknown
            };
            Some(name.to_string())
        }
        _ => None,
    };

    let key_name = key_name?;
    if parts.is_empty() && key_name.len() == 1 {
        // Single character with no modifier — not a binding, let key_to_bytes handle it.
        return None;
    }
    parts.push(&key_name);
    // Only produce a binding string if there is at least one modifier.
    if !modifiers.is_empty() {
        Some(parts.join("+"))
    } else {
        // Named keys without modifiers (e.g., "enter", "escape") — not bindings.
        None
    }
}

// ---------------------------------------------------------------------------
// Keyboard → PTY byte translation
// ---------------------------------------------------------------------------

pub(crate) fn key_to_bytes(event: &winit::event::KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    // Ctrl+key → control codes.
    if modifiers.control_key() {
        if let Key::Character(ref c) = event.logical_key {
            let ch = c.chars().next()?;
            match ch.to_ascii_lowercase() {
                'a'..='z' => return Some(vec![ch.to_ascii_lowercase() as u8 - b'a' + 1]),
                '[' => return Some(vec![0x1B]),
                '\\' => return Some(vec![0x1C]),
                ']' => return Some(vec![0x1D]),
                '^' | '6' => return Some(vec![0x1E]),
                '_' | '7' => return Some(vec![0x1F]),
                '@' | '2' | ' ' => return Some(vec![0x00]),
                _ => {}
            }
        }
    }

    // Named keys → escape sequences.
    if let Key::Named(ref named) = event.logical_key {
        match named {
            NamedKey::Enter => {
                // Shift+Enter sends LF (\n) — used by Claude Code for newline input.
                if modifiers.shift_key() {
                    return Some(b"\n".to_vec());
                }
                return Some(b"\r".to_vec());
            }
            NamedKey::Backspace => return Some(vec![0x7F]),
            NamedKey::Tab => return Some(b"\t".to_vec()),
            NamedKey::Space => return Some(b" ".to_vec()),
            NamedKey::Escape => return Some(vec![0x1B]),
            NamedKey::ArrowUp => return Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => return Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => return Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => return Some(b"\x1b[D".to_vec()),
            NamedKey::Home => return Some(b"\x1b[H".to_vec()),
            NamedKey::End => return Some(b"\x1b[F".to_vec()),
            NamedKey::PageUp => return Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => return Some(b"\x1b[6~".to_vec()),
            NamedKey::Delete => return Some(b"\x1b[3~".to_vec()),
            NamedKey::Insert => return Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => return Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => return Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => return Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => return Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => return Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => return Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => return Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => return Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => return Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => return Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => return Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => return Some(b"\x1b[24~".to_vec()),
            _ => {}
        }
    }

    // Regular text input.
    if let Some(ref text) = event.text {
        if !text.is_empty() {
            return Some(text.as_bytes().to_vec());
        }
    }

    // Fallback: character key.
    if let Key::Character(ref c) = event.logical_key {
        return Some(c.as_bytes().to_vec());
    }

    None
}

// ---------------------------------------------------------------------------
// Mouse → escape sequence encoding (SGR format)
// ---------------------------------------------------------------------------

/// Encode a mouse event as an SGR escape sequence to forward to the PTY.
/// `btn`: 0=left, 1=middle, 2=right; add 32 for motion, 64 for scroll
/// `col`, `row`: 1-based grid coordinates
/// `pressed`: true for press (M), false for release (m)
pub(crate) fn encode_mouse_sgr(btn: u8, col: usize, row: usize, pressed: bool) -> Vec<u8> {
    let suffix = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{btn};{};{}{suffix}", col + 1, row + 1).into_bytes()
}

// ---------------------------------------------------------------------------
// winit ApplicationHandler
// ---------------------------------------------------------------------------

