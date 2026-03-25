//! macOS clipboard operations via NSPasteboard with RTF support.

use crate::Pane;

pub(crate) fn sel_bounds_for(pane: &Pane) -> Option<((usize, usize), (usize, usize))> {
    match &pane.selection {
        Some(s) if !s.is_empty() => {
            let ((sc, abs_sr), (ec, abs_er)) = s.ordered_abs();
            let current_scroll = pane.terminal.scroll_offset() as isize;
            let rows = pane.terminal.rows() as isize;

            // Convert absolute row to screen-relative:
            // screen_row = abs_row + current_scroll_offset
            let vis_sr = abs_sr + current_scroll;
            let vis_er = abs_er + current_scroll;

            // If the selection is entirely outside the viewport, return None.
            if vis_er < 0 || vis_sr >= rows {
                return None;
            }

            // Clamp to viewport bounds.
            let clamped_sr = vis_sr.max(0) as usize;
            let clamped_er = (vis_er.min(rows - 1)) as usize;
            let clamped_sc = if vis_sr >= 0 { sc } else { 0 };
            let clamped_ec = if vis_er < rows {
                ec
            } else {
                pane.terminal.cols().saturating_sub(1)
            };

            Some(((clamped_sc, clamped_sr), (clamped_ec, clamped_er)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rich-text copy (RTF with colors preserved) — macOS
// ---------------------------------------------------------------------------

/// Resolve a terminal `Color` to `(r, g, b)` u8 values using the theme palette.
pub(crate) fn color_to_rgb(
    color: termojinal_vt::Color,
    is_fg: bool,
    palette: &termojinal_render::ThemePalette,
) -> (u8, u8, u8) {
    let rgba = termojinal_render::color_convert::color_to_rgba_themed(color, is_fg, palette);
    (
        (rgba[0] * 255.0).round() as u8,
        (rgba[1] * 255.0).round() as u8,
        (rgba[2] * 255.0).round() as u8,
    )
}

/// Build an RTF string from terminal cells with color and formatting attributes.
///
/// Generates RTF 1.0 with a color table derived from the cells and applies
/// bold, italic, underline, and strikethrough attributes.
pub(crate) fn cells_to_rtf(
    rows: &[Vec<termojinal_vt::Cell>],
    palette: &termojinal_render::ThemePalette,
) -> String {
    use std::collections::HashMap as RtfMap;
    use termojinal_vt::cell::Attrs;

    // Build a de-duplicated color table.
    let mut color_map: RtfMap<(u8, u8, u8), usize> = RtfMap::new();
    let mut color_list: Vec<(u8, u8, u8)> = Vec::new();

    let default_fg = color_to_rgb(termojinal_vt::Color::Default, true, palette);
    let default_bg = color_to_rgb(termojinal_vt::Color::Default, false, palette);

    let mut ensure_color = |rgb: (u8, u8, u8)| -> usize {
        let len = color_list.len();
        *color_map.entry(rgb).or_insert_with(|| {
            color_list.push(rgb);
            len
        })
    };

    // Pre-scan to populate color table.
    ensure_color(default_fg);
    ensure_color(default_bg);
    for row in rows {
        for cell in row {
            let fg = color_to_rgb(cell.fg, true, palette);
            let bg = color_to_rgb(cell.bg, true, palette);
            ensure_color(fg);
            ensure_color(bg);
        }
    }

    // RTF header.
    let mut rtf = String::from(
        "{\\rtf1\\ansi\\deff0
",
    );

    // Font table — use a monospaced font.
    rtf.push_str(
        "{\\fonttbl{\\f0\\fmodern\\fcharset0 Menlo;}}
",
    );

    // Color table.
    rtf.push_str("{\\colortbl;");
    for (r, g, b) in &color_list {
        rtf.push_str(&format!("\\red{}\\green{}\\blue{};", r, g, b));
    }
    rtf.push_str(
        "}
",
    );

    // Font size (20 = 10pt in RTF half-points).
    rtf.push_str("\\f0\\fs20 ");

    // Default colors.
    let default_fg_idx = color_map[&default_fg] + 1; // RTF color indices are 1-based
    let default_bg_idx = color_map[&default_bg] + 1;

    for (row_idx, row) in rows.iter().enumerate() {
        for cell in row {
            let fg = color_to_rgb(cell.fg, true, palette);
            let bg = color_to_rgb(cell.bg, false, palette);
            let fg_idx = color_map[&fg] + 1;
            let bg_idx = color_map[&bg] + 1;

            // Set foreground color if different from default.
            if fg_idx != default_fg_idx {
                rtf.push_str(&format!("\\cf{} ", fg_idx));
            } else {
                rtf.push_str(&format!("\\cf{} ", default_fg_idx));
            }

            // Set background color (highlight) if not default.
            if bg_idx != default_bg_idx {
                rtf.push_str(&format!("\\highlight{} ", bg_idx));
            }

            // Attributes.
            let attrs = cell.attrs;
            if attrs.contains(Attrs::BOLD) {
                rtf.push_str("\\b ");
            }
            if attrs.contains(Attrs::ITALIC) {
                rtf.push_str("\\i ");
            }
            if attrs.contains(Attrs::UNDERLINE)
                || attrs.contains(Attrs::DOUBLE_UNDERLINE)
                || attrs.contains(Attrs::CURLY_UNDERLINE)
                || attrs.contains(Attrs::DOTTED_UNDERLINE)
                || attrs.contains(Attrs::DASHED_UNDERLINE)
            {
                rtf.push_str("\\ul ");
            }
            if attrs.contains(Attrs::STRIKETHROUGH) {
                rtf.push_str("\\strike ");
            }

            // Escape the character for RTF.
            let c = cell.c;
            match c {
                '\\' => rtf.push_str("\\\\"),
                '{' => rtf.push_str("\\{"),
                '}' => rtf.push_str("\\}"),
                c if c as u32 > 127 => {
                    // Unicode character: \uN?
                    rtf.push_str(&format!("\\u{}?", c as i32));
                }
                _ => rtf.push(c),
            }

            // Reset attributes.
            if attrs.contains(Attrs::BOLD) {
                rtf.push_str("\\b0");
            }
            if attrs.contains(Attrs::ITALIC) {
                rtf.push_str("\\i0");
            }
            if attrs.contains(Attrs::UNDERLINE)
                || attrs.contains(Attrs::DOUBLE_UNDERLINE)
                || attrs.contains(Attrs::CURLY_UNDERLINE)
                || attrs.contains(Attrs::DOTTED_UNDERLINE)
                || attrs.contains(Attrs::DASHED_UNDERLINE)
            {
                rtf.push_str("\\ul0");
            }
            if attrs.contains(Attrs::STRIKETHROUGH) {
                rtf.push_str("\\strike0");
            }
            if bg_idx != default_bg_idx {
                rtf.push_str("\\highlight0");
            }
        }
        if row_idx < rows.len() - 1 {
            rtf.push_str(
                "\\par
",
            );
        }
    }

    rtf.push_str(
        "}
",
    );
    rtf
}

/// Sanitize a string by removing NUL bytes and other control characters
/// that could cause issues with NSPasteboard.
fn sanitize_for_clipboard(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\0' && (c >= ' ' || c == '\n' || c == '\r' || c == '\t'))
        .collect()
}

/// Fallback: copy plain text only using arboard (safe, no unsafe code).
fn copy_plain_text_fallback(text: &str) {
    let sanitized = sanitize_for_clipboard(text);
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(&sanitized) {
                log::error!("arboard fallback copy failed: {e}");
            }
        }
        Err(e) => {
            log::error!("failed to create arboard clipboard: {e}");
        }
    }
}

/// Copy text + RTF to the macOS clipboard using NSPasteboard.
/// Falls back to plain-text-only copy (via arboard) if the unsafe
/// NSPasteboard call panics.
pub(crate) fn copy_to_clipboard_with_rtf(plain_text: &str, rtf_text: &str) {
    let plain_owned = sanitize_for_clipboard(plain_text);
    let rtf_owned = sanitize_for_clipboard(rtf_text);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use objc2::rc::Id;
        use objc2::runtime::NSObject;
        use objc2::{class, msg_send, msg_send_id};

        unsafe {
            let pasteboard: Id<NSObject> = msg_send_id![class!(NSPasteboard), generalPasteboard];
            let () = msg_send![&*pasteboard, clearContents];

            let make_nsstring = |s: &str| -> Id<NSObject> {
                let cstr = std::ffi::CString::new(s).unwrap_or_else(|_| {
                    let cleaned: String = s.chars().filter(|&c| c != '\0').collect();
                    std::ffi::CString::new(cleaned)
                        .unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
                });
                msg_send_id![class!(NSString), stringWithUTF8String: cstr.as_ptr()]
            };

            let make_nsdata = |bytes: &[u8]| -> Id<NSObject> {
                msg_send_id![
                    class!(NSData),
                    dataWithBytes: bytes.as_ptr(),
                    length: bytes.len()
                ]
            };

            // UTType constants as NSString.
            let utf8_type = make_nsstring("public.utf8-plain-text");
            let rtf_type = make_nsstring("public.rtf");

            // Set plain text.
            let ns_text = make_nsstring(&plain_owned);
            let () = msg_send![&*pasteboard, setString: &*ns_text, forType: &*utf8_type];

            // Set RTF data.
            let rtf_data = make_nsdata(rtf_owned.as_bytes());
            let () = msg_send![&*pasteboard, setData: &*rtf_data, forType: &*rtf_type];
        }
    }));

    if let Err(e) = result {
        log::error!(
            "NSPasteboard copy panicked, falling back to plain text: {:?}",
            e.downcast_ref::<String>().map(|s| s.as_str())
        );
        copy_plain_text_fallback(plain_text);
    }
}
