//! jterm — GPU-accelerated terminal emulator.
//!
//! Opens a winit window, spawns a PTY with the user's shell, and renders
//! the terminal grid using the wgpu-based renderer from jterm-render.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use jterm_pty::{Pty, PtyConfig, PtySize};
use jterm_render::{FontConfig, Renderer};
use jterm_vt::Terminal;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

// ---------------------------------------------------------------------------
// Custom events sent from background threads to the winit event loop
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum UserEvent {
    /// New data available from the PTY.
    PtyOutput,
    /// The PTY process exited.
    PtyExited,
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

        // Create window.
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

        // Initialize GPU renderer.
        let font_config = FontConfig::default();
        let renderer = match pollster::block_on(Renderer::new(window.clone(), &font_config)) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        // Derive grid size from window pixels.
        let size = window.inner_size();
        let (cols, rows) = renderer.grid_size(size.width, size.height);
        log::info!("window {}x{} → grid {cols}x{rows}", size.width, size.height);

        // Spawn PTY.
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

        // Terminal state machine.
        let terminal = Terminal::new(cols as usize, rows as usize);
        let vt_parser = vte::Parser::new();

        // Background thread: read PTY output → buffer → wake event loop.
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
                            let _ = proxy.send_event(UserEvent::PtyOutput);
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
        });

        // Trigger the first render.
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
                        if c.as_str() == "q" {
                            event_loop.exit();
                            return;
                        }
                    }
                }

                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    let _ = state.pty.write(&bytes);
                }
            }

            WindowEvent::RedrawRequested => {
                if let Err(e) = state.renderer.render(&state.terminal) {
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
                // Drain buffered PTY data through the VT parser.
                let mut lock = self.pty_buffer.lock().unwrap();
                while let Some(data) = lock.pop_front() {
                    state.terminal.feed(&mut state.vt_parser, &data);
                }
                drop(lock);
                state.window.request_redraw();
            }
            UserEvent::PtyExited => {
                log::info!("shell exited");
                event_loop.exit();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard → PTY byte translation
// ---------------------------------------------------------------------------

fn key_to_bytes(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> Option<Vec<u8>> {
    // Ctrl+key → control codes (0x01–0x1F).
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
        return match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Backspace => Some(vec![0x7F]),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Escape => Some(vec![0x1B]),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::Insert => Some(b"\x1b[2~".to_vec()),
            NamedKey::F1 => Some(b"\x1bOP".to_vec()),
            NamedKey::F2 => Some(b"\x1bOQ".to_vec()),
            NamedKey::F3 => Some(b"\x1bOR".to_vec()),
            NamedKey::F4 => Some(b"\x1bOS".to_vec()),
            NamedKey::F5 => Some(b"\x1b[15~".to_vec()),
            NamedKey::F6 => Some(b"\x1b[17~".to_vec()),
            NamedKey::F7 => Some(b"\x1b[18~".to_vec()),
            NamedKey::F8 => Some(b"\x1b[19~".to_vec()),
            NamedKey::F9 => Some(b"\x1b[20~".to_vec()),
            NamedKey::F10 => Some(b"\x1b[21~".to_vec()),
            NamedKey::F11 => Some(b"\x1b[23~".to_vec()),
            NamedKey::F12 => Some(b"\x1b[24~".to_vec()),
            _ => None,
        };
    }

    // Regular text input.
    if let Some(ref text) = event.text {
        if !text.is_empty() {
            return Some(text.as_bytes().to_vec());
        }
    }

    // Fallback: character key without text.
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

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");

    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);

    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
    }
}
