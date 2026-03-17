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
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId};

// ---------------------------------------------------------------------------
// Command Palette
// ---------------------------------------------------------------------------

struct PaletteCommand {
    name: String,
    description: String,
    action: Action,
}

enum PaletteResult {
    /// Key was handled by palette.
    Consumed,
    /// User selected a command — execute the action.
    Execute(Action),
    /// User pressed Escape — dismiss palette.
    Dismiss,
    /// Key not handled by palette.
    Pass,
}

struct CommandPalette {
    visible: bool,
    input: String,
    commands: Vec<PaletteCommand>,
    filtered: Vec<usize>, // Indices into commands
    selected: usize,      // Index into filtered
}

impl CommandPalette {
    fn new() -> Self {
        let commands = vec![
            PaletteCommand {
                name: "Split Right".to_string(),
                description: "Split pane horizontally".to_string(),
                action: Action::SplitRight,
            },
            PaletteCommand {
                name: "Split Down".to_string(),
                description: "Split pane vertically".to_string(),
                action: Action::SplitDown,
            },
            PaletteCommand {
                name: "Close Pane".to_string(),
                description: "Close the focused pane".to_string(),
                action: Action::CloseTab,
            },
            PaletteCommand {
                name: "New Tab".to_string(),
                description: "Open a new tab".to_string(),
                action: Action::NewTab,
            },
            PaletteCommand {
                name: "Zoom Pane".to_string(),
                description: "Toggle pane zoom".to_string(),
                action: Action::ZoomPane,
            },
            PaletteCommand {
                name: "Next Pane".to_string(),
                description: "Focus next pane".to_string(),
                action: Action::NextPane,
            },
            PaletteCommand {
                name: "Previous Pane".to_string(),
                description: "Focus previous pane".to_string(),
                action: Action::PrevPane,
            },
            PaletteCommand {
                name: "New Workspace".to_string(),
                description: "Create a new workspace".to_string(),
                action: Action::NewWorkspace,
            },
            PaletteCommand {
                name: "Next Tab".to_string(),
                description: "Switch to next tab".to_string(),
                action: Action::NextTab,
            },
            PaletteCommand {
                name: "Previous Tab".to_string(),
                description: "Switch to previous tab".to_string(),
                action: Action::PrevTab,
            },
            PaletteCommand {
                name: "Toggle Sidebar".to_string(),
                description: "Show/hide sidebar".to_string(),
                action: Action::None,
            },
            PaletteCommand {
                name: "Copy".to_string(),
                description: "Copy selection to clipboard".to_string(),
                action: Action::Copy,
            },
            PaletteCommand {
                name: "Paste".to_string(),
                description: "Paste from clipboard".to_string(),
                action: Action::Paste,
            },
            PaletteCommand {
                name: "Search".to_string(),
                description: "Find in terminal".to_string(),
                action: Action::Search,
            },
        ];
        let filtered: Vec<usize> = (0..commands.len()).collect();
        Self {
            visible: false,
            input: String::new(),
            commands,
            filtered,
            selected: 0,
        }
    }

    fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            // Reset state when opening.
            self.input.clear();
            self.update_filter();
        }
    }

    fn update_filter(&mut self) {
        let query = self.input.to_lowercase();
        self.filtered = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, cmd)| cmd.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
    }

    fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn execute(&mut self) -> Option<Action> {
        if let Some(&idx) = self.filtered.get(self.selected) {
            Some(self.commands[idx].action.clone())
        } else {
            None
        }
    }

    fn handle_key(&mut self, event: &winit::event::KeyEvent) -> PaletteResult {
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => PaletteResult::Dismiss,
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = self.execute() {
                    PaletteResult::Execute(action)
                } else {
                    PaletteResult::Consumed
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.select_prev();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next();
                PaletteResult::Consumed
            }
            Key::Named(NamedKey::Backspace) => {
                self.input.pop();
                self.update_filter();
                PaletteResult::Consumed
            }
            _ => {
                if let Some(ref text) = event.text {
                    if !text.is_empty() && !text.contains('\r') {
                        self.input.push_str(text);
                        self.update_filter();
                        return PaletteResult::Consumed;
                    }
                }
                PaletteResult::Pass
            }
        }
    }
}

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
// Drag-resize state for split pane separators
// ---------------------------------------------------------------------------

struct DragResize {
    /// Which direction the separator runs (Horizontal = vertical separator, etc.)
    direction: SplitDirection,
    /// The pane on the "first" side of the separator (used for resize calculation)
    pane_id: PaneId,
    /// Last mouse position along the drag axis
    last_pos: f64,
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
    preedit: Option<String>,
}

// ---------------------------------------------------------------------------
// Tab — a single tab within a workspace, containing a layout tree + panes
// ---------------------------------------------------------------------------

struct Tab {
    layout: LayoutTree,
    panes: HashMap<PaneId, Pane>,
    name: String,
}

// ---------------------------------------------------------------------------
// Workspace — contains multiple tabs
// ---------------------------------------------------------------------------

struct Workspace {
    tabs: Vec<Tab>,
    active_tab: usize,
    name: String,
}

// ---------------------------------------------------------------------------
// Sidebar / tab bar constants
// ---------------------------------------------------------------------------

const TAB_BAR_HEIGHT: f32 = 24.0;
const DEFAULT_SIDEBAR_WIDTH: f32 = 200.0;

// ---------------------------------------------------------------------------
// AppState — the full multi-pane application state
// ---------------------------------------------------------------------------

struct AppState {
    window: Arc<Window>,
    renderer: Renderer,
    workspaces: Vec<Workspace>,
    active_workspace: usize,
    keybindings: KeybindingConfig,
    modifiers: ModifiersState,
    cursor_pos: (f64, f64),
    drag_resize: Option<DragResize>,
    next_pane_id: PaneId,
    sidebar_visible: bool,
    sidebar_width: f32,
    command_palette: CommandPalette,
}

/// Helper to access the active workspace immutably.
fn active_ws(state: &AppState) -> &Workspace {
    &state.workspaces[state.active_workspace]
}

/// Helper to access the active workspace mutably.
fn active_ws_mut(state: &mut AppState) -> &mut Workspace {
    &mut state.workspaces[state.active_workspace]
}

/// Helper to access the active tab of the active workspace immutably.
fn active_tab(state: &AppState) -> &Tab {
    let ws = active_ws(state);
    &ws.tabs[ws.active_tab]
}

/// Helper to access the active tab of the active workspace mutably.
fn active_tab_mut(state: &mut AppState) -> &mut Tab {
    let ws = active_ws_mut(state);
    let idx = ws.active_tab;
    &mut ws.tabs[idx]
}

/// Compute the content area that excludes the tab bar and sidebar.
/// Returns (content_x, content_y, content_w, content_h) in physical pixels.
fn content_area(state: &AppState, phys_w: f32, phys_h: f32) -> (f32, f32, f32, f32) {
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let tab_bar_h = if active_ws(state).tabs.len() > 1 { TAB_BAR_HEIGHT } else { 0.0 };
    let content_x = sidebar_w;
    let content_y = tab_bar_h;
    let content_w = (phys_w - sidebar_w).max(1.0);
    let content_h = (phys_h - tab_bar_h).max(1.0);
    (content_x, content_y, content_w, content_h)
}

/// Get pane rects for the active tab of the active workspace, offset by tab bar + sidebar.
fn active_pane_rects(state: &AppState) -> Vec<(PaneId, jterm_layout::Rect)> {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let (cx, cy, cw, ch) = content_area(state, phys_w, phys_h);
    let tab = active_tab(state);
    let mut rects = tab.layout.panes(cw, ch);
    for (_, rect) in &mut rects {
        rect.x += cx;
        rect.y += cy;
    }
    rects
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
        preedit: None,
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
        let phys_w = size.width as f32;
        let phys_h = size.height as f32;
        // No tab bar for the initial single-tab workspace.
        let (cols, rows) = renderer.grid_size_raw(phys_w as u32, phys_h as u32);
        log::info!("window {}x{} -> grid {cols}x{rows}", size.width, size.height);

        // Create the initial pane (id 0) in the first workspace, first tab.
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

        let initial_tab = Tab {
            layout,
            panes,
            name: "Tab 1".to_string(),
        };

        let initial_workspace = Workspace {
            tabs: vec![initial_tab],
            active_tab: 0,
            name: "Workspace 1".to_string(),
        };

        let keybindings = KeybindingConfig::load();

        self.state = Some(AppState {
            window,
            renderer,
            workspaces: vec![initial_workspace],
            active_workspace: 0,
            keybindings,
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
            drag_resize: None,
            next_pane_id: 1, // 0 is already used
            sidebar_visible: true,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            command_palette: CommandPalette::new(),
        });

        // Enable IME after window is fully created and request initial redraw.
        let state = self.state.as_ref().unwrap();
        state.window.set_ime_allowed(true);
        // Set initial IME cursor area so macOS will start sending IME events.
        state.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(0.0, 0.0),
            winit::dpi::PhysicalSize::new(
                state.renderer.cell_size().width as f64,
                state.renderer.cell_size().height as f64,
            ),
        );
        state.window.request_redraw();
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
                if focused {
                    // Re-enable IME when window gains focus.
                    state.window.set_ime_allowed(true);
                }
                // Send focus in/out events to the focused pane if it has focus_events mode.
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab(state).panes.get(&focused_id) {
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

                // Suppress raw key events during IME composition.
                let focused_id = active_tab(state).layout.focused();
                if active_tab(state).panes.get(&focused_id).map_or(false, |p| p.preedit.is_some()) {
                    return;
                }

                // Command palette intercepts ALL keyboard input when visible.
                if state.command_palette.visible {
                    match state.command_palette.handle_key(&event) {
                        PaletteResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Execute(action) => {
                            state.command_palette.visible = false;
                            // Handle the special "Toggle Sidebar" palette command.
                            if matches!(action, Action::None) {
                                // Check if this was the "Toggle Sidebar" command by
                                // looking at the selected index — Action::None from
                                // palette means toggle sidebar.
                                state.sidebar_visible = !state.sidebar_visible;
                                resize_all_panes(state);
                                state.window.request_redraw();
                                return;
                            }
                            dispatch_action(
                                state,
                                &action,
                                &self.proxy,
                                &self.pty_buffers,
                                event_loop,
                            );
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Dismiss => {
                            state.command_palette.visible = false;
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Pass => {}
                    }
                }

                // Cmd+Q — always quit.
                if state.modifiers.super_key() {
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "q" {
                            event_loop.exit();
                            return;
                        }
                    }
                }

                // Handle Cmd+Shift+{ and Cmd+Shift+} for prev/next tab within workspace.
                if state.modifiers.super_key() && state.modifiers.shift_key() {
                    if let Key::Character(ref c) = event.logical_key {
                        match c.as_str() {
                            "{" => {
                                let ws = active_ws_mut(state);
                                if ws.active_tab > 0 {
                                    ws.active_tab -= 1;
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                }
                                return;
                            }
                            "}" => {
                                let ws = active_ws_mut(state);
                                if ws.active_tab + 1 < ws.tabs.len() {
                                    ws.active_tab += 1;
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                // Handle Cmd+B for sidebar toggle.
                if state.modifiers.super_key() && !state.modifiers.shift_key() {
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "b" {
                            state.sidebar_visible = !state.sidebar_visible;
                            resize_all_panes(state);
                            state.window.request_redraw();
                            return;
                        }
                    }
                }

                // Try keybinding lookup.
                if let Some(binding_str) = key_to_binding_string(&event, state.modifiers) {
                    // Determine layer: check if focused pane is in alternate_screen.
                    let is_alt_screen = active_tab(state)
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
                        if dispatch_action(
                            state,
                            &action,
                            &self.proxy,
                            &self.pty_buffers,
                            event_loop,
                        ) {
                            return;
                        }
                        // Action::Passthrough falls through to send key to PTY.
                    }
                }

                // Clear selection on any keypress.
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    if pane.selection.is_some() {
                        pane.selection = None;
                        state.window.request_redraw();
                    }
                }

                // Forward key to PTY.
                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                        let _ = pane.pty.write(&bytes);
                    }
                }
            }

            // IME events.
            WindowEvent::Ime(ime) => {
                let focused_id = active_tab(state).layout.focused();
                match ime {
                    winit::event::Ime::Enabled => {
                        // IME session started — nothing to do.
                    }
                    winit::event::Ime::Preedit(text, _cursor) => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            if text.is_empty() {
                                pane.preedit = None;
                            } else {
                                pane.preedit = Some(text);
                            }
                            state.window.request_redraw();
                        }
                    }
                    winit::event::Ime::Commit(text) => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            pane.preedit = None;
                            if !text.is_empty() {
                                let _ = pane.pty.write(text.as_bytes());
                            }
                            state.window.request_redraw();
                        }
                    }
                    winit::event::Ime::Disabled => {
                        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                            pane.preedit = None;
                            state.window.request_redraw();
                        }
                    }
                }
            }

            // --- Mouse events ---

            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_pos = (position.x, position.y);

                // --- Drag-resize active: update layout ---
                if let Some(ref mut drag) = state.drag_resize {
                    let current_pos = match drag.direction {
                        SplitDirection::Horizontal => position.x,
                        SplitDirection::Vertical => position.y,
                    };
                    let pixel_delta = current_pos - drag.last_pos;
                    drag.last_pos = current_pos;

                    // Scale pixel delta to the 1000x1000 coordinate space used
                    // by LayoutTree::resize().
                    let size = state.window.inner_size();
                    let phys_w = size.width as f32;
                    let phys_h = size.height as f32;
                    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
                    let ws = &state.workspaces[state.active_workspace];
                    let tab_bar_h = if ws.tabs.len() > 1 { TAB_BAR_HEIGHT } else { 0.0 };
                    let cw = (phys_w - sidebar_w).max(1.0);
                    let ch = (phys_h - tab_bar_h).max(1.0);
                    let actual_dim = match drag.direction {
                        SplitDirection::Horizontal => cw as f64,
                        SplitDirection::Vertical => ch as f64,
                    };
                    if actual_dim > 0.0 {
                        let scaled_delta = (pixel_delta * 1000.0 / actual_dim) as f32;
                        let pane_id = drag.pane_id;
                        let direction = drag.direction;
                        let tab = active_tab_mut(state);
                        tab.layout = tab.layout.resize(pane_id, direction, scaled_delta);
                        resize_all_panes(state);
                        state.window.request_redraw();
                    }
                } else {
                    // --- Cursor icon management: show resize cursor near separators ---
                    let pane_rects = active_pane_rects(state);
                    let mx = position.x as f32;
                    let my = position.y as f32;

                    if let Some((dir, _)) = find_separator(&pane_rects, mx, my, 4.0) {
                        let icon = match dir {
                            SplitDirection::Horizontal => CursorIcon::ColResize,
                            SplitDirection::Vertical => CursorIcon::RowResize,
                        };
                        state.window.set_cursor(icon);
                    } else {
                        state.window.set_cursor(CursorIcon::Text);
                    }

                    // --- Original mouse handling (motion reporting / selection) ---
                    let focused_id = active_tab(state).layout.focused();
                    let cell_size = state.renderer.cell_size();
                    let cursor_x = position.x as f32;
                    let cursor_y = position.y as f32;
                    let tab = active_tab_mut(state);
                    if let Some(pane) = tab.panes.get_mut(&focused_id) {
                        // Handle mouse motion reporting for the terminal.
                        if pane.terminal.modes.mouse_mode == MouseMode::AnyMotion
                            || (pane.terminal.modes.mouse_mode == MouseMode::ButtonMotion
                                && pane
                                    .selection
                                    .as_ref()
                                    .map_or(false, |s| s.active))
                        {
                            if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let local_x = (cursor_x - rect.x).max(0.0);
                                let local_y = (cursor_y - rect.y).max(0.0);
                                let col = (local_x / cell_size.width) as usize;
                                let row = (local_y / cell_size.height) as usize;
                                // Motion event: button 32 + 0 = 32.
                                let seq = encode_mouse_sgr(32, col, row, true);
                                let _ = pane.pty.write(&seq);
                            }
                        } else if let Some(ref mut sel) = pane.selection {
                            // Non-mouse-mode selection dragging.
                            if sel.active {
                                if let Some((_, rect)) =
                                    pane_rects.iter().find(|(id, _)| *id == focused_id)
                                {
                                    let local_x = (cursor_x - rect.x).max(0.0);
                                    let local_y = (cursor_y - rect.y).max(0.0);
                                    sel.end = GridPos {
                                        col: (local_x / cell_size.width) as usize,
                                        row: (local_y / cell_size.height) as usize,
                                    };
                                }
                            }
                        }
                    }
                    state.window.request_redraw();
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => 'mouse_input: {
                // --- Handle drag-resize release ---
                if btn_state == ElementState::Released && button == MouseButton::Left {
                    if state.drag_resize.is_some() {
                        state.drag_resize = None;
                        state.window.set_cursor(CursorIcon::Default);
                        // Skip all other release handling when ending a drag.
                        break 'mouse_input;
                    }
                }

                // --- Priority 0: Check if click is in the sidebar area ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.sidebar_visible
                {
                    let cx = state.cursor_pos.0 as f32;
                    if cx < state.sidebar_width {
                        handle_sidebar_click(state);
                        break 'mouse_input;
                    }
                }

                // --- Priority 0.5: Check if click is in the tab bar area ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let tab_bar_h = if active_ws(state).tabs.len() > 1 { TAB_BAR_HEIGHT } else { 0.0 };
                    let cy = state.cursor_pos.1 as f32;
                    if tab_bar_h > 0.0 && cy < tab_bar_h {
                        handle_tab_bar_click(state);
                        break 'mouse_input;
                    }
                }

                let focused_id = active_tab(state).layout.focused();

                // --- Priority 1: Check if clicking on a separator → start drag resize ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;

                    if let Some((direction, pane_id)) = find_separator(&pane_rects, cx, cy, 4.0) {
                        let last_pos = match direction {
                            SplitDirection::Horizontal => state.cursor_pos.0,
                            SplitDirection::Vertical => state.cursor_pos.1,
                        };
                        let icon = match direction {
                            SplitDirection::Horizontal => CursorIcon::ColResize,
                            SplitDirection::Vertical => CursorIcon::RowResize,
                        };
                        state.window.set_cursor(icon);
                        state.drag_resize = Some(DragResize {
                            direction,
                            pane_id,
                            last_pos,
                        });
                        // Don't fall through to focus-change or selection.
                        break 'mouse_input;
                    }
                }

                // --- Priority 2: Check if click is in a different pane → change focus ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    for (pid, rect) in &pane_rects {
                        if *pid != focused_id
                            && cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            let tab = active_tab_mut(state);
                            tab.layout = tab.layout.focus(*pid);
                            state.window.request_redraw();
                            break;
                        }
                    }
                }

                // --- Priority 3: Selection / mouse-mode forwarding ---
                let focused_id = active_tab(state).layout.focused();
                let cell_size = state.renderer.cell_size();
                let cursor_pos = state.cursor_pos;
                let pane_rects = active_pane_rects(state);
                let tab = active_tab_mut(state);

                if let Some(pane) = tab.panes.get_mut(&focused_id) {
                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward mouse event to PTY.
                        let btn_code = match button {
                            MouseButton::Left => 0u8,
                            MouseButton::Middle => 1,
                            MouseButton::Right => 2,
                            _ => return,
                        };
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let local_x = (cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (cursor_pos.1 as f32 - rect.y).max(0.0);
                            let col = (local_x / cell_size.width) as usize;
                            let row = (local_y / cell_size.height) as usize;
                            let pressed = btn_state == ElementState::Pressed;
                            let seq = encode_mouse_sgr(btn_code, col, row, pressed);
                            let _ = pane.pty.write(&seq);
                        }
                    } else {
                        // Selection mode.
                        if let Some((_, rect)) =
                            pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            let local_x = (cursor_pos.0 as f32 - rect.x).max(0.0);
                            let local_y = (cursor_pos.1 as f32 - rect.y).max(0.0);
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
                                    }
                                    ElementState::Released => {
                                        if let Some(ref mut sel) = pane.selection {
                                            sel.active = false;
                                            sel.end = pos;
                                            if sel.start == sel.end {
                                                pane.selection = None;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                state.window.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let focused_id = active_tab(state).layout.focused();
                let cell_size = state.renderer.cell_size();
                let cell_h = cell_size.height as f64;
                let cursor_pos = state.cursor_pos;
                let pane_rects = active_pane_rects(state);
                let tab = active_tab_mut(state);
                if let Some(pane) = tab.panes.get_mut(&focused_id) {
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
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let local_x =
                                    (cursor_pos.0 as f32 - rect.x).max(0.0);
                                let local_y =
                                    (cursor_pos.1 as f32 - rect.y).max(0.0);
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
                // Find the pane across all workspaces and tabs.
                let mut lock = self.pty_buffers.lock().unwrap();
                let mut total = 0usize;
                if let Some(q) = lock.get_mut(&pane_id) {
                    'outer_feed: for ws in &mut state.workspaces {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
                                while let Some(data) = q.pop_front() {
                                    total += data.len();
                                    pane.terminal.feed(&mut pane.vt_parser, &data);
                                }
                                break 'outer_feed;
                            }
                        }
                    }
                }
                drop(lock);

                // Handle OSC 52 clipboard events.
                'outer_clip: for ws in &mut state.workspaces {
                    for tab in &mut ws.tabs {
                        if let Some(pane) = tab.panes.get_mut(&pane_id) {
                            if let Some(ref clipboard_event) = pane.terminal.clipboard_event.take() {
                                match clipboard_event {
                                    ClipboardEvent::Set { data, .. } => {
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            let _ = cb.set_text(data);
                                        }
                                    }
                                    ClipboardEvent::Query { selection } => {
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
                            break 'outer_clip;
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

                // Find which workspace/tab owns this pane and remove it.
                self.pty_buffers.lock().unwrap().remove(&pane_id);

                let mut found = None; // (ws_idx, tab_idx)
                for (wi, ws) in state.workspaces.iter_mut().enumerate() {
                    for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                        if tab.panes.remove(&pane_id).is_some() {
                            found = Some((wi, ti));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }

                let Some((ws_idx, tab_idx)) = found else {
                    return;
                };

                let tab = &state.workspaces[ws_idx].tabs[tab_idx];
                match tab.layout.close(pane_id) {
                    Some(new_layout) => {
                        state.workspaces[ws_idx].tabs[tab_idx].layout = new_layout;
                        resize_all_panes(state);
                        state.window.request_redraw();
                    }
                    None => {
                        // Last pane in the tab exited — close the tab.
                        let ws = &mut state.workspaces[ws_idx];
                        if ws.tabs.len() == 1 {
                            // Last tab in this workspace — close the workspace.
                            if state.workspaces.len() == 1 {
                                // Last workspace — quit.
                                event_loop.exit();
                            } else {
                                state.workspaces.remove(ws_idx);
                                if state.active_workspace >= state.workspaces.len() {
                                    state.active_workspace = state.workspaces.len() - 1;
                                }
                                resize_all_panes(state);
                                state.window.request_redraw();
                            }
                        } else {
                            ws.tabs.remove(tab_idx);
                            if ws.active_tab >= ws.tabs.len() {
                                ws.active_tab = ws.tabs.len() - 1;
                            }
                            resize_all_panes(state);
                            state.window.request_redraw();
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Dispatch an action from keybinding or command palette.
///
/// Returns `true` if the action was handled (caller should `return`),
/// or `false` if the key should fall through to PTY (e.g., `Passthrough`).
fn dispatch_action(
    state: &mut AppState,
    action: &Action,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) -> bool {
    let focused_id = active_tab(state).layout.focused();
    match action {
        Action::SplitRight => {
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) =
                tab.layout.split(focused_id, SplitDirection::Horizontal);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) =
                    state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::SplitDown => {
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) =
                tab.layout.split(focused_id, SplitDirection::Vertical);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) =
                    state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::ZoomPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.toggle_zoom();
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::NextPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Next);
            state.window.request_redraw();
            true
        }
        Action::PrevPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Prev);
            state.window.request_redraw();
            true
        }
        Action::NewTab => {
            // Create a new tab in the current workspace.
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            // When we add a new tab, the workspace will have >1 tabs, so tab bar will appear.
            let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = (phys_h - TAB_BAR_HEIGHT).max(1.0);
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws = active_ws_mut(state);
                    let tab_num = ws.tabs.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: format!("Tab {tab_num}"),
                    };
                    ws.tabs.push(tab);
                    ws.active_tab = ws.tabs.len() - 1;
                    resize_all_panes(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new tab: {e}");
                }
            }
            true
        }
        Action::CloseTab => {
            close_focused_pane(state, buffers, event_loop);
            true
        }
        Action::NewWorkspace => {
            // Create a new workspace with one tab and one pane.
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = phys_h.max(1.0); // Single tab, no tab bar
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(new_id, cols.max(1), rows.max(1), proxy, buffers) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws_num = state.workspaces.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: "Tab 1".to_string(),
                    };
                    let ws = Workspace {
                        tabs: vec![tab],
                        active_tab: 0,
                        name: format!("Workspace {ws_num}"),
                    };
                    state.workspaces.push(ws);
                    state.active_workspace = state.workspaces.len() - 1;
                    resize_all_panes(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new workspace: {e}");
                }
            }
            true
        }
        Action::NextTab => {
            let ws = active_ws_mut(state);
            if ws.active_tab + 1 < ws.tabs.len() {
                ws.active_tab += 1;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::PrevTab => {
            let ws = active_ws_mut(state);
            if ws.active_tab > 0 {
                ws.active_tab -= 1;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::Workspace(n) => {
            let idx = (*n as usize).saturating_sub(1);
            if idx < state.workspaces.len() {
                state.active_workspace = idx;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::CommandPalette => {
            state.command_palette.toggle();
            state.window.request_redraw();
            true
        }
        Action::Copy => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                if let Some(ref sel) = pane.selection {
                    let text = sel.text(pane.terminal.grid());
                    if !text.is_empty() {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            let _ = cb.set_text(&text);
                        }
                        pane.selection = None;
                        state.window.request_redraw();
                        return true;
                    }
                }
            }
            // No selection — send Ctrl+C to the focused pane.
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                let _ = pane.pty.write(&[0x03]);
            }
            true
        }
        Action::Paste => {
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
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
            true
        }
        Action::Passthrough => {
            // Force key through to PTY, skip binding.
            false
        }
        Action::None => true,
        Action::AllowFlowPanel
        | Action::UnreadJump
        | Action::FontIncrease
        | Action::FontDecrease
        | Action::Search
        | Action::OpenSettings
        | Action::Command(_) => {
            log::debug!("unhandled action: {:?}", action);
            true
        }
    }
}

/// Resize all panes' terminals and PTYs to match current layout rects.
fn resize_all_panes(state: &mut AppState) {
    let pane_rects = active_pane_rects(state);
    for (pid, rect) in &pane_rects {
        let (cols, rows) =
            state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
        let cols = cols.max(1);
        let rows = rows.max(1);
        let tab = active_tab_mut(state);
        if let Some(pane) = tab.panes.get_mut(pid) {
            pane.terminal.resize(cols as usize, rows as usize);
            let _ = pane.pty.resize(PtySize { cols, rows });
        }
    }
}

/// Detect if the mouse at (mx, my) is near a separator between panes.
///
/// Returns `Some((direction, pane_id))` where:
/// - `direction` is the split direction (Horizontal = vertical separator line,
///   Vertical = horizontal separator line)
/// - `pane_id` is the pane on the "first" (left/top) side of the separator
///
/// The threshold is how many pixels from the separator boundary to detect.
fn find_separator(
    pane_rects: &[(PaneId, jterm_layout::Rect)],
    mx: f32,
    my: f32,
    threshold: f32,
) -> Option<(SplitDirection, PaneId)> {
    for i in 0..pane_rects.len() {
        for j in (i + 1)..pane_rects.len() {
            let (id1, r1) = &pane_rects[i];
            let (_, r2) = &pane_rects[j];

            // Vertical separator: r1 is to the left of r2 (horizontal split).
            let r1_right = r1.x + r1.w;
            if (r1_right - r2.x).abs() <= 2.0 {
                let y_top = r1.y.max(r2.y);
                let y_bot = (r1.y + r1.h).min(r2.y + r2.h);
                if y_bot > y_top
                    && my >= y_top
                    && my <= y_bot
                    && (mx - r1_right).abs() <= threshold
                {
                    return Some((SplitDirection::Horizontal, *id1));
                }
            }

            // Horizontal separator: r1 is above r2 (vertical split).
            let r1_bottom = r1.y + r1.h;
            if (r1_bottom - r2.y).abs() <= 2.0 {
                let x_left = r1.x.max(r2.x);
                let x_right = (r1.x + r1.w).min(r2.x + r2.w);
                if x_right > x_left
                    && mx >= x_left
                    && mx <= x_right
                    && (my - r1_bottom).abs() <= threshold
                {
                    return Some((SplitDirection::Vertical, *id1));
                }
            }
        }
    }
    None
}

/// Close the focused pane. If it is the last pane in the tab, close the tab.
/// If it is the last tab in the workspace, close the workspace.
/// If it is the last workspace, exit the app.
fn close_focused_pane(
    state: &mut AppState,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) {
    let tab = active_tab_mut(state);
    let focused_id = tab.layout.focused();
    // Drop the pane (this sends SIGHUP to the PTY child).
    tab.panes.remove(&focused_id);
    buffers.lock().unwrap().remove(&focused_id);

    match active_tab(state).layout.close(focused_id) {
        Some(new_layout) => {
            active_tab_mut(state).layout = new_layout;
            resize_all_panes(state);
            state.window.request_redraw();
        }
        None => {
            // Last pane in the current tab — close the tab.
            let ws = active_ws_mut(state);
            if ws.tabs.len() == 1 {
                // Last tab in this workspace — close the workspace.
                if state.workspaces.len() == 1 {
                    // Last workspace — quit.
                    event_loop.exit();
                } else {
                    state.workspaces.remove(state.active_workspace);
                    if state.active_workspace >= state.workspaces.len() {
                        state.active_workspace = state.workspaces.len() - 1;
                    }
                    resize_all_panes(state);
                    state.window.request_redraw();
                }
            } else {
                let tab_idx = ws.active_tab;
                ws.tabs.remove(tab_idx);
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len() - 1;
                }
                resize_all_panes(state);
                state.window.request_redraw();
            }
        }
    }
}

/// Handle a click in the tab bar area. Determine which tab was clicked and switch to it.
fn handle_tab_bar_click(state: &mut AppState) {
    let cx = state.cursor_pos.0 as f32;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let local_cx = cx - sidebar_w;
    if local_cx < 0.0 {
        return;
    }
    let cell_w = state.renderer.cell_size().width;
    let min_tab_width: f32 = 80.0;

    let ws = active_ws(state);
    let mut tab_x: f32 = 0.0;
    for (i, tab) in ws.tabs.iter().enumerate() {
        let text_width = tab.name.len() as f32 * cell_w + 2.0 * cell_w; // padding
        let tab_w = text_width.max(min_tab_width);
        if local_cx >= tab_x && local_cx < tab_x + tab_w {
            let ws = active_ws_mut(state);
            if ws.active_tab != i {
                ws.active_tab = i;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            return;
        }
        tab_x += tab_w;
    }
}

/// Handle a click in the sidebar area. Determine which workspace was clicked.
fn handle_sidebar_click(state: &mut AppState) {
    let cy = state.cursor_pos.1 as f32;
    let cell_h = state.renderer.cell_size().height;
    // Each sidebar entry is one line of text, starting at the top.
    let entry_idx = (cy / cell_h) as usize;
    // Check if clicking on the "+ New" entry at the bottom.
    if entry_idx == state.workspaces.len() {
        // Treat this as a "new workspace" signal, but actual creation
        // is handled by Cmd+N. Just ignore for now.
        return;
    }
    if entry_idx < state.workspaces.len() && state.active_workspace != entry_idx {
        state.active_workspace = entry_idx;
        resize_all_panes(state);
        state.window.request_redraw();
    }
}

/// Render the tab bar (only shown when workspace has >1 tabs).
fn render_tab_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32) {
    let tab_bar_bg = [0.12, 0.12, 0.15, 1.0];
    let active_tab_bg = [0.2, 0.2, 0.25, 1.0];
    let tab_fg = [0.85, 0.85, 0.85, 1.0];
    let cell_w = state.renderer.cell_size().width;
    let min_tab_width: f32 = 80.0;
    let sidebar_w = if state.sidebar_visible { state.sidebar_width } else { 0.0 };
    let bar_x = sidebar_w as u32;
    let bar_w = (phys_w - sidebar_w).max(0.0) as u32;

    // Draw tab bar background.
    state.renderer.submit_separator(
        view,
        bar_x,
        0,
        bar_w,
        TAB_BAR_HEIGHT as u32,
        tab_bar_bg,
    );

    // Draw each tab in the current workspace.
    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let active_tab_idx = ws.active_tab;
    let mut tab_x: f32 = sidebar_w;
    for (i, tab) in ws.tabs.iter().enumerate() {
        let text_width = tab.name.len() as f32 * cell_w + 2.0 * cell_w;
        let tab_w = text_width.max(min_tab_width);
        let is_active = i == active_tab_idx;

        // Draw tab background.
        if is_active {
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                0,
                tab_w as u32,
                TAB_BAR_HEIGHT as u32,
                active_tab_bg,
            );
        }

        // Draw tab text, centered vertically in the tab bar.
        let text_y = (TAB_BAR_HEIGHT - state.renderer.cell_size().height) / 2.0;
        let text_x = tab_x + cell_w; // 1 cell padding on the left
        let bg = if is_active { active_tab_bg } else { tab_bar_bg };
        state.renderer.render_text(view, &tab.name, text_x, text_y.max(0.0), tab_fg, bg);

        tab_x += tab_w;
    }
}

/// Render the sidebar showing workspaces as a vertical list.
fn render_sidebar(state: &mut AppState, view: &wgpu::TextureView, phys_h: f32) {
    let sidebar_bg = [0.10, 0.10, 0.13, 1.0];
    let active_entry_bg = [0.18, 0.18, 0.22, 1.0];
    let sidebar_fg = [0.75, 0.75, 0.78, 1.0];
    let active_fg = [0.95, 0.95, 0.95, 1.0];
    let accent_color = [0.2, 0.6, 1.0, 1.0]; // Blue accent for active dot
    let cell_h = state.renderer.cell_size().height;
    let cell_w = state.renderer.cell_size().width;

    let sidebar_w = state.sidebar_width;

    // Draw sidebar background (full height).
    state.renderer.submit_separator(
        view,
        0,
        0,
        sidebar_w as u32,
        phys_h as u32,
        sidebar_bg,
    );

    // Draw workspace entries.
    for (i, ws) in state.workspaces.iter().enumerate() {
        let entry_y = i as f32 * cell_h;
        if entry_y + cell_h > phys_h {
            break; // No more room.
        }

        let is_active = i == state.active_workspace;

        // Highlight active entry.
        if is_active {
            state.renderer.submit_separator(
                view,
                0,
                entry_y as u32,
                sidebar_w as u32,
                cell_h as u32,
                active_entry_bg,
            );
            // Draw accent indicator on the left edge.
            state.renderer.submit_separator(
                view,
                0,
                entry_y as u32,
                3,
                cell_h as u32,
                accent_color,
            );
        }

        let fg = if is_active { active_fg } else { sidebar_fg };
        let bg = if is_active { active_entry_bg } else { sidebar_bg };

        // Format: " N  workspace-name [tab_count]"
        let tab_count = ws.tabs.len();
        let tab_indicator = if tab_count > 1 {
            format!(" ({tab_count})")
        } else {
            String::new()
        };
        let label = format!(" {}  {}{}", i + 1, ws.name, tab_indicator);
        // Truncate to fit sidebar width.
        let max_chars = (sidebar_w / cell_w) as usize;
        let display: String = label.chars().take(max_chars).collect();
        state.renderer.render_text(view, &display, 4.0, entry_y, fg, bg);
    }
}

/// Render all panes with tab bar and sidebar.
fn render_frame(state: &mut AppState) -> Result<(), jterm_render::RenderError> {
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let pane_rects = active_pane_rects(state);
    let focused_id = active_tab(state).layout.focused();
    let has_tab_bar = active_ws(state).tabs.len() > 1;

    // Always use the multi-pane path since we may have the tab bar/sidebar occupying space.
    let output = state.renderer.get_surface_texture()?;
    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // Clear entire surface.
    state.renderer.clear_surface(&view);

    // Render sidebar if visible.
    if state.sidebar_visible {
        render_sidebar(state, &view, phys_h);
    }

    // Render tab bar only if workspace has >1 tabs.
    if has_tab_bar {
        render_tab_bar(state, &view, phys_w);
    }

    // Render each pane.
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    for (pid, rect) in &pane_rects {
        if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(pid) {
            let sel_bounds = sel_bounds_for(pane);
            let preedit = if *pid == focused_id {
                pane.preedit.as_deref()
            } else {
                None
            };
            let viewport = (
                rect.x as u32,
                rect.y as u32,
                (rect.w as u32).max(1),
                (rect.h as u32).max(1),
            );
            state
                .renderer
                .render_pane(&pane.terminal, sel_bounds, viewport, *pid, preedit, &view)?;
        }
    }

    // Draw separators between panes.
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

    // Focus border on the focused pane (only when multiple panes).
    if pane_rects.len() > 1 {
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
    }

    // Update IME cursor position for the focused pane.
    if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
        if let Some(fp) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(&focused_id) {
            let cell_size = state.renderer.cell_size();
            let x = rect.x + (fp.terminal.cursor_col as f32 * cell_size.width);
            let y = rect.y + (fp.terminal.cursor_row as f32 * cell_size.height);
            state.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x as f64, y as f64),
                winit::dpi::PhysicalSize::new(
                    cell_size.width as f64,
                    cell_size.height as f64,
                ),
            );
        }
    }

    // Render command palette overlay if visible.
    if state.command_palette.visible {
        render_command_palette(state, &view, phys_w, phys_h);
    }

    output.present();
    Ok(())
}

/// Render the command palette as an overlay on top of the terminal.
fn render_command_palette(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = [0.0, 0.0, 0.0, 0.5];
    state.renderer.submit_separator(
        view,
        0,
        0,
        phys_w as u32,
        phys_h as u32,
        overlay_color,
    );

    // 2. Centered floating box: ~60% width, max 400px height.
    let box_w = (phys_w * 0.6).min(phys_w - 40.0).max(200.0);
    let max_box_h: f32 = 400.0;
    // Height depends on number of filtered items + input row.
    let max_visible_items = 10usize;
    let visible_items = state.command_palette.filtered.len().min(max_visible_items);
    let rows_needed = 1 + visible_items; // input row + command rows
    let box_h = ((rows_needed as f32) * cell_h + cell_h).min(max_box_h); // extra padding
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background.
    let box_bg = [0.12, 0.12, 0.16, 0.95];
    state.renderer.submit_separator(
        view,
        box_x as u32,
        box_y as u32,
        box_w as u32,
        box_h as u32,
        box_bg,
    );

    // Draw a subtle border around the box.
    let border_color = [0.3, 0.3, 0.4, 1.0];
    let b = 1u32;
    let bx = box_x as u32;
    let by = box_y as u32;
    let bw = box_w as u32;
    let bh = box_h as u32;
    state.renderer.submit_separator(view, bx, by, bw, b, border_color); // top
    state.renderer.submit_separator(view, bx, by + bh - b, bw, b, border_color); // bottom
    state.renderer.submit_separator(view, bx, by, b, bh, border_color); // left
    state.renderer.submit_separator(view, bx + bw - b, by, b, bh, border_color); // right

    // 3. Input field at the top of the box.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let prompt = format!("> {}", state.command_palette.input);
    let input_fg = [0.95, 0.95, 0.95, 1.0];
    state.renderer.render_text(view, &prompt, input_x, input_y, input_fg, box_bg);

    // Draw a separator line below the input.
    let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
    let sep_color = [0.25, 0.25, 0.3, 1.0];
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        sep_y,
        box_w as u32 - 2,
        1,
        sep_color,
    );

    // 4. Filtered command list.
    let list_start_y = sep_y as f32 + cell_h * 0.25;
    let cmd_fg = [0.8, 0.8, 0.82, 1.0];
    let selected_bg = [0.22, 0.22, 0.32, 1.0];
    let desc_fg = [0.5, 0.5, 0.55, 1.0];

    // Calculate max characters that fit in the box (for truncation).
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    for (i, &cmd_idx) in state
        .command_palette
        .filtered
        .iter()
        .enumerate()
        .take(max_visible_items)
    {
        let item_y = list_start_y + (i as f32) * cell_h;
        if item_y + cell_h > box_y + box_h {
            break;
        }

        let is_selected = i == state.command_palette.selected;
        let bg = if is_selected { selected_bg } else { box_bg };

        // Highlight selected row.
        if is_selected {
            state.renderer.submit_separator(
                view,
                box_x as u32 + 1,
                item_y as u32,
                box_w as u32 - 2,
                cell_h as u32,
                selected_bg,
            );
        }

        let cmd = &state.command_palette.commands[cmd_idx];

        // Render name in brighter color, description in dimmer color.
        let name_display: String = cmd.name.chars().take(max_chars).collect();
        let fg = if is_selected { input_fg } else { cmd_fg };
        state.renderer.render_text(
            view,
            &name_display,
            input_x,
            item_y,
            fg,
            bg,
        );

        // Render description after the name.
        let desc_offset = name_display.len() as f32 * cell_w + 2.0 * cell_w;
        if desc_offset < box_w - 2.0 * cell_w {
            let remaining = max_chars.saturating_sub(name_display.len() + 2);
            let desc_display: String = cmd.description.chars().take(remaining).collect();
            state.renderer.render_text(
                view,
                &desc_display,
                input_x + desc_offset,
                item_y,
                desc_fg,
                bg,
            );
        }
    }

    // Show "No matches" if filtered list is empty.
    if state.command_palette.filtered.is_empty() {
        let no_match = "No matching commands";
        let empty_fg = [0.5, 0.5, 0.55, 1.0];
        state.renderer.render_text(
            view,
            no_match,
            input_x,
            list_start_y,
            empty_fg,
            box_bg,
        );
    }
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
