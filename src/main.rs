//! jterm — GPU-accelerated multi-pane terminal emulator.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use jterm_ipc::keybinding::{Action, KeybindingConfig};
use jterm_layout::{Direction, LayoutTree, PaneId, SplitDirection};
use jterm_pty::{Pty, PtyConfig, PtySize};
use jterm_render::{FontConfig, Renderer};
use jterm_vt::{ClipboardEvent, MouseMode, Terminal};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

// ---------------------------------------------------------------------------
// User events from PTY reader threads
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum UserEvent {
    PtyOutput(PaneId),
    PtyExited(PaneId),
}

// ---------------------------------------------------------------------------
// Selection state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridPos {
    col: usize,
    row: usize,
}

#[derive(Debug, Clone)]
struct Selection {
    start: GridPos,
    end: GridPos,
    active: bool, // mouse is still held
}

impl Selection {
    /// Normalize so start <= end in reading order.
    fn ordered(&self) -> (GridPos, GridPos) {
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Extract selected text from the terminal grid.
    fn text(&self, grid: &jterm_vt::Grid) -> String {
        let (s, e) = self.ordered();
        let mut result = String::new();
        for row in s.row..=e.row {
            if row >= grid.rows() {
                break;
            }
            let col_start = if row == s.row { s.col } else { 0 };
            let col_end = if row == e.row {
                e.col + 1
            } else {
                grid.cols()
            };
            for col in col_start..col_end.min(grid.cols()) {
                let cell = grid.cell(col, row);
                if cell.width > 0 && cell.c != '\0' {
                    result.push(cell.c);
                }
            }
            if row != e.row {
                let trimmed = result.trim_end().len();
                result.truncate(trimmed);
                result.push('\n');
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Pane — holds per-pane terminal + PTY state
// ---------------------------------------------------------------------------

struct Pane {
    #[allow(dead_code)]
    id: PaneId,
    terminal: Terminal,
    vt_parser: vte::Parser,
    pty: Pty,
    selection: Option<Selection>,
}

// ---------------------------------------------------------------------------
// AppState — the full multi-pane application state
// ---------------------------------------------------------------------------

struct AppState {
    window: Arc<Window>,
    renderer: Renderer,
    layout: LayoutTree,
    panes: HashMap<PaneId, Pane>,
    keybindings: KeybindingConfig,
    modifiers: ModifiersState,
    cursor_pos: (f64, f64),
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
    pty_buffers: Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            state: None,
            proxy,
            pty_buffers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// ---------------------------------------------------------------------------
// Pane spawning helper
// ---------------------------------------------------------------------------

fn spawn_pane(
    id: PaneId,
    cols: u16,
    rows: u16,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
) -> Result<Pane, jterm_pty::PtyError> {
    let config = PtyConfig {
        size: PtySize { cols, rows },
        ..PtyConfig::default()
    };
    let pty = Pty::spawn(&config)?;
    log::info!("pane {id}: shell={}, pid={}", config.shell, pty.pid());

    let terminal = Terminal::new(cols as usize, rows as usize);
    let vt_parser = vte::Parser::new();

    // Insert buffer for this pane.
    buffers.lock().unwrap().insert(id, VecDeque::new());

    // Spawn PTY reader thread.
    let master_fd = pty.master_fd();
    let proxy = proxy.clone();
    let buffers = buffers.clone();
    std::thread::Builder::new()
        .name(format!("pty-reader-{id}"))
        .spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match nix::unistd::read(master_fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut lock) = buffers.lock() {
                            if let Some(q) = lock.get_mut(&id) {
                                q.push_back(buf[..n].to_vec());
                            }
                        }
                        if proxy.send_event(UserEvent::PtyOutput(id)).is_err() {
                            break;
                        }
                    }
                    Err(nix::errno::Errno::EIO | nix::errno::Errno::EBADF) => break,
                    Err(e) => {
                        log::error!("pane {id}: PTY read error: {e}");
                        break;
                    }
                }
            }
            let _ = proxy.send_event(UserEvent::PtyExited(id));
        })
        .expect("failed to spawn pty-reader thread");

    Ok(Pane {
        id,
        terminal,
        vt_parser,
        pty,
        selection: None,
    })
}

// ---------------------------------------------------------------------------
// Keybinding string conversion
// ---------------------------------------------------------------------------

/// Convert a winit KeyEvent + modifiers into the keybinding string format
/// used by jterm-ipc (e.g., "cmd+d", "ctrl+c", "cmd+shift+enter").
fn key_to_binding_string(event: &winit::event::KeyEvent, modifiers: ModifiersState) -> Option<String> {
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

fn key_to_bytes(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> Option<Vec<u8>> {
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
            NamedKey::Enter => return Some(b"\r".to_vec()),
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
fn encode_mouse_sgr(btn: u8, col: usize, row: usize, pressed: bool) -> Vec<u8> {
    let suffix = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{btn};{};{}{suffix}", col + 1, row + 1).into_bytes()
}

// ---------------------------------------------------------------------------
// winit ApplicationHandler
// ---------------------------------------------------------------------------

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("jterm")
            .with_inner_size(LogicalSize::new(960, 640));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let font_config = FontConfig::default();
        let renderer = match pollster::block_on(Renderer::new(window.clone(), &font_config)) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let (cols, rows) = renderer.grid_size(size.width, size.height);
        log::info!("window {}x{} -> grid {cols}x{rows}", size.width, size.height);

        // Create the initial pane (id 0).
        let initial_id: PaneId = 0;
        let layout = LayoutTree::new(initial_id);

        let pane = match spawn_pane(initial_id, cols, rows, &self.proxy, &self.pty_buffers) {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to spawn initial pane: {e}");
                event_loop.exit();
                return;
            }
        };

        let mut panes = HashMap::new();
        panes.insert(initial_id, pane);

        let keybindings = KeybindingConfig::load();

        self.state = Some(AppState {
            window,
            renderer,
            layout,
            panes,
            keybindings,
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
        });

        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                state.renderer.resize(size.width, size.height);
                resize_all_panes(state);
                state.window.request_redraw();
            }

            WindowEvent::Focused(focused) => {
                // Send focus in/out events to the focused pane if it has focus_events mode.
                let focused_id = state.layout.focused();
                if let Some(pane) = state.panes.get(&focused_id) {
                    if pane.terminal.modes.focus_events {
                        let seq = if focused { b"\x1b[I" } else { b"\x1b[O" };
                        let _ = pane.pty.write(seq);
                    }
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let focused_id = state.layout.focused();

                // Cmd+Q — always quit.
                if state.modifiers.super_key() {
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "q" {
                            event_loop.exit();
                            return;
                        }
                    }
                }

                // Try keybinding lookup.
                if let Some(binding_str) = key_to_binding_string(&event, state.modifiers) {
                    // Determine layer: check if focused pane is in alternate_screen.
                    let is_alt_screen = state
                        .panes
                        .get(&focused_id)
                        .map(|p| p.terminal.modes.alternate_screen)
                        .unwrap_or(false);

                    let action = if is_alt_screen {
                        state
                            .keybindings
                            .lookup_alternate_screen(&binding_str)
                            .or_else(|| state.keybindings.lookup_normal(&binding_str))
                    } else {
                        state.keybindings.lookup_normal(&binding_str)
                    };

                    if let Some(action) = action.cloned() {
                        match action {
                            Action::SplitRight => {
                                let (new_layout, new_id) =
                                    state.layout.split(focused_id, SplitDirection::Horizontal);
                                state.layout = new_layout;
                                // Compute size for new pane from layout.
                                let size = state.window.inner_size();
                                let phys_w = size.width as f32;
                                let phys_h = size.height as f32;
                                let pane_rects = state.layout.panes(phys_w, phys_h);
                                let new_rect = pane_rects
                                    .iter()
                                    .find(|(id, _)| *id == new_id)
                                    .map(|(_, r)| *r);
                                if let Some(rect) = new_rect {
                                    let (cols, rows) =
                                        state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                                    match spawn_pane(
                                        new_id,
                                        cols.max(1),
                                        rows.max(1),
                                        &self.proxy,
                                        &self.pty_buffers,
                                    ) {
                                        Ok(pane) => {
                                            state.panes.insert(new_id, pane);
                                        }
                                        Err(e) => {
                                            log::error!("failed to spawn pane: {e}");
                                        }
                                    }
                                }
                                // Resize existing panes to fit new layout.
                                resize_all_panes(state);
                                state.window.request_redraw();
                                return;
                            }
                            Action::SplitDown => {
                                let (new_layout, new_id) =
                                    state.layout.split(focused_id, SplitDirection::Vertical);
                                state.layout = new_layout;
                                let size = state.window.inner_size();
                                let phys_w = size.width as f32;
                                let phys_h = size.height as f32;
                                let pane_rects = state.layout.panes(phys_w, phys_h);
                                let new_rect = pane_rects
                                    .iter()
                                    .find(|(id, _)| *id == new_id)
                                    .map(|(_, r)| *r);
                                if let Some(rect) = new_rect {
                                    let (cols, rows) =
                                        state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                                    match spawn_pane(
                                        new_id,
                                        cols.max(1),
                                        rows.max(1),
                                        &self.proxy,
                                        &self.pty_buffers,
                                    ) {
                                        Ok(pane) => {
                                            state.panes.insert(new_id, pane);
                                        }
                                        Err(e) => {
                                            log::error!("failed to spawn pane: {e}");
                                        }
                                    }
                                }
                                resize_all_panes(state);
                                state.window.request_redraw();
                                return;
                            }
                            Action::ZoomPane => {
                                state.layout = state.layout.toggle_zoom();
                                resize_all_panes(state);
                                state.window.request_redraw();
                                return;
                            }
                            Action::NextPane => {
                                state.layout = state.layout.navigate(Direction::Next);
                                state.window.request_redraw();
                                return;
                            }
                            Action::PrevPane => {
                                state.layout = state.layout.navigate(Direction::Prev);
                                state.window.request_redraw();
                                return;
                            }
                            Action::CloseTab => {
                                close_focused_pane(state, &self.pty_buffers, event_loop);
                                return;
                            }
                            Action::Copy => {
                                if let Some(pane) = state.panes.get_mut(&focused_id) {
                                    if let Some(ref sel) = pane.selection {
                                        let text = sel.text(pane.terminal.grid());
                                        if !text.is_empty() {
                                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                                let _ = cb.set_text(&text);
                                            }
                                            pane.selection = None;
                                            state.window.request_redraw();
                                            return;
                                        }
                                    }
                                }
                                // No selection — send Ctrl+C to the focused pane.
                                if let Some(pane) = state.panes.get(&focused_id) {
                                    let _ = pane.pty.write(&[0x03]);
                                }
                                return;
                            }
                            Action::Paste => {
                                if let Some(pane) = state.panes.get(&focused_id) {
                                    if let Ok(mut cb) = arboard::Clipboard::new() {
                                        if let Ok(text) = cb.get_text() {
                                            if pane.terminal.modes.bracketed_paste {
                                                let _ = pane.pty.write(b"\x1b[200~");
                                                let _ = pane.pty.write(text.as_bytes());
                                                let _ = pane.pty.write(b"\x1b[201~");
                                            } else {
                                                let _ = pane.pty.write(text.as_bytes());
                                            }
                                        }
                                    }
                                }
                                return;
                            }
                            Action::Passthrough => {
                                // Force key through to PTY, skip binding.
                            }
                            Action::None => {
                                return;
                            }
                            Action::Workspace(_)
                            | Action::NewTab
                            | Action::CommandPalette
                            | Action::AllowFlowPanel
                            | Action::UnreadJump
                            | Action::FontIncrease
                            | Action::FontDecrease
                            | Action::Search
                            | Action::OpenSettings
                            | Action::Command(_) => {
                                // TODO: Not yet implemented.
                                log::debug!("unhandled action: {:?}", action);
                                return;
                            }
                        }
                    }
                }

                // Clear selection on any keypress.
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    if pane.selection.is_some() {
                        pane.selection = None;
                        state.window.request_redraw();
                    }
                }

                // Forward key to PTY.
                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    if let Some(pane) = state.panes.get(&focused_id) {
                        let _ = pane.pty.write(&bytes);
                    }
                }
            }

            // IME text input.
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                if !text.is_empty() {
                    let focused_id = state.layout.focused();
                    if let Some(pane) = state.panes.get(&focused_id) {
                        let _ = pane.pty.write(text.as_bytes());
                    }
                }
            }

            // --- Mouse events ---

            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_pos = (position.x, position.y);

                let focused_id = state.layout.focused();
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    // Handle mouse motion reporting for the terminal.
                    if pane.terminal.modes.mouse_mode == MouseMode::AnyMotion
                        || (pane.terminal.modes.mouse_mode == MouseMode::ButtonMotion
                            && pane
                                .selection
                                .as_ref()
                                .map_or(false, |s| s.active))
                    {
                        let size = state.window.inner_size();
                        let phys_w = size.width as f32;
                        let phys_h = size.height as f32;
                        let pane_rects = state.layout.panes(phys_w, phys_h);
                        if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let cell_size = state.renderer.cell_size();
                            let local_x = (position.x as f32 - rect.x).max(0.0);
                            let local_y = (position.y as f32 - rect.y).max(0.0);
                            let col = (local_x / cell_size.width) as usize;
                            let row = (local_y / cell_size.height) as usize;
                            // Motion event: button 32 + 0 = 32.
                            let seq = encode_mouse_sgr(32, col, row, true);
                            let _ = pane.pty.write(&seq);
                        }
                    } else if let Some(ref mut sel) = pane.selection {
                        // Non-mouse-mode selection dragging.
                        if sel.active {
                            let size = state.window.inner_size();
                            let phys_w = size.width as f32;
                            let phys_h = size.height as f32;
                            let pane_rects = state.layout.panes(phys_w, phys_h);
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let cell_size = state.renderer.cell_size();
                                let local_x = (position.x as f32 - rect.x).max(0.0);
                                let local_y = (position.y as f32 - rect.y).max(0.0);
                                sel.end = GridPos {
                                    col: (local_x / cell_size.width) as usize,
                                    row: (local_y / cell_size.height) as usize,
                                };
                                state.window.request_redraw();
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                let focused_id = state.layout.focused();

                // Check if click is in a different pane — if so, switch focus.
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let size = state.window.inner_size();
                    let phys_w = size.width as f32;
                    let phys_h = size.height as f32;
                    let pane_rects = state.layout.panes(phys_w, phys_h);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    for (pid, rect) in &pane_rects {
                        if *pid != focused_id
                            && cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            state.layout = state.layout.focus(*pid);
                            state.window.request_redraw();
                            break;
                        }
                    }
                }

                let focused_id = state.layout.focused();

                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward mouse event to PTY.
                        let btn_code = match button {
                            MouseButton::Left => 0u8,
                            MouseButton::Middle => 1,
                            MouseButton::Right => 2,
                            _ => return,
                        };
                        let size = state.window.inner_size();
                        let phys_w = size.width as f32;
                        let phys_h = size.height as f32;
                        let pane_rects = state.layout.panes(phys_w, phys_h);
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let cell_size = state.renderer.cell_size();
                            let local_x = (state.cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (state.cursor_pos.1 as f32 - rect.y).max(0.0);
                            let col = (local_x / cell_size.width) as usize;
                            let row = (local_y / cell_size.height) as usize;
                            let pressed = btn_state == ElementState::Pressed;
                            let seq = encode_mouse_sgr(btn_code, col, row, pressed);
                            let _ = pane.pty.write(&seq);
                        }
                    } else {
                        // Selection mode.
                        let size = state.window.inner_size();
                        let phys_w = size.width as f32;
                        let phys_h = size.height as f32;
                        let pane_rects = state.layout.panes(phys_w, phys_h);
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let cell_size = state.renderer.cell_size();
                            let local_x = (state.cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (state.cursor_pos.1 as f32 - rect.y).max(0.0);
                            let pos = GridPos {
                                col: (local_x / cell_size.width) as usize,
                                row: (local_y / cell_size.height) as usize,
                            };

                            if button == MouseButton::Left {
                                match btn_state {
                                    ElementState::Pressed => {
                                        pane.selection = Some(Selection {
                                            start: pos,
                                            end: pos,
                                            active: true,
                                        });
                                        state.window.request_redraw();
                                    }
                                    ElementState::Released => {
                                        if let Some(ref mut sel) = pane.selection {
                                            sel.active = false;
                                            sel.end = pos;
                                            if sel.start == sel.end {
                                                pane.selection = None;
                                            }
                                        }
                                        state.window.request_redraw();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let focused_id = state.layout.focused();
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    let cell_h = state.renderer.cell_size().height as f64;
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => {
                            (pos.y / cell_h).round() as i32
                        }
                    };

                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward scroll as mouse events.
                        // Scroll up = button 64, scroll down = button 65.
                        if lines != 0 {
                            let size = state.window.inner_size();
                            let phys_w = size.width as f32;
                            let phys_h = size.height as f32;
                            let pane_rects = state.layout.panes(phys_w, phys_h);
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let cell_size = state.renderer.cell_size();
                                let local_x =
                                    (state.cursor_pos.0 as f32 - rect.x).max(0.0);
                                let local_y =
                                    (state.cursor_pos.1 as f32 - rect.y).max(0.0);
                                let col = (local_x / cell_size.width) as usize;
                                let row = (local_y / cell_size.height) as usize;
                                let count = lines.unsigned_abs();
                                let btn = if lines > 0 { 64u8 } else { 65u8 };
                                for _ in 0..count {
                                    let seq = encode_mouse_sgr(btn, col, row, true);
                                    let _ = pane.pty.write(&seq);
                                }
                            }
                        }
                    } else if lines != 0 {
                        let current = pane.terminal.scroll_offset() as i32;
                        let new_offset = (current + lines).max(0) as usize;
                        pane.terminal.set_scroll_offset(new_offset);
                        state.window.request_redraw();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                if let Err(e) = render_frame(state) {
                    log::error!("render error: {e}");
                }
            }

            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput(pane_id) => {
                let Some(state) = &mut self.state else {
                    return;
                };
                let mut lock = self.pty_buffers.lock().unwrap();
                let mut total = 0usize;
                if let Some(q) = lock.get_mut(&pane_id) {
                    if let Some(pane) = state.panes.get_mut(&pane_id) {
                        while let Some(data) = q.pop_front() {
                            total += data.len();
                            pane.terminal.feed(&mut pane.vt_parser, &data);
                        }
                    }
                }
                drop(lock);

                // Handle OSC 52 clipboard events.
                if let Some(pane) = state.panes.get_mut(&pane_id) {
                    if let Some(ref clipboard_event) = pane.terminal.clipboard_event.take() {
                        match clipboard_event {
                            ClipboardEvent::Set { data, .. } => {
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    let _ = cb.set_text(data);
                                }
                            }
                            ClipboardEvent::Query { selection } => {
                                // Respond with the current clipboard content encoded as base64.
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    if let Ok(text) = cb.get_text() {
                                        use base64::Engine as _;
                                        let b64 = base64::engine::general_purpose::STANDARD
                                            .encode(text.as_bytes());
                                        let response =
                                            format!("\x1b]52;{selection};{b64}\x07");
                                        let _ = pane.pty.write(response.as_bytes());
                                    }
                                }
                            }
                        }
                    }
                }

                if total > 0 {
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited(pane_id) => {
                log::info!("pane {pane_id}: shell exited");
                let Some(state) = &mut self.state else {
                    return;
                };

                // Remove the pane and close it in the layout.
                state.panes.remove(&pane_id);
                self.pty_buffers.lock().unwrap().remove(&pane_id);

                match state.layout.close(pane_id) {
                    Some(new_layout) => {
                        state.layout = new_layout;
                        resize_all_panes(state);
                        state.window.request_redraw();
                    }
                    None => {
                        // Last pane exited — quit.
                        event_loop.exit();
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resize all panes' terminals and PTYs to match current layout rects.
fn resize_all_panes(state: &mut AppState) {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let pane_rects = state.layout.panes(phys_w, phys_h);
    let multi = pane_rects.len() > 1;
    for (pid, rect) in &pane_rects {
        if let Some(pane) = state.panes.get_mut(pid) {
            let (cols, rows) = if multi {
                // No padding in multi-pane mode.
                state.renderer.grid_size_raw(rect.w as u32, rect.h as u32)
            } else {
                state.renderer.grid_size_raw(rect.w as u32, rect.h as u32)
            };
            let cols = cols.max(1);
            let rows = rows.max(1);
            pane.terminal.resize(cols as usize, rows as usize);
            let _ = pane.pty.resize(PtySize { cols, rows });
        }
    }
}

/// Close the focused pane. If it is the last pane, exit the app.
fn close_focused_pane(
    state: &mut AppState,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) {
    let focused_id = state.layout.focused();
    // Drop the pane (this sends SIGHUP to the PTY child).
    state.panes.remove(&focused_id);
    buffers.lock().unwrap().remove(&focused_id);

    match state.layout.close(focused_id) {
        Some(new_layout) => {
            state.layout = new_layout;
            resize_all_panes(state);
            state.window.request_redraw();
        }
        None => {
            event_loop.exit();
        }
    }
}

/// Render all panes.
fn render_frame(state: &mut AppState) -> Result<(), jterm_render::RenderError> {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let pane_rects = state.layout.panes(phys_w, phys_h);
    let focused_id = state.layout.focused();

    // Single pane: use the simple full-surface render (includes padding + clear).
    if pane_rects.len() == 1 {
        if let Some((pid, _)) = pane_rects.first() {
            if let Some(pane) = state.panes.get(pid) {
                let sel_bounds = sel_bounds_for(pane);
                return state.renderer.render(&pane.terminal, sel_bounds);
            }
        }
        return Ok(());
    }

    // Multi-pane: get surface, clear, render each pane with its own submit.
    let output = state.renderer.get_surface_texture()?;
    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // Clear entire surface.
    state.renderer.clear_surface(&view);

    // Render each pane — each call does its own encoder + submit.
    for (pid, rect) in &pane_rects {
        if let Some(pane) = state.panes.get(pid) {
            let sel_bounds = sel_bounds_for(pane);
            let viewport = (
                rect.x as u32,
                rect.y as u32,
                (rect.w as u32).max(1),
                (rect.h as u32).max(1),
            );
            state
                .renderer
                .render_pane(&pane.terminal, sel_bounds, viewport, *pid, &view)?;
        }
    }

    // Draw separators.
    let sep_color = [0.3, 0.3, 0.3, 1.0];
    let sep = 2u32;
    for i in 0..pane_rects.len() {
        for j in (i + 1)..pane_rects.len() {
            let (_, r1) = &pane_rects[i];
            let (_, r2) = &pane_rects[j];

            let r1_right = (r1.x + r1.w) as u32;
            let r2_left = r2.x as u32;
            if r1_right.abs_diff(r2_left) <= 1 {
                let y0 = r1.y.max(r2.y) as u32;
                let y1 = (r1.y + r1.h).min(r2.y + r2.h) as u32;
                if y1 > y0 {
                    state.renderer.submit_separator(
                        &view,
                        r1_right.saturating_sub(sep / 2),
                        y0,
                        sep,
                        y1 - y0,
                        sep_color,
                    );
                }
            }

            let r1_bottom = (r1.y + r1.h) as u32;
            let r2_top = r2.y as u32;
            if r1_bottom.abs_diff(r2_top) <= 1 {
                let x0 = r1.x.max(r2.x) as u32;
                let x1 = (r1.x + r1.w).min(r2.x + r2.w) as u32;
                if x1 > x0 {
                    state.renderer.submit_separator(
                        &view,
                        x0,
                        r1_bottom.saturating_sub(sep / 2),
                        x1 - x0,
                        sep,
                        sep_color,
                    );
                }
            }
        }
    }

    // Focus border on the focused pane.
    let focus_color = [0.2, 0.6, 1.0, 0.8];
    let b = 2u32;
    if let Some((_, r)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
        let (x, y, w, h) = (r.x as u32, r.y as u32, r.w as u32, r.h as u32);
        state.renderer.submit_separator(&view, x, y, w, b, focus_color);
        if h > b {
            state.renderer.submit_separator(&view, x, y + h - b, w, b, focus_color);
        }
        state.renderer.submit_separator(&view, x, y, b, h, focus_color);
        if w > b {
            state.renderer.submit_separator(&view, x + w - b, y, b, h, focus_color);
        }
    }

    output.present();
    Ok(())
}

fn sel_bounds_for(pane: &Pane) -> Option<((usize, usize), (usize, usize))> {
    match &pane.selection {
        Some(s) if s.start != s.end => {
            let (start, end) = s.ordered();
            Some(((start.col, start.row), (end.col, end.row)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);

    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
    }
}
