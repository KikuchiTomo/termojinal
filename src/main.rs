//! jterm — GPU-accelerated terminal emulator.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use jterm_pty::{Pty, PtyConfig, PtySize};
use jterm_render::{FontConfig, Renderer};
use jterm_vt::Terminal;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Debug)]
enum UserEvent {
    PtyOutput,
    PtyExited,
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

    fn contains(&self, col: usize, row: usize) -> bool {
        let (s, e) = self.ordered();
        if row < s.row || row > e.row {
            return false;
        }
        if row == s.row && row == e.row {
            return col >= s.col && col <= e.col;
        }
        if row == s.row {
            return col >= s.col;
        }
        if row == e.row {
            return col <= e.col;
        }
        true
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
                // Trim trailing spaces and add newline.
                let trimmed = result.trim_end().len();
                result.truncate(trimmed);
                result.push('\n');
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

struct AppState {
    window: Arc<Window>,
    renderer: Renderer,
    terminal: Terminal,
    vt_parser: vte::Parser,
    pty: Pty,
    modifiers: ModifiersState,
    selection: Option<Selection>,
    cursor_pos: (f64, f64), // mouse position in physical pixels
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
    pty_buffer: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            state: None,
            proxy,
            pty_buffer: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
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
        log::info!("window {}x{} → grid {cols}x{rows}", size.width, size.height);

        let config = PtyConfig {
            size: PtySize { cols, rows },
            ..PtyConfig::default()
        };
        let pty = match Pty::spawn(&config) {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to spawn PTY: {e}");
                event_loop.exit();
                return;
            }
        };
        log::info!("shell={}, pid={}", config.shell, pty.pid());

        let terminal = Terminal::new(cols as usize, rows as usize);
        let vt_parser = vte::Parser::new();

        let master_fd = pty.master_fd();
        let proxy = self.proxy.clone();
        let buffer = self.pty_buffer.clone();
        std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 65536];
                loop {
                    match nix::unistd::read(master_fd, &mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            buffer.lock().unwrap().push_back(buf[..n].to_vec());
                            if proxy.send_event(UserEvent::PtyOutput).is_err() {
                                break;
                            }
                        }
                        Err(nix::errno::Errno::EIO | nix::errno::Errno::EBADF) => break,
                        Err(e) => {
                            log::error!("PTY read error: {e}");
                            break;
                        }
                    }
                }
                let _ = proxy.send_event(UserEvent::PtyExited);
            })
            .expect("failed to spawn pty-reader thread");

        self.state = Some(AppState {
            window,
            renderer,
            terminal,
            vt_parser,
            pty,
            modifiers: ModifiersState::empty(),
            selection: None,
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
                state.renderer.resize(size.width, size.height);
                let (cols, rows) = state.renderer.grid_size(size.width, size.height);
                state.terminal.resize(cols as usize, rows as usize);
                let _ = state.pty.resize(PtySize { cols, rows });
                state.window.request_redraw();
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                // Cmd+Q — quit.
                if state.modifiers.super_key() {
                    if let Key::Character(ref c) = event.logical_key {
                        match c.as_str() {
                            "q" => {
                                event_loop.exit();
                                return;
                            }
                            "c" => {
                                // Cmd+C: copy selection or send SIGINT.
                                if let Some(ref sel) = state.selection {
                                    let text = sel.text(state.terminal.grid());
                                    if !text.is_empty() {
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            let _ = cb.set_text(&text);
                                        }
                                        state.selection = None;
                                        state.window.request_redraw();
                                        return;
                                    }
                                }
                                // No selection → send Ctrl+C.
                                let _ = state.pty.write(&[0x03]);
                                return;
                            }
                            "v" => {
                                // Cmd+V: paste from clipboard.
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    if let Ok(text) = cb.get_text() {
                                        if state.terminal.modes.bracketed_paste {
                                            let _ = state.pty.write(b"\x1b[200~");
                                            let _ = state.pty.write(text.as_bytes());
                                            let _ = state.pty.write(b"\x1b[201~");
                                        } else {
                                            let _ = state.pty.write(text.as_bytes());
                                        }
                                    }
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                // Clear selection on any keypress.
                if state.selection.is_some() {
                    state.selection = None;
                    state.window.request_redraw();
                }

                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    let _ = state.pty.write(&bytes);
                }
            }

            // IME text input.
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                if !text.is_empty() {
                    let _ = state.pty.write(text.as_bytes());
                }
            }

            // --- Mouse selection ---

            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_pos = (position.x, position.y);
                if let Some(ref mut sel) = state.selection {
                    if sel.active {
                        let cell_size = state.renderer.cell_size();
                        sel.end = GridPos {
                            col: (position.x as f32 / cell_size.width) as usize,
                            row: (position.y as f32 / cell_size.height) as usize,
                        };
                        state.window.request_redraw();
                    }
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                let cell_size = state.renderer.cell_size();
                let pos = GridPos {
                    col: (state.cursor_pos.0 as f32 / cell_size.width) as usize,
                    row: (state.cursor_pos.1 as f32 / cell_size.height) as usize,
                };

                match btn_state {
                    ElementState::Pressed => {
                        state.selection = Some(Selection {
                            start: pos,
                            end: pos,
                            active: true,
                        });
                        state.window.request_redraw();
                    }
                    ElementState::Released => {
                        if let Some(ref mut sel) = state.selection {
                            sel.active = false;
                            sel.end = pos;
                            // If start == end, clear selection (it was just a click).
                            if sel.start == sel.end {
                                state.selection = None;
                            }
                        }
                        state.window.request_redraw();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                // Pass selection info to renderer via cell flags.
                if let Err(e) = render_with_selection(
                    &mut state.renderer,
                    &mut state.terminal,
                    state.selection.as_ref(),
                ) {
                    log::error!("render error: {e}");
                }
            }

            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput => {
                let Some(state) = &mut self.state else {
                    return;
                };
                let mut lock = self.pty_buffer.lock().unwrap();
                let mut total = 0usize;
                while let Some(data) = lock.pop_front() {
                    total += data.len();
                    state.terminal.feed(&mut state.vt_parser, &data);
                }
                drop(lock);
                if total > 0 {
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited => {
                log::info!("shell exited");
                event_loop.exit();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Render helper — applies selection highlight by temporarily swapping colors
// ---------------------------------------------------------------------------

fn render_with_selection(
    renderer: &mut Renderer,
    terminal: &mut Terminal,
    selection: Option<&Selection>,
) -> Result<(), jterm_render::RenderError> {
    // For now, selection highlighting is done via the REVERSE attribute.
    // We temporarily set REVERSE on selected cells, render, then restore.
    let sel = match selection {
        Some(s) if s.start != s.end => s,
        _ => return renderer.render(terminal),
    };

    // Collect cells to flip.
    let grid = terminal.grid();
    let rows = grid.rows();
    let cols = grid.cols();
    let mut flipped: Vec<(usize, usize, jterm_vt::Attrs)> = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            if sel.contains(col, row) {
                let original_attrs = grid.cell(col, row).attrs;
                flipped.push((col, row, original_attrs));
            }
        }
    }

    // Set REVERSE on selected cells.
    // We need mutable access to the grid through the terminal.
    // Since Terminal doesn't expose grid_mut publicly, we'll use a workaround:
    // the renderer can handle selection via a separate mechanism.
    // For now, just render normally — selection highlight is a TODO.
    renderer.render(terminal)
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
    // Only return for keys we explicitly handle; fall through for others
    // so they can be handled by the text/character fallback below.
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
            _ => {} // Fall through to text/character check.
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
