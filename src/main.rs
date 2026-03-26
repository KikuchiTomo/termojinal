//! termojinal — GPU-accelerated multi-pane terminal emulator.

mod allow_flow;
mod appearance;
mod command_ui;
mod config;
mod dir_tree;
mod ipc;
mod notification;
mod palette;
mod platform;
mod quick_terminal;
mod ui;

pub(crate) use dir_tree::*;
pub(crate) use ipc::*;
pub(crate) use palette::*;
pub(crate) use platform::*;
pub(crate) use quick_terminal::*;
pub(crate) use ui::*;
mod actions;
mod daemon_client;
mod input;
mod status;
mod types;
mod workspace;

pub(crate) use actions::*;
pub(crate) use daemon_client::*;
pub(crate) use input::*;
pub(crate) use status::*;
pub(crate) use types::*;
pub(crate) use workspace::*;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use config::{
    color_or, format_tab_title, load_config, resolve_theme, TermojinalConfig,
};

use termojinal_ipc::app_protocol::AppIpcResponse;
use termojinal_ipc::command_loader;
use termojinal_ipc::keybinding::{Action, KeybindingConfig};

use command_ui::{CommandExecution, CommandKeyResult, CommandUIState};
use termojinal_layout::{LayoutTree, PaneId, SplitDirection};
use termojinal_render::{FontConfig, Renderer, ThemePalette};
use termojinal_vt::{ClipboardEvent, MouseMode};

use termojinal_claude::monitor::SessionState;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, WindowAttributes, WindowId};

// ---------------------------------------------------------------------------
// Claudes Dashboard (Multi-Agent Dashboard)
// ---------------------------------------------------------------------------

/// State for the Claudes Dashboard overlay.
struct ClaudesDashboard {
    visible: bool,
    entries: Vec<DashboardEntry>,
    selected_idx: usize,
    scroll_offset: usize,
    detail_scroll_offset: usize,
    /// Timestamp of last data refresh.
    last_refresh: std::time::Instant,
}

impl ClaudesDashboard {
    fn new() -> Self {
        Self {
            visible: false,
            entries: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            detail_scroll_offset: 0,
            last_refresh: std::time::Instant::now(),
        }
    }

    fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.selected_idx = 0;
            self.scroll_offset = 0;
            self.detail_scroll_offset = 0;
        }
    }

    fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected_idx = if self.selected_idx + 1 >= self.entries.len() {
                0
            } else {
                self.selected_idx + 1
            };
            self.detail_scroll_offset = 0;
        }
    }

    fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected_idx = if self.selected_idx == 0 {
                self.entries.len() - 1
            } else {
                self.selected_idx - 1
            };
            self.detail_scroll_offset = 0;
        }
    }

    fn ensure_visible(&mut self, max_visible: usize) {
        if max_visible == 0 {
            return;
        }
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + max_visible {
            self.scroll_offset = self.selected_idx + 1 - max_visible;
        }
    }
}

/// A single entry in the Claudes Dashboard.
struct DashboardEntry {
    pane_id: u64,
    workspace_idx: usize,
    session_id: String,
    title: String,
    state: SessionState,
    model: String,
    context_used: u64,
    context_max: u64,
    tokens_used: u64,
    cost_estimate: f64,
    cwd: String,
    workspace_name: String,
    subagents: Vec<termojinal_claude::monitor::SubAgentState>,
    tool_usage: HashMap<String, u32>,
    started_at: u64,
}

// ---------------------------------------------------------------------------
// Command Palette
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Daemon Connection (synchronous, for GUI thread)
// ---------------------------------------------------------------------------

/// Helper to access the active workspace immutably.
pub(crate) fn active_ws(state: &AppState) -> &Workspace {
    &state.workspaces[state.active_workspace]
}

/// Helper to access the active workspace mutably.
pub(crate) fn active_ws_mut(state: &mut AppState) -> &mut Workspace {
    &mut state.workspaces[state.active_workspace]
}

/// Helper to access the active tab of the active workspace immutably.
pub(crate) fn active_tab(state: &AppState) -> &Tab {
    let ws = active_ws(state);
    &ws.tabs[ws.active_tab]
}

/// Helper to access the active tab of the active workspace mutably.
pub(crate) fn active_tab_mut(state: &mut AppState) -> &mut Tab {
    let ws = active_ws_mut(state);
    let idx = ws.active_tab;
    &mut ws.tabs[idx]
}

/// Whether the tab bar should be visible for the active workspace.
pub(crate) fn tab_bar_visible(state: &AppState) -> bool {
    // Quick Terminal mode can suppress the tab bar.
    if state.quick_terminal.visible && !state.config.quick_terminal.show_tab_bar {
        return false;
    }
    let ws = active_ws(state);
    state.config.tab_bar.always_show || ws.tabs.len() > 1
}

/// Update the display title of a tab based on the focused pane's state.
/// `fallback_cwd` is the CWD from lsof (used when OSC 7 is unavailable).
pub(crate) fn update_tab_title(tab: &mut Tab, format: &str, tab_index: usize, fallback_cwd: &str) {
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get(&focused_id) {
        let title = &pane.terminal.osc.title;
        let cwd = if pane.terminal.osc.cwd.is_empty() {
            fallback_cwd
        } else {
            &pane.terminal.osc.cwd
        };
        let new_title = format_tab_title(format, title, cwd, tab_index);
        if new_title != tab.display_title {
            tab.display_title = new_title;
        }
    }
}

/// Abbreviate the home directory prefix in a path with `~`.
pub(crate) fn abbreviate_home(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if path.starts_with(&home) {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

/// Update the window title to reflect the focused pane's current state.
/// Format: "{title} — termojinal" or just "termojinal" if no info is available.
pub(crate) fn update_window_title(state: &AppState) {
    if state.workspaces.is_empty() {
        state.window.set_title("termojinal");
        return;
    }
    let ws = &state.workspaces[state.active_workspace];
    if ws.tabs.is_empty() {
        state.window.set_title("termojinal");
        return;
    }
    let tab = &ws.tabs[ws.active_tab];
    let focused_id = tab.layout.focused();

    let title = if let Some(pane) = tab.panes.get(&focused_id) {
        let osc_title = &pane.terminal.osc.title;
        let osc_cwd = &pane.terminal.osc.cwd;
        if !osc_title.is_empty() {
            format!("{osc_title} \u{2014} termojinal")
        } else if !osc_cwd.is_empty() {
            let display_cwd = abbreviate_home(osc_cwd);
            format!("{display_cwd} \u{2014} termojinal")
        } else {
            // Try fallback CWD from status collector cache.
            let fallback = &state.pane_git_cache.cwd;
            if !fallback.is_empty() {
                let display_cwd = abbreviate_home(fallback);
                format!("{display_cwd} \u{2014} termojinal")
            } else {
                "termojinal".to_string()
            }
        }
    } else {
        "termojinal".to_string()
    };

    state.window.set_title(&title);
}

/// Compute the content area that excludes the tab bar, sidebar, and status bar.
/// Effective status bar height (accounts for cell height minimum).
pub(crate) fn effective_status_bar_height(state: &AppState) -> f32 {
    if !state.config.status_bar.enabled {
        return 0.0;
    }
    // Quick Terminal mode can suppress the status bar.
    if state.quick_terminal.visible && !state.config.quick_terminal.show_status_bar {
        return 0.0;
    }
    let cell_h = state.renderer.cell_size().height;
    let bar_pad = 4.0_f32;
    state.config.status_bar.height.max(cell_h + bar_pad * 2.0)
}

/// Returns (content_x, content_y, content_w, content_h) in physical pixels.
pub(crate) fn content_area(state: &AppState, phys_w: f32, phys_h: f32) -> (f32, f32, f32, f32) {
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let tab_bar_h = if tab_bar_visible(state) {
        state.config.tab_bar.height
    } else {
        0.0
    };
    let status_bar_h = effective_status_bar_height(state);
    let content_x = sidebar_w;
    let content_y = tab_bar_h;
    let content_w = (phys_w - sidebar_w).max(1.0);
    let content_h = (phys_h - tab_bar_h - status_bar_h).max(1.0);
    (content_x, content_y, content_w, content_h)
}

/// Get pane rects for the active tab of the active workspace, offset by tab bar + sidebar.
pub(crate) fn active_pane_rects(state: &AppState) -> Vec<(PaneId, termojinal_layout::Rect)> {
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

/// Remove stale session-to-workspace mappings (inactive agents older than 1 hour).
fn cleanup_stale_session_mappings(state: &mut AppState) {
    let one_hour = std::time::Duration::from_secs(3600);
    state.session_to_workspace.retain(|session_id, &mut idx| {
        if idx >= state.agent_infos.len() {
            return false;
        }
        let agent = &state.agent_infos[idx];
        if agent.active {
            return true;
        }
        let id_matches = agent
            .session_id
            .as_deref()
            .map_or(false, |sid| sid == session_id);
        if !id_matches {
            return false;
        }
        agent.last_updated.elapsed() < one_hour
    });
}

struct App {
    state: Option<AppState>,
    proxy: EventLoopProxy<UserEvent>,
    pty_buffers: Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    config: Option<TermojinalConfig>,
    /// Whether `--quick-terminal` was passed on the command line.
    quick_terminal_mode: bool,
    /// Handle for communicating with the termojinald daemon.
    #[allow(dead_code)]
    daemon: DaemonHandle,
    /// Global shutdown flag for background threads spawned before AppState.
    app_shutdown: Arc<AtomicBool>,
}

impl App {
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        config: TermojinalConfig,
        app_shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: None,
            proxy,
            pty_buffers: Arc::new(Mutex::new(HashMap::new())),
            config: Some(config),
            quick_terminal_mode: false,
            daemon: DaemonHandle::new(),
            app_shutdown,
        }
    }
}

// ---------------------------------------------------------------------------
// Directory resolution helpers
// ---------------------------------------------------------------------------

/// Clear IME preedit on the focused pane of the active tab.
/// Must be called before switching workspace/tab/pane so that the
/// in-progress composition is discarded cleanly.
pub(crate) fn clear_focused_preedit(state: &mut AppState) {
    let tab = active_tab_mut(state);
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get_mut(&focused_id) {
        pane.preedit = None;
    }
}

/// Get the working directory of the focused pane in the active tab.
pub(crate) fn focused_pane_cwd(state: &AppState) -> Option<String> {
    let tab = active_tab(state);
    let focused_id = tab.layout.focused();
    if let Some(pane) = tab.panes.get(&focused_id) {
        let osc_cwd = &pane.terminal.osc.cwd;
        if !osc_cwd.is_empty() {
            return Some(osc_cwd.clone());
        }
    }
    // Fallback to git cache cwd
    let cwd = &state.pane_git_cache.cwd;
    if !cwd.is_empty() {
        Some(cwd.clone())
    } else {
        None
    }
}

/// Expand `~` at the start of a path to the user's home directory.
pub(crate) fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return path.replacen('~', &home.to_string_lossy(), 1);
        }
    }
    path.to_string()
}

/// Validate and expand a configured directory path.
/// Returns `None` if the directory is empty or does not exist.
pub(crate) fn validate_dir(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let expanded = expand_tilde(path);
    if std::path::Path::new(&expanded).is_dir() {
        Some(expanded)
    } else {
        log::warn!("configured directory does not exist: {expanded}");
        None
    }
}

/// Determine the working directory for a new pane/tab based on config.
pub(crate) fn resolve_new_pane_cwd(state: &AppState) -> Option<String> {
    match state.config.pane.working_directory {
        config::PaneWorkingDirectory::Inherit => focused_pane_cwd(state),
        config::PaneWorkingDirectory::Home => std::env::var("HOME")
            .ok()
            .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string())),
        config::PaneWorkingDirectory::Fixed => validate_dir(&state.config.pane.fixed_directory),
    }
}

/// Determine the working directory for the initial pane on startup.
fn resolve_startup_cwd(config: &config::TermojinalConfig) -> Option<String> {
    // If the user set startup.directory, honour it regardless of mode
    // (except Restore, which explicitly overrides with last session CWD).
    if !config.startup.directory.is_empty()
        && !matches!(config.startup.mode, config::StartupMode::Restore)
    {
        if let Some(dir) = validate_dir(&config.startup.directory) {
            return Some(dir);
        }
        log::warn!(
            "startup.directory {:?} is not a valid directory, falling back",
            config.startup.directory
        );
    }

    match config.startup.mode {
        config::StartupMode::Default | config::StartupMode::Fixed => {
            // Use $HOME so the terminal doesn't inherit the
            // (often meaningless) parent-process CWD (e.g. "/" from Finder).
            std::env::var("HOME")
                .ok()
                .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        }
        config::StartupMode::Restore => {
            load_last_cwd().or_else(|| {
                // If no saved CWD, fall back to $HOME.
                std::env::var("HOME")
                    .ok()
                    .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
            })
        }
    }
}

/// State file path for persisting last CWD.
fn last_cwd_path() -> std::path::PathBuf {
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir.join("termojinal").join("last_cwd.txt")
}

/// Load the last saved CWD from disk.
fn load_last_cwd() -> Option<String> {
    let path = last_cwd_path();
    std::fs::read_to_string(&path).ok().and_then(|s| {
        let s = s.trim().to_string();
        if s.is_empty() || !std::path::Path::new(&s).is_dir() {
            None
        } else {
            Some(s)
        }
    })
}

/// Save the current CWD to disk for restore on next startup.
fn save_last_cwd(cwd: &str) {
    let path = last_cwd_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, cwd);
}

// ---------------------------------------------------------------------------
// Pane spawning helper
// ---------------------------------------------------------------------------

/// used by termojinal-ipc (e.g., "cmd+d", "ctrl+c", "cmd+shift+enter").

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let opacity = self.config.as_ref().map_or(1.0, |c| c.window.opacity);
        let transparent = opacity < 1.0;
        log::info!(
            "config present={}, opacity={opacity}, transparent={transparent}",
            self.config.is_some()
        );
        if let Some(c) = &self.config {
            log::info!(
                "config font.size={}, window={}x{}",
                c.font.size,
                c.window.width,
                c.window.height
            );
        }

        let attrs = WindowAttributes::default()
            .with_title("termojinal")
            .with_inner_size(LogicalSize::new(
                self.config.as_ref().map_or(960, |c| c.window.width),
                self.config.as_ref().map_or(640, |c| c.window.height),
            ))
            .with_transparent(transparent);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let cfg = self.config.as_ref().map(|c| c.clone()).unwrap_or_default();
        let initial_scale_factor = window.scale_factor();
        let font_config = FontConfig {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
            line_height: cfg.font.line_height,
        };
        let mut renderer = match pollster::block_on(Renderer::new(window.clone(), &font_config)) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        // Resolve theme (with auto-switch if enabled).
        let current_appearance = appearance::detect_macos_appearance();
        let effective_theme = resolve_theme(&cfg, current_appearance);

        // Build theme palette from effective theme and apply to renderer.
        let palette = ThemePalette::from_theme_colors(
            &effective_theme.background,
            &effective_theme.foreground,
            &effective_theme.black,
            &effective_theme.red,
            &effective_theme.green,
            &effective_theme.yellow,
            &effective_theme.blue,
            &effective_theme.magenta,
            &effective_theme.cyan,
            &effective_theme.white,
            &effective_theme.bright_black,
            &effective_theme.bright_red,
            &effective_theme.bright_green,
            &effective_theme.bright_yellow,
            &effective_theme.bright_blue,
            &effective_theme.bright_magenta,
            &effective_theme.bright_cyan,
            &effective_theme.bright_white,
        );
        renderer.set_theme(palette);

        // Set renderer fields from config.
        renderer.default_bg = color_or(&effective_theme.background, [0.067, 0.067, 0.09, 1.0]);
        renderer.bg_opacity = opacity;
        renderer.preedit_bg = color_or(&effective_theme.preedit_bg, [0.15, 0.15, 0.20, 1.0]);
        renderer.scrollbar_thumb_opacity = cfg.pane.scrollbar_thumb_opacity;
        renderer.scrollbar_track_opacity = cfg.pane.scrollbar_track_opacity;

        // Resolve CJK ambiguous width setting from config and propagate to
        // renderer and atlas so that character widths are consistent everywhere.
        let cjk_width = crate::config::resolve_ambiguous_width(&cfg.font.ambiguous_width);
        log::info!(
            "CJK ambiguous width: {cjk_width} (setting={:?})",
            cfg.font.ambiguous_width
        );
        renderer.cjk_width = cjk_width;
        renderer.atlas_set_cjk_width(cjk_width);

        let size = window.inner_size();
        let phys_w = size.width as f32;
        let phys_h = size.height as f32;
        // Compute the initial content area matching what resize_all_panes will
        // use, so the PTY is spawned with the exact grid size the shell will
        // see — preventing a SIGWINCH resize (which causes an extra newline
        // and the zsh `%` marker on startup).
        let initial_sidebar_w = cfg.sidebar.width;
        let initial_tab_bar_h = if cfg.tab_bar.always_show {
            cfg.tab_bar.height
        } else {
            0.0
        };
        let cell_h = renderer.cell_size().height;
        let bar_pad = 4.0_f32;
        let initial_status_bar_h = if cfg.status_bar.enabled {
            cfg.status_bar.height.max(cell_h + bar_pad * 2.0)
        } else {
            0.0
        };
        let content_w = (phys_w - initial_sidebar_w).max(1.0);
        let content_h = (phys_h - initial_tab_bar_h - initial_status_bar_h).max(1.0);
        let (cols, rows) = renderer.grid_size_raw(content_w as u32, content_h as u32);
        log::info!(
            "window {}x{} -> grid {cols}x{rows}",
            size.width,
            size.height
        );

        // Create the initial pane (id 0) in the first workspace, first tab.
        let initial_id: PaneId = 0;
        let layout = LayoutTree::new(initial_id);

        let startup_cwd = resolve_startup_cwd(&cfg);
        let pane = match spawn_pane(
            initial_id,
            cols,
            rows,
            &self.proxy,
            &self.pty_buffers,
            startup_cwd,
            Some(&cfg.time_travel),
            cjk_width,
        ) {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to spawn initial pane: {e}");
                // Show a user-facing error dialog via native macOS alert
                #[cfg(target_os = "macos")]
                {
                    use std::process::Command;
                    let msg = format!(
                        "Termojinal requires the daemon (termojinald) to be running.\n\n\
                         Start it with:\n  termojinald &\n\n\
                         Or via Homebrew:\n  brew services start termojinal\n\n\
                         Error: {e}"
                    );
                    let _ = Command::new("osascript")
                        .args(["-e", &format!(
                            "display dialog \"{}\" with title \"Termojinal\" buttons {{\"OK\"}} default button \"OK\" with icon stop",
                            msg.replace('\"', "\\\"").replace('\n', "\\n")
                        )])
                        .output();
                }
                eprintln!("Error: termojinald is not running. Start it with: termojinald &");
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
            display_title: format_tab_title(&cfg.tab_bar.format, "", "", 1),
        };

        let initial_workspace = Workspace {
            tabs: vec![initial_tab],
            active_tab: 0,
            name: "Workspace 1".to_string(),
        };

        let keybindings = KeybindingConfig::load();

        // Load external commands from ~/.config/termojinal/commands/.
        let external_commands = command_loader::load_commands();
        log::info!("loaded {} external commands", external_commands.len());

        // Build the command palette with external commands appended.
        let mut palette = CommandPalette::new();
        for cmd in &external_commands {
            let kind = if cmd.verify_result.is_verified() {
                CommandKind::PluginVerified
            } else {
                CommandKind::Plugin
            };
            palette.commands.push(PaletteCommand {
                name: cmd.meta.name.clone(),
                description: cmd.meta.description.clone(),
                action: Action::Command(cmd.meta.name.clone()),
                kind,
            });
        }
        palette.update_filter();

        self.state = Some(AppState {
            window,
            renderer,
            workspaces: vec![initial_workspace],
            active_workspace: 0,
            keybindings,
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
            drag_resize: None,
            scrollbar_drag: None,
            next_pane_id: 1, // 0 is already used
            sidebar_visible: true,
            sidebar_width: cfg.sidebar.width,
            sidebar_drag: false,
            command_palette: palette,
            font_size: cfg.font.size,
            search: None,
            workspace_infos: vec![WorkspaceInfo::new()],
            agent_infos: vec![AgentSessionInfo::default()],
            app_start_time: std::time::Instant::now(),
            tab_drag: None,
            pending_tab_click: None,
            tab_pane_drag: None,
            config: cfg.clone(),
            status_cache: StatusCache::new(),
            pane_git_cache: PaneGitCache::new(),
            status_collector: AsyncStatusCollector::new(self.proxy.clone()),
            workspace_refresher: AsyncWorkspaceRefresher::new(self.proxy.clone()),
            claude_monitor: termojinal_claude::monitor::ClaudeSessionMonitor::new(),
            scale_factor: initial_scale_factor,
            allow_flow: allow_flow::AllowFlowUI::new(cfg.allow_flow.clone()),
            pending_ipc_responses: HashMap::new(),
            session_to_workspace: HashMap::new(),
            command_execution: None,
            external_commands,
            quick_terminal: QuickTerminalState::new(),
            about_visible: false,
            about_scroll: 0,
            dir_trees: vec![DirectoryTreeState::new()],
            timeline_visible: false,
            timeline_input: String::new(),
            timeline_selected: 0,
            timeline_scroll_offset: 0,
            timeline_pane_id: None,
            claudes_collapsed: false,
            claudes_dashboard: ClaudesDashboard::new(),
            sessions_collapsed: false,
            daemon_sessions: Vec::new(),
            pending_close_confirm: None, // (proc_name, pane_id)
            needs_animation_frame: false,
            last_animation_redraw: std::time::Instant::now(),
            scroll_accum: 0.0,
            selection_auto_scroll: None,
            quick_launch: QuickLaunchState::new(),
            update_checker: UpdateChecker::new(),
            update_check_result: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
        });

        // Activate Quick Terminal mode if --quick-terminal was passed.
        if self.quick_terminal_mode {
            if let Some(state) = self.state.as_mut() {
                state.quick_terminal.active = true;
                log::info!("quick terminal state activated");
            }
        }

        // Detect ProMotion display and try low-latency present mode.
        if let Some(state) = self.state.as_mut() {
            let monitor = state.window.current_monitor();
            if let Some(m) = monitor {
                let refresh = m.refresh_rate_millihertz().unwrap_or(60000);
                if refresh > 60000 {
                    log::info!("high refresh rate display detected: {}Hz", refresh / 1000);
                    // Try Mailbox first (low latency), fall back to Immediate.
                    // If neither is supported, keep the default Fifo.
                    if state
                        .renderer
                        .try_set_present_mode(wgpu::PresentMode::Mailbox)
                        || state
                            .renderer
                            .try_set_present_mode(wgpu::PresentMode::Immediate)
                    {
                        log::info!("using low-latency present mode");
                    }
                }
            }
        }

        // On macOS, set window background to clear for transparency to work.
        #[cfg(target_os = "macos")]
        if transparent {
            if let Some(state) = self.state.as_ref() {
                set_macos_window_transparent(&state.window);
            }
        }

        // Set Dock icon now that NSApplication is fully initialized.
        set_dock_icon();

        // Initialize notification system (sets bundle ID for app icon in notifications).
        notification::init();

        // Request notification permission if not already granted.
        #[cfg(target_os = "macos")]
        notification::request_notification_permission_if_needed();

        // Start background Homebrew update check.
        if let Some(state) = self.state.as_mut() {
            if !state.update_checker.checked {
                let result = state.update_check_result.clone();
                UpdateChecker::start_check(result);
            }
            state.update_checker.checked = true;
        }

        // Enable IME after window is fully created and request initial redraw.
        let Some(state) = self.state.as_ref() else { return };
        state.window.set_ime_allowed(true);
        // Set initial IME cursor area so macOS will start sending IME events.
        state.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(0.0, 0.0),
            winit::dpi::PhysicalSize::new(
                state.renderer.cell_size().width as f64,
                state.renderer.cell_size().height as f64,
            ),
        );
        // Seed the background status collector with the initial pane's PID.
        {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                state
                    .status_collector
                    .update_request(pane.shell_pid, &pane.terminal.osc.cwd);
            }
        }
        state.window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                // Signal all background threads to stop.
                state.shutdown.store(true, Ordering::SeqCst);
                state.status_collector.shutdown.store(true, Ordering::SeqCst);
                state.workspace_refresher.shutdown.store(true, Ordering::SeqCst);
                self.app_shutdown.store(true, Ordering::SeqCst);
                // Save last CWD for restore_last_directory feature
                if state.config.startup.mode == config::StartupMode::Restore {
                    if let Some(cwd) = focused_pane_cwd(state) {
                        save_last_cwd(&cwd);
                    }
                }
                // Detach PTY handles so daemon-tracked sessions survive the GUI exit.
                detach_all_ptys(state);
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

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let old_scale = state.scale_factor;
                state.scale_factor = scale_factor;
                state.renderer.scale_factor = scale_factor as f32;
                log::info!("scale factor changed: {old_scale} -> {scale_factor}");

                // Rebuild font atlas at new DPI. Logical font size unchanged,
                // but physical rasterization pixels = logical * scale_factor.
                if let Err(e) = state.renderer.set_font_size(state.font_size) {
                    log::error!("failed to rebuild font atlas on scale change: {e}");
                }
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
                        let _ = daemon_pty_write(&pane.session_id, seq);
                    }
                }

                // Quick Terminal: hide on focus loss if configured.
                if !focused
                    && state.quick_terminal.active
                    && state.quick_terminal.visible
                    && state.config.quick_terminal.hide_on_focus_loss
                {
                    hide_quick_terminal(state);
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                // Claudes Dashboard intercepts keyboard input when visible.
                if state.claudes_dashboard.visible {
                    // Determine navigation intent from key + modifiers.
                    let is_down = matches!(&event.logical_key, Key::Named(NamedKey::ArrowDown))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "j")
                        || (state.modifiers.control_key()
                            && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "n" || c.as_str() == "\x0e"));
                    let is_up = matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "k")
                        || (state.modifiers.control_key()
                            && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "p" || c.as_str() == "\x10"));
                    let is_enter = matches!(&event.logical_key, Key::Named(NamedKey::Enter));
                    let is_escape = matches!(&event.logical_key, Key::Named(NamedKey::Escape));

                    if is_escape {
                        state.claudes_dashboard.visible = false;
                        state.window.request_redraw();
                    } else if is_down {
                        state.claudes_dashboard.select_next();
                        state.window.request_redraw();
                    } else if is_up {
                        state.claudes_dashboard.select_prev();
                        state.window.request_redraw();
                    } else if is_enter {
                        // Jump to the selected pane.
                        if let Some(entry) = state
                            .claudes_dashboard
                            .entries
                            .get(state.claudes_dashboard.selected_idx)
                        {
                            let target_pane_id = entry.pane_id;
                            let target_ws = entry.workspace_idx;
                            state.claudes_dashboard.visible = false;
                            // Switch workspace if needed.
                            if target_ws < state.workspaces.len()
                                && target_ws != state.active_workspace
                            {
                                state.active_workspace = target_ws;
                                resize_all_panes(state);
                                update_window_title(state);
                            }
                            // Focus the target pane: find which tab contains it.
                            let ws = &mut state.workspaces[state.active_workspace];
                            for (ti, tab) in ws.tabs.iter().enumerate() {
                                if tab.panes.contains_key(&target_pane_id) {
                                    ws.active_tab = ti;
                                    break;
                                }
                            }
                            let ws = &mut state.workspaces[state.active_workspace];
                            let tab_idx = ws.active_tab;
                            ws.tabs[tab_idx].layout = ws.tabs[tab_idx].layout.focus(target_pane_id);
                            resize_all_panes(state);
                            state.window.request_redraw();
                        }
                    }
                    // Any other key is consumed (no pass-through).
                    return;
                }

                // About overlay intercepts all keyboard input when visible.
                if state.about_visible {
                    let scroll_down = matches!(&event.logical_key, Key::Named(NamedKey::ArrowDown))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "j");
                    let scroll_up = matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp))
                        || matches!(&event.logical_key, Key::Character(c) if c.as_str() == "k");
                    if scroll_down {
                        state.about_scroll = state.about_scroll.saturating_add(3);
                        state.window.request_redraw();
                    } else if scroll_up {
                        state.about_scroll = state.about_scroll.saturating_sub(3);
                        state.window.request_redraw();
                    } else {
                        // Any other key dismisses the about screen.
                        state.about_visible = false;
                        state.window.request_redraw();
                    }
                    return;
                }

                // Close confirmation dialog intercepts keyboard input.
                if let Some((_, confirm_pane_id)) = state.pending_close_confirm.clone() {
                    let buffers = &self.pty_buffers;
                    match &event.logical_key {
                        Key::Character(c) if c.as_str() == "y" || c.as_str() == "Y" => {
                            state.pending_close_confirm = None;
                            // Verify the pane still exists and is focused before closing.
                            let current_focused = active_tab(state).layout.focused();
                            if current_focused == confirm_pane_id {
                                close_focused_pane(state, buffers, event_loop);
                            } else {
                                // Pane changed — dismiss silently.
                                state.window.request_redraw();
                            }
                        }
                        Key::Character(c) if c.as_str() == "n" || c.as_str() == "N" => {
                            state.pending_close_confirm = None;
                            state.window.request_redraw();
                        }
                        Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter) => {
                            state.pending_close_confirm = None;
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                // Allow Flow: intercept physical keys even during IME composition.
                // This lets y/n/a work for fast-allow regardless of IME state.
                if state.allow_flow.first_workspace_with_pending().is_some() {
                    use winit::keyboard::KeyCode;
                    let physical = match event.physical_key {
                        winit::keyboard::PhysicalKey::Code(code) => Some(code),
                        _ => None,
                    };
                    let shift = state.modifiers.shift_key();
                    let no_mods = !state.modifiers.control_key()
                        && !state.modifiers.super_key()
                        && !state.modifiers.alt_key();
                    if no_mods {
                        if let Some(code) = physical {
                            let mapped = match (code, shift) {
                                (KeyCode::KeyY, false) => Some(Key::Character("y".into())),
                                (KeyCode::KeyY, true) => Some(Key::Character("Y".into())),
                                (KeyCode::KeyN, false) => Some(Key::Character("n".into())),
                                (KeyCode::KeyN, true) => Some(Key::Character("N".into())),
                                (KeyCode::KeyA, _) => {
                                    Some(Key::Character(if shift { "A" } else { "a" }.into()))
                                }
                                (KeyCode::Escape, _) => {
                                    Some(Key::Named(winit::keyboard::NamedKey::Escape))
                                }
                                _ => None,
                            };
                            if let Some(key) = mapped {
                                let active_ws = state.active_workspace;
                                let pane_sessions: std::collections::HashMap<u64, String> = {
                                    let mut m = std::collections::HashMap::new();
                                    for ws in &state.workspaces {
                                        for tab in &ws.tabs {
                                            for (pid, pane) in &tab.panes {
                                                m.insert(*pid, pane.session_id.clone());
                                            }
                                        }
                                    }
                                    m
                                };
                                let key_result =
                                    state
                                        .allow_flow
                                        .process_key(&key, active_ws, &pane_sessions);
                                match key_result {
                                    crate::allow_flow::AllowFlowKeyResult::NotConsumed => {}
                                    crate::allow_flow::AllowFlowKeyResult::Consumed => {
                                        state.window.request_redraw();
                                        return;
                                    }
                                    crate::allow_flow::AllowFlowKeyResult::Resolved(decisions) => {
                                        for (req_id, decision) in &decisions {
                                            if let Some((tx, _alive)) =
                                                state.pending_ipc_responses.remove(req_id)
                                            {
                                                let decision_str = match decision {
                                                    termojinal_claude::AllowDecision::Allow => {
                                                        "allow"
                                                    }
                                                    termojinal_claude::AllowDecision::Deny => {
                                                        "deny"
                                                    }
                                                };
                                                let _ = tx.send(AppIpcResponse::ok(
                                                    serde_json::json!({"decision": decision_str}),
                                                ));
                                            }
                                        }
                                        // Update agent state: if no more pending requests for
                                        // a workspace, transition agent from WaitingForPermission to Running.
                                        for wi in 0..state.agent_infos.len() {
                                            if state.agent_infos[wi].active
                                                && matches!(
                                                    state.agent_infos[wi].state,
                                                    AgentState::WaitingForPermission
                                                )
                                                && !state.allow_flow.has_pending_for_workspace(wi)
                                            {
                                                state.agent_infos[wi].state = AgentState::Running;
                                                state.agent_infos[wi].last_updated =
                                                    std::time::Instant::now();
                                            }
                                        }
                                        state.window.request_redraw();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                // Suppress raw key events during IME composition (but not when palette is open).
                let focused_id = active_tab(state).layout.focused();
                if !state.command_palette.visible {
                    if active_tab(state)
                        .panes
                        .get(&focused_id)
                        .map_or(false, |p| p.preedit.is_some())
                    {
                        return;
                    }
                }

                // --- Directory tree keyboard navigation ---
                // When the tree has focus, intercept keys before anything else.
                {
                    let wi = state.active_workspace;
                    let tree_has_focus = wi < state.dir_trees.len()
                        && state.dir_trees[wi].visible
                        && state.dir_trees[wi].focused;
                    if tree_has_focus {
                        let is_ctrl = state.modifiers.control_key();
                        let tree = &mut state.dir_trees[wi];

                        // --- Find mode: typing narrows to matching entry ---
                        if tree.find_active {
                            match &event.logical_key {
                                Key::Named(NamedKey::Escape) => {
                                    tree.find_query.clear();
                                    tree.find_active = false;
                                    state.window.request_redraw();
                                }
                                Key::Named(NamedKey::Enter) => {
                                    // Accept current match, exit find mode.
                                    tree.find_query.clear();
                                    tree.find_active = false;
                                    state.window.request_redraw();
                                }
                                Key::Named(NamedKey::Tab) => {
                                    // Cycle to next match (skip current).
                                    dir_tree_find_match_from(tree, 1);
                                    state.window.request_redraw();
                                }
                                Key::Named(NamedKey::Backspace) => {
                                    tree.find_query.pop();
                                    dir_tree_find_match(tree);
                                    state.window.request_redraw();
                                }
                                Key::Character(c) if !is_ctrl => {
                                    tree.find_query.push_str(c.as_str());
                                    dir_tree_find_match(tree);
                                    state.window.request_redraw();
                                }
                                _ => {}
                            }
                            return; // Consume ALL keys in find mode.
                        }

                        match &event.logical_key {
                            // Move down: Arrow Down, j, Ctrl+N
                            Key::Named(NamedKey::ArrowDown) => {
                                dir_tree_move_down(state);
                            }
                            Key::Character(c) if c.as_str() == "j" && !is_ctrl => {
                                dir_tree_move_down(state);
                            }
                            Key::Character(c)
                                if is_ctrl && (c.as_str() == "n" || c.as_str() == "\x0e") =>
                            {
                                dir_tree_move_down(state);
                            }
                            // Move up: Arrow Up, k, Ctrl+P
                            Key::Named(NamedKey::ArrowUp) => {
                                dir_tree_move_up(state);
                            }
                            Key::Character(c) if c.as_str() == "k" && !is_ctrl => {
                                dir_tree_move_up(state);
                            }
                            Key::Character(c)
                                if is_ctrl && (c.as_str() == "p" || c.as_str() == "\x10") =>
                            {
                                dir_tree_move_up(state);
                            }
                            // Expand: Arrow Right, l, Ctrl+F
                            Key::Named(NamedKey::ArrowRight) | Key::Character(_)
                                if matches!(
                                    &event.logical_key,
                                    Key::Named(NamedKey::ArrowRight)
                                ) || (matches!(&event.logical_key, Key::Character(c) if c.as_str() == "l" && !is_ctrl)) =>
                            {
                                dir_tree_expand(state);
                            }
                            Key::Character(c)
                                if is_ctrl && (c.as_str() == "f" || c.as_str() == "\x06") =>
                            {
                                dir_tree_expand(state);
                            }
                            // Collapse: Arrow Left, h, Ctrl+B
                            Key::Named(NamedKey::ArrowLeft) => {
                                dir_tree_collapse(state);
                            }
                            Key::Character(c) if c.as_str() == "h" && !is_ctrl => {
                                dir_tree_collapse(state);
                            }
                            Key::Character(c)
                                if is_ctrl && (c.as_str() == "b" || c.as_str() == "\x02") =>
                            {
                                dir_tree_collapse(state);
                            }
                            // Enter: toggle expand/collapse (directories) or open (files)
                            Key::Named(NamedKey::Enter) => {
                                let wi2 = state.active_workspace;
                                if wi2 < state.dir_trees.len() {
                                    let sel = state.dir_trees[wi2].selected;
                                    if sel < state.dir_trees[wi2].entries.len()
                                        && state.dir_trees[wi2].entries[sel].is_dir
                                    {
                                        toggle_tree_entry(&mut state.dir_trees[wi2], sel);
                                    } else {
                                        dir_tree_open_in_editor(
                                            state,
                                            &self.proxy,
                                            &self.pty_buffers,
                                        );
                                    }
                                }
                                state.window.request_redraw();
                            }
                            // e: cd to selected directory, then unfocus tree
                            Key::Character(c) if c.as_str() == "e" && !is_ctrl => {
                                dir_tree_cd(state);
                                let wi2 = state.active_workspace;
                                if wi2 < state.dir_trees.len() {
                                    state.dir_trees[wi2].focused = false;
                                }
                            }
                            // v: open selected file in new tab, then unfocus tree
                            Key::Character(c) if c.as_str() == "v" && !is_ctrl => {
                                dir_tree_open_in_editor(state, &self.proxy, &self.pty_buffers);
                                let wi2 = state.active_workspace;
                                if wi2 < state.dir_trees.len() {
                                    state.dir_trees[wi2].focused = false;
                                }
                            }
                            // f: enter find mode (prefix search)
                            Key::Character(c) if c.as_str() == "f" && !is_ctrl => {
                                let wi2 = state.active_workspace;
                                if wi2 < state.dir_trees.len() {
                                    state.dir_trees[wi2].find_active = true;
                                    state.dir_trees[wi2].find_query.clear();
                                    state.window.request_redraw();
                                }
                            }
                            // Escape or q: unfocus tree (return focus to terminal)
                            Key::Named(NamedKey::Escape) => {
                                state.dir_trees[wi].focused = false;
                                state.window.request_redraw();
                            }
                            Key::Character(c) if c.as_str() == "q" && !is_ctrl => {
                                state.dir_trees[wi].focused = false;
                                state.window.request_redraw();
                            }
                            // Cmd+Shift+E: toggle tree (same as global keybinding)
                            Key::Character(c)
                                if state.modifiers.super_key()
                                    && state.modifiers.shift_key()
                                    && (c.as_str() == "e" || c.as_str() == "E") =>
                            {
                                state.dir_trees[wi].visible = false;
                                state.dir_trees[wi].focused = false;
                                state.window.request_redraw();
                            }
                            // Tab: unfocus tree, keep it visible
                            Key::Named(NamedKey::Tab) => {
                                state.dir_trees[wi].focused = false;
                                state.window.request_redraw();
                            }
                            _ => {
                                // Tree has focus: consume ALL keys to prevent leaking to PTY.
                            }
                        };
                        return; // Always return when tree has focus.
                    }
                }

                // Emacs keybindings: Ctrl+N = Down, Ctrl+P = Up (in palette/command UI)
                let is_ctrl = state.modifiers.control_key();
                if is_ctrl
                    && (state.command_palette.visible
                        || state.command_execution.is_some()
                        || state.quick_launch.visible)
                {
                    match &event.logical_key {
                        Key::Character(c) if c.as_str() == "n" || c.as_str() == "\x0e" => {
                            if state.quick_launch.visible {
                                state.quick_launch.select_next();
                            } else if let Some(ref mut exec) = state.command_execution {
                                exec.selected = if exec.filtered_items.is_empty() {
                                    0
                                } else if exec.selected + 1 >= exec.filtered_items.len() {
                                    0
                                } else {
                                    exec.selected + 1
                                };
                            } else if state.command_palette.mode == PaletteMode::FileFinder {
                                state.command_palette.file_finder.select_next();
                            } else {
                                state.command_palette.select_next();
                            }
                            state.window.request_redraw();
                            return;
                        }
                        Key::Character(c) if c.as_str() == "p" || c.as_str() == "\x10" => {
                            if state.quick_launch.visible {
                                state.quick_launch.select_prev();
                            } else if let Some(ref mut exec) = state.command_execution {
                                exec.selected = if exec.filtered_items.is_empty() {
                                    0
                                } else if exec.selected == 0 {
                                    exec.filtered_items.len() - 1
                                } else {
                                    exec.selected - 1
                                };
                            } else if state.command_palette.mode == PaletteMode::FileFinder {
                                state.command_palette.file_finder.select_prev();
                            } else {
                                state.command_palette.select_prev();
                            }
                            state.window.request_redraw();
                            return;
                        }
                        _ => {}
                    }
                }

                // Active command execution intercepts ALL keyboard input.
                if state.command_palette.visible {
                    if let Some(ref mut cmd_exec) = state.command_execution {
                        let result = cmd_exec.handle_key(&event);
                        match result {
                            CommandKeyResult::Consumed => {
                                state.window.request_redraw();
                                return;
                            }
                            CommandKeyResult::Cancelled | CommandKeyResult::Dismiss => {
                                state.command_execution = None;
                                state.command_palette.visible = false;
                                state.window.request_redraw();
                                return;
                            }
                        }
                    }
                }

                // Command timeline intercepts keyboard input when visible.
                if state.timeline_visible {
                    match handle_timeline_key(state, &event) {
                        TimelineKeyResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        TimelineKeyResult::JumpToCommand(cmd_id) => {
                            let focused_id = active_tab(state).layout.focused();
                            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                                pane.terminal.jump_to_command(cmd_id);
                            }
                            state.timeline_visible = false;
                            state.window.request_redraw();
                            return;
                        }
                        TimelineKeyResult::RerunCommand(cmd_text) => {
                            state.timeline_visible = false;
                            let focused_id = active_tab(state).layout.focused();
                            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                                let cmd_with_newline = format!("{}\n", cmd_text);
                                let _ =
                                    daemon_pty_write(&pane.session_id, cmd_with_newline.as_bytes());
                            }
                            state.window.request_redraw();
                            return;
                        }
                        TimelineKeyResult::Dismiss => {
                            state.timeline_visible = false;
                            state.window.request_redraw();
                            return;
                        }
                        TimelineKeyResult::Pass => {}
                    }
                }

                // Quick Launch overlay intercepts keyboard input when visible.
                if state.quick_launch.visible {
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            state.quick_launch.visible = false;
                            state.window.request_redraw();
                            return;
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            state.quick_launch.select_prev();
                            state.window.request_redraw();
                            return;
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            state.quick_launch.select_next();
                            state.window.request_redraw();
                            return;
                        }
                        Key::Named(NamedKey::Enter) => {
                            if let Some(entry) = state.quick_launch.selected_entry().cloned() {
                                state.quick_launch.visible = false;
                                // Navigate to the selected target.
                                if entry.workspace_idx < state.workspaces.len() {
                                    state.active_workspace = entry.workspace_idx;
                                    let ws = &mut state.workspaces[entry.workspace_idx];
                                    if entry.tab_idx < ws.tabs.len() {
                                        ws.active_tab = entry.tab_idx;
                                    }
                                    if let Some(pane_id) = entry.pane_id {
                                        let tab = &mut ws.tabs[ws.active_tab];
                                        tab.layout = tab.layout.focus(pane_id);
                                    }
                                }
                                if entry.workspace_idx < state.workspace_infos.len() {
                                    state.workspace_infos[entry.workspace_idx].has_unread = false;
                                }
                                resize_all_panes(state);
                                update_window_title(state);
                                state.window.request_redraw();
                            }
                            return;
                        }
                        Key::Named(NamedKey::Backspace) => {
                            if state.quick_launch.input.is_empty() {
                                state.quick_launch.visible = false;
                            } else {
                                state.quick_launch.input.pop();
                                state.quick_launch.update_filter();
                            }
                            state.window.request_redraw();
                            return;
                        }
                        _ => {
                            if let Some(ref text) = event.text {
                                if !text.is_empty() && !text.contains('\r') {
                                    state.quick_launch.input.push_str(text);
                                    state.quick_launch.update_filter();
                                    state.window.request_redraw();
                                    return;
                                }
                            }
                        }
                    }
                    return; // Consume all keys when Quick Launch is visible.
                }

                // Command palette intercepts ALL keyboard input when visible.
                if state.command_palette.visible {
                    match state.command_palette.handle_key(&event, state.modifiers) {
                        PaletteResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Execute(action) => {
                            state.command_palette.visible = false;
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
                        PaletteResult::OpenInEditor(path) => {
                            state.command_palette.visible = false;
                            palette_open_in_editor(state, &path, &self.proxy, &self.pty_buffers);
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::CdToDirectory(path) => {
                            state.command_palette.visible = false;
                            palette_cd_to_dir(state, &path);
                            state.window.request_redraw();
                            return;
                        }
                        PaletteResult::Pass => {}
                    }
                }

                // Search bar intercepts keyboard input when visible (Feature 5).
                if state.search.is_some() {
                    let search_result = handle_search_key(state, &event);
                    match search_result {
                        SearchKeyResult::Consumed => {
                            state.window.request_redraw();
                            return;
                        }
                        SearchKeyResult::Dismiss => {
                            state.search = None;
                            state.window.request_redraw();
                            return;
                        }
                        SearchKeyResult::Pass => {
                            // Escape was not pressed, fall through.
                        }
                    }
                }

                // Quick Terminal: Esc dismisses when no sub-UI is active.
                if state.quick_terminal.active
                    && state.quick_terminal.visible
                    && state.config.quick_terminal.dismiss_on_esc
                    && event.logical_key == Key::Named(NamedKey::Escape)
                    && !state.command_palette.visible
                    && state.command_execution.is_none()
                    && state.search.is_none()
                {
                    hide_quick_terminal(state);
                    return;
                }

                // Try keybinding lookup (all keybindings are now routed through the TOML config).
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

                // Clear selection on any keypress, EXCEPT modifier-only keys
                // (so that Cmd+C can copy without the Cmd press clearing the selection first).
                let is_modifier_only = matches!(
                    event.logical_key,
                    Key::Named(
                        NamedKey::Super
                            | NamedKey::Shift
                            | NamedKey::Control
                            | NamedKey::Alt
                            | NamedKey::Meta
                    )
                );
                if !is_modifier_only {
                    if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                        if pane.selection.is_some() {
                            pane.selection = None;
                            state.window.request_redraw();
                        }
                    }
                }

                // Forward key to PTY.
                if let Some(bytes) = key_to_bytes(&event, state.modifiers) {
                    if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                        let _ = daemon_pty_write(&pane.session_id, &bytes);
                    }
                }
            }

            // IME events.
            WindowEvent::Ime(ime) => {
                // Route IME to command palette/execution when visible.
                // Route IME to Quick Launch when visible.
                if state.quick_launch.visible {
                    match ime {
                        winit::event::Ime::Commit(text) => {
                            if !text.is_empty() {
                                state.quick_launch.input.push_str(&text);
                                state.quick_launch.update_filter();
                            }
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                if state.command_palette.visible {
                    match ime {
                        winit::event::Ime::Commit(text) => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.preedit.clear();
                                if !text.is_empty() {
                                    exec.input.push_str(&text);
                                    exec.filter_items();
                                }
                            } else {
                                state.command_palette.preedit.clear();
                                if !text.is_empty() {
                                    state.command_palette.input.push_str(&text);
                                    state.command_palette.update_filter();
                                }
                            }
                            state.window.request_redraw();
                        }
                        winit::event::Ime::Preedit(text, _cursor) => {
                            if let Some(ref mut exec) = state.command_execution {
                                exec.preedit = text;
                            } else {
                                state.command_palette.preedit = text;
                            }
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                // Allow Flow: intercept y/n/a/Y/N/A from IME commit.
                // When Japanese (or other) IME is active, pressing 'y' or 'n'
                // may arrive only as Ime::Commit instead of KeyboardInput.
                // Intercept these single-char commits before they reach the PTY.
                if let winit::event::Ime::Commit(ref text) = ime {
                    if state.allow_flow.first_workspace_with_pending().is_some() {
                        let allow_flow_key = match text.as_str() {
                            "y" | "n" | "a" | "Y" | "N" | "A" => {
                                Some(Key::Character(text.clone().into()))
                            }
                            _ => None,
                        };
                        if let Some(key) = allow_flow_key {
                            // Clear preedit on the focused pane since we're consuming the commit.
                            let focused_id = active_tab(state).layout.focused();
                            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                                pane.preedit = None;
                            }
                            let active_ws = state.active_workspace;
                            let pane_sessions: std::collections::HashMap<u64, String> = {
                                let mut m = std::collections::HashMap::new();
                                for ws in &state.workspaces {
                                    for tab in &ws.tabs {
                                        for (pid, pane) in &tab.panes {
                                            m.insert(*pid, pane.session_id.clone());
                                        }
                                    }
                                }
                                m
                            };
                            let key_result =
                                state
                                    .allow_flow
                                    .process_key(&key, active_ws, &pane_sessions);
                            match key_result {
                                crate::allow_flow::AllowFlowKeyResult::NotConsumed => {}
                                crate::allow_flow::AllowFlowKeyResult::Consumed => {
                                    state.window.request_redraw();
                                    return;
                                }
                                crate::allow_flow::AllowFlowKeyResult::Resolved(decisions) => {
                                    for (req_id, decision) in &decisions {
                                        if let Some((tx, _alive)) =
                                            state.pending_ipc_responses.remove(req_id)
                                        {
                                            let decision_str = match decision {
                                                termojinal_claude::AllowDecision::Allow => "allow",
                                                termojinal_claude::AllowDecision::Deny => "deny",
                                            };
                                            let _ = tx.send(AppIpcResponse::ok(
                                                serde_json::json!({"decision": decision_str}),
                                            ));
                                        }
                                    }
                                    for wi in 0..state.agent_infos.len() {
                                        if state.agent_infos[wi].active
                                            && matches!(
                                                state.agent_infos[wi].state,
                                                AgentState::WaitingForPermission
                                            )
                                            && !state.allow_flow.has_pending_for_workspace(wi)
                                        {
                                            state.agent_infos[wi].state = AgentState::Running;
                                            state.agent_infos[wi].last_updated =
                                                std::time::Instant::now();
                                        }
                                    }
                                    state.window.request_redraw();
                                    return;
                                }
                            }
                        }
                    }
                }

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
                                let _ = daemon_pty_write(&pane.session_id, text.as_bytes());
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

                // --- Sidebar drag active: update sidebar width (Feature 1) ---
                if state.sidebar_drag {
                    let new_width = (position.x as f32).clamp(
                        state.config.sidebar.min_width,
                        state.config.sidebar.max_width,
                    );
                    state.sidebar_width = new_width;
                    resize_all_panes(state);
                    state.window.request_redraw();
                    return;
                }

                // --- Scrollbar drag active: update scroll position ---
                if let Some(ref sb_drag) = state.scrollbar_drag {
                    let pane_id = sb_drag.pane_id;
                    let grab_offset = sb_drag.grab_offset_px;
                    let pane_rect = sb_drag.pane_rect;
                    let local_y = (position.y as f32) - pane_rect.y;
                    // First, compute geometry immutably.
                    let new_scroll = {
                        let tab = active_tab(state);
                        tab.panes.get(&pane_id).and_then(|pane| {
                            state
                                .renderer
                                .scrollbar_geometry(&pane.terminal)
                                .map(|geo| {
                                    let desired_thumb_top = local_y - grab_offset;
                                    let total_lines = geo.scrollback_len + geo.rows;
                                    let frac =
                                        (desired_thumb_top / geo.total_height).clamp(0.0, 1.0);
                                    let new_offset =
                                        geo.scrollback_len as f32 - frac * total_lines as f32;
                                    (new_offset.round() as isize)
                                        .clamp(0, geo.scrollback_len as isize)
                                        as usize
                                })
                        })
                    };
                    // Then, apply the scroll offset mutably.
                    if let Some(offset) = new_scroll {
                        let tab = active_tab_mut(state);
                        if let Some(pane) = tab.panes.get_mut(&pane_id) {
                            pane.terminal.set_scroll_offset(offset);
                        }
                    }
                    state.window.request_redraw();
                    return;
                }

                // --- Pending tab click: check if mouse moved beyond drag threshold ---
                if let Some(ref pending) = state.pending_tab_click {
                    let dx = position.x - pending.start_x;
                    let dy = position.y - pending.start_y;
                    if dx * dx + dy * dy > 25.0 {
                        // Threshold exceeded (5px) — promote to tab drag.
                        let tab_idx = pending.tab_idx;
                        let start_x = pending.start_x;
                        state.pending_tab_click = None;
                        state.tab_drag = Some(TabDrag {
                            tab_idx,
                            start_x,
                        });
                    }
                    // While pending, consume the move event (don't start selection etc.).
                    state.window.request_redraw();
                    return;
                }

                // --- Tab-to-pane drag active: update drop zone preview ---
                if state.tab_pane_drag.is_some() {
                    let cx = position.x as f32;
                    let cy = position.y as f32;
                    let pane_rects = active_pane_rects(state);
                    // Find which pane the cursor is over.
                    let mut found = false;
                    for (pid, rect) in &pane_rects {
                        if cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            let zone = compute_drop_zone(cx, cy, &rect);
                            let drag_tab_idx = state.tab_pane_drag.as_ref().map(|d| d.tab_idx).unwrap_or(0);
                            state.tab_pane_drag = Some(TabPaneDrag {
                                tab_idx: drag_tab_idx,
                                target_pane: *pid,
                                zone,
                            });
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        // Cursor is outside all panes — check if back in tab bar.
                        let tab_bar_h = if tab_bar_visible(state) {
                            state.config.tab_bar.height
                        } else {
                            0.0
                        };
                        if cy < tab_bar_h {
                            // Re-enter tab bar reorder mode.
                            let drag_tab_idx = state.tab_pane_drag.as_ref().map(|d| d.tab_idx).unwrap_or(0);
                            state.tab_drag = Some(TabDrag {
                                tab_idx: drag_tab_idx,
                                start_x: position.x,
                            });
                            state.tab_pane_drag = None;
                        }
                    }
                    state.window.request_redraw();
                    return;
                }

                // --- Tab drag active: check for reordering or transition to pane split ---
                if state.tab_drag.is_some() {
                    let (drag_idx, drag_start_x) = match state.tab_drag.as_ref() {
                        Some(d) => (d.tab_idx, d.start_x),
                        None => { return; }
                    };
                    let cx = position.x as f32;
                    let cy = position.y as f32;
                    let tab_bar_h = if tab_bar_visible(state) {
                        state.config.tab_bar.height
                    } else {
                        0.0
                    };

                    // If cursor has moved below the tab bar, transition to pane split mode.
                    // Only allow this when there are at least 2 tabs (last tab cannot be dragged out).
                    if cy >= tab_bar_h && active_ws(state).tabs.len() > 1 {
                        state.tab_drag = None;
                        // Find the pane under the cursor and compute the drop zone.
                        let pane_rects = active_pane_rects(state);
                        for (pid, rect) in &pane_rects {
                            if cx >= rect.x
                                && cx < rect.x + rect.w
                                && cy >= rect.y
                                && cy < rect.y + rect.h
                            {
                                let zone = compute_drop_zone(cx, cy, rect);
                                state.tab_pane_drag = Some(TabPaneDrag {
                                    tab_idx: drag_idx,
                                    target_pane: *pid,
                                    zone,
                                });
                                break;
                            }
                        }
                        state.window.request_redraw();
                        return;
                    }

                    let sidebar_w = if state.sidebar_visible {
                        state.sidebar_width
                    } else {
                        0.0
                    };
                    let local_cx = cx - sidebar_w;
                    let cell_w = state.renderer.cell_size().width;
                    let max_tab_w = state.config.tab_bar.max_width;
                    let ws = active_ws(state);
                    // Determine which tab position the cursor is over.
                    let mut tab_x: f32 = 0.0;
                    let mut target_idx = drag_idx;
                    for (i, tab) in ws.tabs.iter().enumerate() {
                        let tab_w = compute_tab_width(
                            &tab.display_title,
                            cell_w,
                            max_tab_w,
                            state.config.tab_bar.min_tab_width,
                        );
                        if local_cx >= tab_x && local_cx < tab_x + tab_w {
                            target_idx = i;
                            break;
                        }
                        tab_x += tab_w;
                    }
                    if target_idx != drag_idx {
                        let ws = active_ws_mut(state);
                        let tab = ws.tabs.remove(drag_idx);
                        ws.tabs.insert(target_idx, tab);
                        if ws.active_tab == drag_idx {
                            ws.active_tab = target_idx;
                        } else if drag_idx < ws.active_tab && target_idx >= ws.active_tab {
                            ws.active_tab -= 1;
                        } else if drag_idx > ws.active_tab && target_idx <= ws.active_tab {
                            ws.active_tab += 1;
                        }
                        state.tab_drag = Some(TabDrag {
                            tab_idx: target_idx,
                            start_x: drag_start_x,
                        });
                        state.window.request_redraw();
                    }
                    return;
                }

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
                    let sidebar_w = if state.sidebar_visible {
                        state.sidebar_width
                    } else {
                        0.0
                    };
                    let ws = &state.workspaces[state.active_workspace];
                    let show_tab_bar = state.config.tab_bar.always_show || ws.tabs.len() > 1;
                    let tab_bar_h = if show_tab_bar {
                        state.config.tab_bar.height
                    } else {
                        0.0
                    };
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
                    // --- Cursor icon management: show resize cursor near separators or sidebar edge ---
                    let pane_rects = active_pane_rects(state);
                    let mx = position.x as f32;
                    let my = position.y as f32;

                    // Check sidebar edge first (Feature 1).
                    let sep_tol = state.config.pane.separator_tolerance;
                    let near_sidebar_edge =
                        state.sidebar_visible && (mx - state.sidebar_width).abs() < sep_tol;

                    if near_sidebar_edge {
                        state.window.set_cursor(CursorIcon::ColResize);
                    } else if let Some((dir, _)) = find_separator(&pane_rects, mx, my, 4.0) {
                        let icon = match dir {
                            SplitDirection::Horizontal => CursorIcon::ColResize,
                            SplitDirection::Vertical => CursorIcon::RowResize,
                        };
                        state.window.set_cursor(icon);
                    } else {
                        // Context-aware cursor: sidebar → pointer, tab bar → pointer/hand, pane → text
                        let sidebar_w = if state.sidebar_visible {
                            state.sidebar_width
                        } else {
                            0.0
                        };
                        let tab_h = if tab_bar_visible(state) {
                            state.config.tab_bar.height
                        } else {
                            0.0
                        };
                        if mx < sidebar_w {
                            state.window.set_cursor(CursorIcon::Pointer);
                        } else if my < tab_h {
                            // In tab bar: hand on close buttons, pointer elsewhere
                            let cursor = tab_bar_cursor(state, mx, my);
                            state.window.set_cursor(cursor);
                        } else {
                            // Check if cursor is over a scrollbar in any pane.
                            let mut on_scrollbar = false;
                            for (pid, rect) in &pane_rects {
                                if mx >= rect.x
                                    && mx < rect.x + rect.w
                                    && my >= rect.y
                                    && my < rect.y + rect.h
                                {
                                    let tab = active_tab(state);
                                    if let Some(pane) = tab.panes.get(pid) {
                                        if let Some(geo) =
                                            state.renderer.scrollbar_geometry(&pane.terminal)
                                        {
                                            let local_x = mx - rect.x;
                                            if local_x >= geo.track_x {
                                                on_scrollbar = true;
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                            if on_scrollbar {
                                state.window.set_cursor(CursorIcon::Default);
                            } else {
                                state.window.set_cursor(CursorIcon::Text);
                            }
                        }
                    }

                    // --- Original mouse handling (motion reporting / selection) ---
                    let focused_id = active_tab(state).layout.focused();
                    let cell_size = state.renderer.cell_size();
                    let mut sel_auto_scroll: Option<Option<i32>> = None;
                    let tab = active_tab_mut(state);
                    if let Some(pane) = tab.panes.get_mut(&focused_id) {
                        // Handle mouse motion reporting for the terminal.
                        if pane.terminal.modes.mouse_mode == MouseMode::AnyMotion
                            || (pane.terminal.modes.mouse_mode == MouseMode::ButtonMotion
                                && pane.selection.as_ref().map_or(false, |s| s.active))
                        {
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                // Subtract in f64 to avoid rounding the cursor
                                // position before the subtraction, then floor to
                                // get the correct column/row index.
                                let local_x = ((position.x - rect.x as f64) as f32).max(0.0);
                                let local_y = ((position.y - rect.y as f64) as f32).max(0.0);
                                let col = (local_x / cell_size.width).floor() as usize;
                                let row = (local_y / cell_size.height).floor() as usize;
                                // Motion event: button 32 + 0 = 32.
                                let seq = encode_mouse_sgr(32, col, row, true);
                                let _ = daemon_pty_write(&pane.session_id, &seq);
                            }
                        } else if let Some(ref mut sel) = pane.selection {
                            // Non-mouse-mode selection dragging.
                            if sel.active {
                                if let Some((_, rect)) =
                                    pane_rects.iter().find(|(id, _)| *id == focused_id)
                                {
                                    let local_x = ((position.x - rect.x as f64) as f32).max(0.0);
                                    let local_y = (position.y - rect.y as f64) as f32;
                                    let pane_h = rect.h;
                                    let so = pane.terminal.scroll_offset();

                                    // Auto-scroll when cursor is above or below the pane.
                                    sel_auto_scroll = Some(if local_y < 0.0 {
                                        let speed =
                                            ((-local_y / cell_size.height).ceil() as i32).max(1);
                                        Some(speed)
                                    } else if local_y > pane_h {
                                        let speed = (((local_y - pane_h) / cell_size.height).ceil()
                                            as i32)
                                            .max(1);
                                        Some(-speed)
                                    } else {
                                        None
                                    });

                                    let clamped_y = local_y.clamp(0.0, pane_h - 1.0);
                                    sel.end = GridPos {
                                        col: (local_x / cell_size.width).floor() as usize,
                                        row: (clamped_y / cell_size.height).floor() as usize,
                                    };
                                    sel.scroll_offset_at_end = so;
                                }
                            }
                        }
                    }
                    // Apply auto-scroll state after releasing the pane borrow.
                    if let Some(auto) = sel_auto_scroll {
                        state.selection_auto_scroll = auto;
                        if auto.is_some() {
                            state.needs_animation_frame = true;
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
                // --- Handle drag-resize / sidebar-drag / tab-drag / scrollbar-drag release ---
                if btn_state == ElementState::Released && button == MouseButton::Left {
                    // Clear auto-scroll on any mouse release.
                    state.selection_auto_scroll = None;

                    if state.sidebar_drag {
                        state.sidebar_drag = false;
                        state.window.set_cursor(CursorIcon::Default);
                        break 'mouse_input;
                    }
                    if state.scrollbar_drag.is_some() {
                        state.scrollbar_drag = None;
                        state.window.request_redraw();
                        break 'mouse_input;
                    }
                    if let Some(drag) = state.tab_pane_drag.take() {
                        // Execute the tab-to-pane split.
                        let ws = active_ws(state);
                        // Ensure the source tab still exists and is not the only tab.
                        if drag.tab_idx < ws.tabs.len() && ws.tabs.len() > 1 {
                            let (direction, insert_first) = match drag.zone {
                                DropZone::Left => (SplitDirection::Horizontal, true),
                                DropZone::Right => (SplitDirection::Horizontal, false),
                                DropZone::Top => (SplitDirection::Vertical, true),
                                DropZone::Bottom => (SplitDirection::Vertical, false),
                            };
                            // Take all panes from the dragged tab.
                            let ws = active_ws_mut(state);
                            let source_tab = ws.tabs.remove(drag.tab_idx);
                            // Fix active_tab index after removal.
                            if ws.active_tab >= ws.tabs.len() {
                                ws.active_tab = ws.tabs.len().saturating_sub(1);
                            } else if ws.active_tab > drag.tab_idx {
                                ws.active_tab -= 1;
                            }
                            // Insert each pane from the source tab into the target pane's layout.
                            let pane_ids: Vec<PaneId> = source_tab.layout.pane_ids();
                            let mut first_inserted = None;
                            for pid in &pane_ids {
                                let tab = active_tab_mut(state);
                                let anchor = first_inserted.unwrap_or(drag.target_pane);
                                tab.layout = tab.layout.split_insert(
                                    anchor,
                                    direction,
                                    *pid,
                                    insert_first,
                                );
                                if first_inserted.is_none() {
                                    first_inserted = Some(*pid);
                                }
                            }
                            // Move panes from source tab into the active tab's pane map.
                            for (pid, pane) in source_tab.panes {
                                active_tab_mut(state).panes.insert(pid, pane);
                            }
                            // Focus the first inserted pane.
                            if let Some(fid) = first_inserted {
                                let tab = active_tab_mut(state);
                                tab.layout = tab.layout.focus(fid);
                            }
                            resize_all_panes(state);
                        }
                        state.window.request_redraw();
                        break 'mouse_input;
                    }
                    if let Some(pending) = state.pending_tab_click.take() {
                        // Mouse released within drag threshold — treat as a click (tab switch).
                        let ws = active_ws_mut(state);
                        if ws.active_tab != pending.tab_idx {
                            ws.active_tab = pending.tab_idx;
                            resize_all_panes(state);
                            update_window_title(state);
                        }
                        state.window.request_redraw();
                        break 'mouse_input;
                    }
                    if state.tab_drag.is_some() {
                        state.tab_drag = None;
                        state.window.request_redraw();
                        break 'mouse_input;
                    }
                    if state.drag_resize.is_some() {
                        state.drag_resize = None;
                        state.window.set_cursor(CursorIcon::Default);
                        break 'mouse_input;
                    }
                }

                // --- Priority 0: Check if click is on sidebar right edge for DnD resize (Feature 1) ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.sidebar_visible
                {
                    let cx = state.cursor_pos.0 as f32;
                    if (cx - state.sidebar_width).abs() < state.config.pane.separator_tolerance {
                        state.sidebar_drag = true;
                        state.window.set_cursor(CursorIcon::ColResize);
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
                        if let Some(action) = handle_sidebar_click(state) {
                            let proxy = &self.proxy;
                            let buffers = &self.pty_buffers;
                            dispatch_action(state, &action, proxy, buffers, event_loop);
                        }
                        // Handle deferred "open in editor" from double-click on file.
                        let wi = state.active_workspace;
                        if wi < state.dir_trees.len() && state.dir_trees[wi].pending_open_in_editor
                        {
                            state.dir_trees[wi].pending_open_in_editor = false;
                            dir_tree_open_in_editor(state, &self.proxy, &self.pty_buffers);
                        }
                        break 'mouse_input;
                    }
                }

                // --- Unfocus file tree on click outside sidebar ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let wi = state.active_workspace;
                    if wi < state.dir_trees.len() && state.dir_trees[wi].focused {
                        let cx = state.cursor_pos.0 as f32;
                        let sidebar_w = if state.sidebar_visible {
                            state.sidebar_width
                        } else {
                            0.0
                        };
                        if cx >= sidebar_w {
                            state.dir_trees[wi].focused = false;
                            state.dir_trees[wi].find_active = false;
                            state.dir_trees[wi].find_query.clear();
                            state.window.request_redraw();
                        }
                    }
                }

                // --- Priority 0.5: Check if click is in the tab bar area (Feature 4: tab drag) ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let tab_bar_h = if tab_bar_visible(state) {
                        state.config.tab_bar.height
                    } else {
                        0.0
                    };
                    let cy = state.cursor_pos.1 as f32;
                    if tab_bar_h > 0.0 && cy < tab_bar_h {
                        match handle_tab_bar_click(state) {
                            TabBarClickResult::Tab(tab_idx) => {
                                state.pending_tab_click = Some(PendingTabClick {
                                    tab_idx,
                                    start_x: state.cursor_pos.0,
                                    start_y: state.cursor_pos.1,
                                });
                            }
                            TabBarClickResult::CloseTab(tab_idx) => {
                                // Close the clicked tab.
                                let ws = active_ws_mut(state);
                                if ws.tabs.len() > 1 {
                                    ws.tabs.remove(tab_idx);
                                    if ws.active_tab >= ws.tabs.len() {
                                        ws.active_tab = ws.tabs.len() - 1;
                                    } else if ws.active_tab > tab_idx {
                                        ws.active_tab -= 1;
                                    }
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                } else {
                                    // Last tab — close focused pane instead.
                                    close_focused_pane(state, &self.pty_buffers, event_loop);
                                }
                            }
                            TabBarClickResult::NewTab => {
                                dispatch_action(
                                    state,
                                    &Action::NewTab,
                                    &self.proxy,
                                    &self.pty_buffers,
                                    event_loop,
                                );
                            }
                            TabBarClickResult::None => {}
                        }
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

                // --- Priority 1.2: Check if clicking on a scrollbar → start drag or page scroll ---
                if btn_state == ElementState::Pressed && button == MouseButton::Left {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    let mut handled = false;
                    for (pid, rect) in &pane_rects {
                        if cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            let tab = active_tab(state);
                            if let Some(pane) = tab.panes.get(pid) {
                                if let Some(geo) = state.renderer.scrollbar_geometry(&pane.terminal)
                                {
                                    let local_x = cx - rect.x;
                                    let local_y = cy - rect.y;
                                    if local_x >= geo.track_x {
                                        // Click is in the scrollbar area.
                                        if local_y >= geo.thumb_top && local_y < geo.thumb_bottom {
                                            // Clicked on the thumb — start dragging.
                                            state.scrollbar_drag = Some(ScrollbarDrag {
                                                pane_id: *pid,
                                                grab_offset_px: local_y - geo.thumb_top,
                                                pane_rect: *rect,
                                            });
                                        } else {
                                            // Clicked above or below the thumb — jump thumb
                                            // to the click position and start dragging.
                                            let thumb_h = geo.thumb_bottom - geo.thumb_top;
                                            let total_lines = geo.scrollback_len + geo.rows;
                                            let desired_thumb_top = local_y - thumb_h / 2.0;
                                            let frac = (desired_thumb_top / geo.total_height)
                                                .clamp(0.0, 1.0);
                                            let new_offset = geo.scrollback_len as f32
                                                - frac * total_lines as f32;
                                            let new_offset = (new_offset.round() as isize)
                                                .clamp(0, geo.scrollback_len as isize)
                                                as usize;
                                            let tab = active_tab_mut(state);
                                            if let Some(pane) = tab.panes.get_mut(pid) {
                                                pane.terminal.set_scroll_offset(new_offset);
                                            }
                                            // Start drag from the new position.
                                            state.scrollbar_drag = Some(ScrollbarDrag {
                                                pane_id: *pid,
                                                grab_offset_px: thumb_h / 2.0,
                                                pane_rect: *rect,
                                            });
                                        }
                                        handled = true;
                                        state.window.request_redraw();
                                    }
                                }
                            }
                            break;
                        }
                    }
                    if handled {
                        break 'mouse_input;
                    }
                }

                // --- Priority 1.5: Cmd+click on a pane → extract to new tab (Feature 4) ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.modifiers.super_key()
                {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    let tab = active_tab(state);
                    // Only allow extraction if there are multiple panes.
                    if tab.layout.pane_count() > 1 {
                        for (pid, rect) in &pane_rects {
                            if cx >= rect.x
                                && cx < rect.x + rect.w
                                && cy >= rect.y
                                && cy < rect.y + rect.h
                            {
                                let target_pane = *pid;
                                let tab = active_tab(state);
                                if let Some((remaining, _extracted)) =
                                    tab.layout.extract_pane(target_pane)
                                {
                                    // Remove the pane from current tab and create a new tab with it.
                                    let pane = active_tab_mut(state).panes.remove(&target_pane);
                                    active_tab_mut(state).layout = remaining;

                                    if let Some(pane) = pane {
                                        let new_layout = LayoutTree::new(target_pane);
                                        let mut new_panes = HashMap::new();
                                        new_panes.insert(target_pane, pane);
                                        let fmt = state.config.tab_bar.format.clone();
                                        let fb_cwd = state.pane_git_cache.cwd.clone();
                                        let ws = active_ws_mut(state);
                                        let tab_num = ws.tabs.len() + 1;
                                        let new_tab = Tab {
                                            layout: new_layout,
                                            panes: new_panes,
                                            name: format!("Tab {tab_num}"),
                                            display_title: format_tab_title(&fmt, "", "", tab_num),
                                        };
                                        ws.tabs.push(new_tab);
                                        let new_tab_idx = ws.tabs.len() - 1;
                                        ws.active_tab = new_tab_idx;
                                        update_tab_title(
                                            &mut ws.tabs[new_tab_idx],
                                            &fmt,
                                            tab_num,
                                            &fb_cwd,
                                        );
                                    }
                                    resize_all_panes(state);
                                    state.window.request_redraw();
                                    break 'mouse_input;
                                }
                                break;
                            }
                        }
                    }
                }

                // --- Priority 1.7: Option+click → open URL or file path at click position ---
                if btn_state == ElementState::Pressed
                    && button == MouseButton::Left
                    && state.modifiers.alt_key()
                    && !state.modifiers.super_key()
                {
                    let pane_rects = active_pane_rects(state);
                    let cx = state.cursor_pos.0 as f32;
                    let cy = state.cursor_pos.1 as f32;
                    let cell_size = state.renderer.cell_size();
                    let tab = active_tab(state);
                    let focused_id = tab.layout.focused();
                    if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
                        if cx >= rect.x
                            && cx < rect.x + rect.w
                            && cy >= rect.y
                            && cy < rect.y + rect.h
                        {
                            let local_x = ((cx - rect.x) as f32).max(0.0);
                            let local_y = ((cy - rect.y) as f32).max(0.0);
                            let click_col = (local_x / cell_size.width).floor() as usize;
                            let click_row = (local_y / cell_size.height).floor() as usize;
                            if let Some(pane) = tab.panes.get(&focused_id) {
                                if let Some(target) =
                                    extract_clickable_target(&pane.terminal, click_row, click_col)
                                {
                                    log::info!("Option+click: opening {target}");
                                    let _ = std::process::Command::new("open").arg(&target).spawn();
                                    break 'mouse_input;
                                }
                            }
                        }
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
                            update_window_title(state);
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
                        if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            // Subtract in f64 to avoid rounding before subtraction.
                            let local_x = ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                            let local_y = ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                            let col = (local_x / cell_size.width).floor() as usize;
                            let row = (local_y / cell_size.height).floor() as usize;
                            let pressed = btn_state == ElementState::Pressed;
                            let seq = encode_mouse_sgr(btn_code, col, row, pressed);
                            let _ = daemon_pty_write(&pane.session_id, &seq);
                        }
                    } else {
                        // Selection mode.
                        if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id)
                        {
                            // cursor_pos is in physical pixels (from CursorMoved PhysicalPosition).
                            // rect and cell_size are also in physical pixels.
                            let local_x = ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                            let local_y = ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                            let pos = GridPos {
                                col: (local_x / cell_size.width).floor() as usize,
                                row: (local_y / cell_size.height).floor() as usize,
                            };

                            if button == MouseButton::Left {
                                match btn_state {
                                    ElementState::Pressed => {
                                        let so = pane.terminal.scroll_offset();
                                        pane.selection = Some(Selection {
                                            start: pos,
                                            end: pos,
                                            active: true,
                                            scroll_offset_at_start: so,
                                            scroll_offset_at_end: so,
                                        });
                                    }
                                    ElementState::Released => {
                                        if let Some(ref mut sel) = pane.selection {
                                            sel.active = false;
                                            let so = pane.terminal.scroll_offset();
                                            sel.end = pos;
                                            sel.scroll_offset_at_end = so;
                                            if sel.is_empty() {
                                                pane.selection = None;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Right-click: show native context menu.
                        #[cfg(target_os = "macos")]
                        if button == MouseButton::Right && btn_state == ElementState::Pressed {
                            let has_sel = pane.selection.is_some();
                            if let Some(action) = show_context_menu(&state.window, has_sel) {
                                let mapped = match action {
                                    ContextMenuAction::Copy => Action::Copy,
                                    ContextMenuAction::Paste => Action::Paste,
                                    ContextMenuAction::SelectAll => Action::SelectAll,
                                    ContextMenuAction::Clear => Action::ClearScrollback,
                                    ContextMenuAction::SplitRight => Action::SplitRight,
                                    ContextMenuAction::SplitDown => Action::SplitDown,
                                };
                                dispatch_action(
                                    state,
                                    &mapped,
                                    &self.proxy,
                                    &self.pty_buffers,
                                    event_loop,
                                );
                            }
                        }
                    }
                }
                state.window.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // --- File tree mouse scroll ---
                // If cursor is over sidebar and directory tree is visible, scroll the tree.
                let sidebar_w = if state.sidebar_visible {
                    state.sidebar_width
                } else {
                    0.0
                };
                if state.cursor_pos.0 < sidebar_w as f64 {
                    let wi = state.active_workspace;
                    if wi < state.dir_trees.len()
                        && state.dir_trees[wi].visible
                        && !state.dir_trees[wi].entries.is_empty()
                    {
                        let cell_h_f = state.renderer.cell_size().height as f64;
                        let scroll_lines = match delta {
                            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                (pos.y / cell_h_f).round() as i32
                            }
                        };
                        if scroll_lines != 0 {
                            let tree = &mut state.dir_trees[wi];
                            let max_visible = if tree.current_visible_lines > 0 {
                                tree.current_visible_lines
                            } else {
                                state.config.directory_tree.max_visible_lines.max(1)
                            };
                            if scroll_lines > 0 {
                                // Scroll up.
                                tree.scroll_offset =
                                    tree.scroll_offset.saturating_sub(scroll_lines as usize);
                            } else {
                                // Scroll down.
                                let max_offset = tree.entries.len().saturating_sub(max_visible);
                                tree.scroll_offset = (tree.scroll_offset
                                    + scroll_lines.unsigned_abs() as usize)
                                    .min(max_offset);
                            }
                            state.window.request_redraw();
                            return;
                        }
                    }
                }

                let focused_id = active_tab(state).layout.focused();
                let cell_size = state.renderer.cell_size();
                let cell_h = cell_size.height as f64;
                let cursor_pos = state.cursor_pos;
                let pane_rects = active_pane_rects(state);
                // Accumulate fractional pixel deltas from trackpad scrolling.
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        state.scroll_accum = 0.0; // reset on line-based scroll
                        y as i32
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        state.scroll_accum += pos.y;
                        let whole_lines = (state.scroll_accum / cell_h).trunc() as i32;
                        if whole_lines != 0 {
                            state.scroll_accum -= whole_lines as f64 * cell_h;
                        }
                        whole_lines
                    }
                };
                let tab = active_tab_mut(state);
                if let Some(pane) = tab.panes.get_mut(&focused_id) {
                    if pane.terminal.modes.mouse_mode != MouseMode::None {
                        // Forward scroll as mouse events.
                        // Scroll up = button 64, scroll down = button 65.
                        if lines != 0 {
                            if let Some((_, rect)) =
                                pane_rects.iter().find(|(id, _)| *id == focused_id)
                            {
                                let local_x = ((cursor_pos.0 - rect.x as f64) as f32).max(0.0);
                                let local_y = ((cursor_pos.1 - rect.y as f64) as f32).max(0.0);
                                let col = (local_x / cell_size.width).floor() as usize;
                                let row = (local_y / cell_size.height).floor() as usize;
                                let count = lines.unsigned_abs();
                                let btn = if lines > 0 { 64u8 } else { 65u8 };
                                for _ in 0..count {
                                    let seq = encode_mouse_sgr(btn, col, row, true);
                                    let _ = daemon_pty_write(&pane.session_id, &seq);
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

            WindowEvent::ThemeChanged(_winit_theme) => {
                // macOS appearance changed (Dark <-> Light).
                if state.config.theme.auto_switch {
                    let new_appearance = appearance::detect_macos_appearance();
                    let effective_theme = resolve_theme(&state.config, new_appearance);
                    let palette = ThemePalette::from_theme_colors(
                        &effective_theme.background,
                        &effective_theme.foreground,
                        &effective_theme.black,
                        &effective_theme.red,
                        &effective_theme.green,
                        &effective_theme.yellow,
                        &effective_theme.blue,
                        &effective_theme.magenta,
                        &effective_theme.cyan,
                        &effective_theme.white,
                        &effective_theme.bright_black,
                        &effective_theme.bright_red,
                        &effective_theme.bright_green,
                        &effective_theme.bright_yellow,
                        &effective_theme.bright_blue,
                        &effective_theme.bright_magenta,
                        &effective_theme.bright_cyan,
                        &effective_theme.bright_white,
                    );
                    state.renderer.set_theme(palette);
                    state.renderer.default_bg =
                        color_or(&effective_theme.background, [0.067, 0.067, 0.09, 1.0]);
                    state.renderer.preedit_bg =
                        color_or(&effective_theme.preedit_bg, [0.15, 0.15, 0.20, 1.0]);
                    log::info!("theme switched to {:?}", new_appearance);
                    state.window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                // Reset animation flag — set below if continuous frames are needed.
                state.needs_animation_frame = false;

                // Auto-scroll during drag selection.
                if let Some(scroll_dir) = state.selection_auto_scroll {
                    let focused_id = active_tab(state).layout.focused();
                    let tab = active_tab_mut(state);
                    if let Some(pane) = tab.panes.get_mut(&focused_id) {
                        let current = pane.terminal.scroll_offset() as i32;
                        let new_offset = (current + scroll_dir).max(0) as usize;
                        pane.terminal.set_scroll_offset(new_offset);
                        // Update selection endpoint to match new scroll position.
                        if let Some(ref mut sel) = pane.selection {
                            if sel.active {
                                let rows = pane.terminal.rows();
                                sel.scroll_offset_at_end = new_offset;
                                if scroll_dir > 0 {
                                    // Scrolling into scrollback — select top row.
                                    sel.end.row = 0;
                                } else {
                                    // Scrolling toward live — select bottom row.
                                    sel.end.row = rows.saturating_sub(1);
                                }
                            }
                        }
                    }
                    state.needs_animation_frame = true;
                }

                // Poll active command execution for new messages.
                if let Some(ref mut exec) = state.command_execution {
                    if exec.poll() {
                        // State changed; check if the command completed.
                        if exec.is_done() {
                            // Send macOS desktop notification on Done.
                            if let CommandUIState::Done(Some(ref msg)) = exec.ui_state {
                                log::info!("command '{}' done: {}", exec.command_name, msg);
                                if state.config.notifications.enabled {
                                    notification::send_notification(
                                        "termojinal",
                                        msg,
                                        state.config.notifications.sound,
                                    );
                                }
                            }
                        }
                    }
                    // Schedule a timer-based redraw (not immediate) to keep polling.
                    state.needs_animation_frame = true;
                }

                // Quick Terminal animation tick.
                if tick_quick_terminal_animation(state) {
                    state.window.request_redraw();
                }

                // Agent pulse animation: schedule timer-based redraws when any
                // workspace has an active agent with pulse indicator style.
                if state.sidebar_visible
                    && state.config.sidebar.agent_status_enabled
                    && state.config.sidebar.agent_indicator_style == "pulse"
                    && state.agent_infos.iter().any(|a| a.active)
                {
                    state.needs_animation_frame = true;
                }

                if let Err(e) = render_frame(state) {
                    log::error!("render error: {e}");
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            if state.needs_animation_frame {
                let now = std::time::Instant::now();
                let interval = std::time::Duration::from_millis(33); // ~30fps
                let elapsed = now.duration_since(state.last_animation_redraw);
                if elapsed >= interval {
                    // Enough time has passed — request a new frame.
                    state.last_animation_redraw = now;
                    state.window.request_redraw();
                } else {
                    // Not yet time — schedule a wake-up for the remaining duration.
                    let next = state.last_animation_redraw + interval;
                    event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next));
                }
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput(pane_id) => {
                let Some(state) = &mut self.state else {
                    return;
                };
                // Find the pane across all workspaces and tabs.
                let cell_size = state.renderer.cell_size();
                let cell_w = cell_size.width as u32;
                let cell_h = cell_size.height as u32;
                let mut lock = self.pty_buffers.lock().unwrap_or_else(|e| e.into_inner());
                let mut total = 0usize;
                let mut found_ws_idx: Option<usize> = None;
                if let Some(q) = lock.get_mut(&pane_id) {
                    'outer_feed: for (wi, ws) in state.workspaces.iter_mut().enumerate() {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
                                // Sync cell size so image protocols compute
                                // correct cell spans before processing data.
                                pane.terminal.image_store.set_cell_size(cell_w, cell_h);
                                while let Some(data) = q.pop_front() {
                                    total += data.len();
                                    pane.terminal.feed(&mut pane.vt_parser, &data);
                                }
                                found_ws_idx = Some(wi);
                                break 'outer_feed;
                            }
                        }
                    }
                }
                drop(lock);

                // Mark non-active workspaces as having unread output.
                if let Some(wi) = found_ws_idx {
                    if total > 0 && wi != state.active_workspace {
                        if wi < state.workspace_infos.len() {
                            state.workspace_infos[wi].has_unread = true;
                        }
                    }
                }

                // Check if the focused pane's CWD changed (via OSC 7) and
                // update the directory tree root if needed.  This avoids calling
                // update_tree_root_for_focused_pane on every render frame.
                if let Some(wi) = found_ws_idx {
                    if wi == state.active_workspace
                        && wi < state.dir_trees.len()
                        && state.dir_trees[wi].visible
                    {
                        let focused_id = active_tab(state).layout.focused();
                        let osc_cwd = active_tab(state)
                            .panes
                            .get(&focused_id)
                            .map(|p| p.terminal.osc.cwd.clone())
                            .unwrap_or_default();
                        if !osc_cwd.is_empty() && osc_cwd != state.dir_trees[wi].last_resolved_cwd {
                            update_tree_root_for_focused_pane(state);
                        }
                    }
                }

                // Handle OSC 52 clipboard events.
                'outer_clip: for ws in &mut state.workspaces {
                    for tab in &mut ws.tabs {
                        if let Some(pane) = tab.panes.get_mut(&pane_id) {
                            if let Some(ref clipboard_event) = pane.terminal.clipboard_event.take()
                            {
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
                                                let _ = daemon_pty_write(
                                                    &pane.session_id,
                                                    response.as_bytes(),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            break 'outer_clip;
                        }
                    }
                }

                // Drain pending PTY responses (DSR, DA, OSC 10/11/12 queries).
                'outer_resp: for ws in &mut state.workspaces {
                    for tab in &mut ws.tabs {
                        if let Some(pane) = tab.panes.get_mut(&pane_id) {
                            if pane.terminal.has_pending_responses() {
                                for response_data in pane.terminal.drain_responses() {
                                    daemon_pty_write(&pane.session_id, &response_data);
                                }
                            }
                            break 'outer_resp;
                        }
                    }
                }

                // ---------------------------------------------------------------
                // Allow Flow: check for OSC notifications and scan output
                // ---------------------------------------------------------------
                if total > 0 {
                    // Check for Allow Flow notifications via OSC 9/99/777.
                    let ws_idx = found_ws_idx.unwrap_or(state.active_workspace);
                    let mut has_notification = false;
                    'outer_osc: for ws in &mut state.workspaces {
                        for tab in &mut ws.tabs {
                            if let Some(pane) = tab.panes.get_mut(&pane_id) {
                                if let Some(notification) =
                                    pane.terminal.osc.last_notification.take()
                                {
                                    // Send desktop notification if enabled and window not focused.
                                    if state.config.notifications.enabled
                                        && !state.window.has_focus()
                                    {
                                        notification::send_notification(
                                            "termojinal",
                                            &notification,
                                            state.config.notifications.sound,
                                        );
                                    }
                                    if let Some(_req) = state.allow_flow.engine.process_osc(
                                        pane_id,
                                        ws_idx,
                                        &notification,
                                    ) {
                                        state.allow_flow.pane_hint_visible = true;
                                        has_notification = true;
                                    }
                                }
                                break 'outer_osc;
                            }
                        }
                    }

                    // Scan visible lines for Allow prompts (only on new data).
                    if !has_notification {
                        'outer_scan: for ws in &state.workspaces {
                            for tab in &ws.tabs {
                                if let Some(pane) = tab.panes.get(&pane_id) {
                                    let grid = pane.terminal.grid();
                                    let scan_rows = grid.rows().min(5);
                                    let visible_lines: Vec<String> = (0..scan_rows)
                                        .rev()
                                        .map(|row_idx| {
                                            (0..grid.cols())
                                                .map(|col| {
                                                    let c = grid.cell(col, row_idx).c;
                                                    if c == '\0' {
                                                        ' '
                                                    } else {
                                                        c
                                                    }
                                                })
                                                .collect::<String>()
                                        })
                                        .collect();
                                    let line_refs: Vec<&str> =
                                        visible_lines.iter().map(|s| s.as_str()).collect();
                                    if let Some(_req) = state
                                        .allow_flow
                                        .engine
                                        .process_output(pane_id, ws_idx, &line_refs)
                                    {
                                        state.allow_flow.pane_hint_visible = true;
                                    }
                                    break 'outer_scan;
                                }
                            }
                        }
                    }
                }

                // Nudge status collector so it picks up changes quickly
                // (e.g., SSH exit, cd, git operations).
                if total > 0 {
                    state.status_collector.nudge();
                }

                // Update tab display titles after VT feed.
                if total > 0 {
                    let fmt = state.config.tab_bar.format.clone();
                    for ws in &mut state.workspaces {
                        for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                            if tab.panes.contains_key(&pane_id) {
                                update_tab_title(
                                    tab,
                                    &fmt,
                                    ti + 1,
                                    &state.pane_git_cache.cwd.clone(),
                                );
                            }
                        }
                    }
                    update_window_title(state);
                    state.window.request_redraw();
                }
            }
            UserEvent::PtyExited(pane_id) => {
                log::info!("pane {pane_id}: shell exited");
                let Some(state) = &mut self.state else {
                    return;
                };

                // Find which workspace/tab owns this pane and remove it.
                self.pty_buffers.lock().unwrap_or_else(|e| e.into_inner()).remove(&pane_id);

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
                        update_window_title(state);
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
                                // Clean up PTY buffers and daemon registrations for all
                                // remaining panes in the workspace being removed.
                                {
                                    let ws = &state.workspaces[ws_idx];
                                    let pane_ids: Vec<PaneId> = ws
                                        .tabs
                                        .iter()
                                        .flat_map(|t| t.panes.keys().copied())
                                        .collect();
                                    if let Ok(mut bufs) = self.pty_buffers.lock() {
                                        for pid in &pane_ids {
                                            bufs.remove(pid);
                                        }
                                    }
                                }
                                state.workspaces.remove(ws_idx);
                                if ws_idx < state.workspace_infos.len() {
                                    state.workspace_infos.remove(ws_idx);
                                }
                                if ws_idx < state.agent_infos.len() {
                                    state.agent_infos.remove(ws_idx);
                                }
                                if ws_idx < state.dir_trees.len() {
                                    state.dir_trees.remove(ws_idx);
                                }
                                cleanup_session_to_workspace(state, ws_idx);
                                if state.active_workspace >= state.workspaces.len() {
                                    state.active_workspace = state.workspaces.len() - 1;
                                }
                                resize_all_panes(state);
                                update_window_title(state);
                                state.window.request_redraw();
                            }
                        } else {
                            ws.tabs.remove(tab_idx);
                            if ws.active_tab >= ws.tabs.len() {
                                ws.active_tab = ws.tabs.len() - 1;
                            }
                            resize_all_panes(state);
                            update_window_title(state);
                            state.window.request_redraw();
                        }
                    }
                }
            }
            UserEvent::StatusUpdate => {
                let Some(state) = &mut self.state else { return };
                // Update tab titles with the latest CWD from background thread.
                let fmt = state.config.tab_bar.format.clone();
                let cwd = state.pane_git_cache.cwd.clone();
                let ws = active_ws_mut(state);
                for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                    update_tab_title(tab, &fmt, ti + 1, &cwd);
                }
                update_window_title(state);
                state.window.request_redraw();
            }
            UserEvent::ToggleQuickTerminal => {
                log::info!("toggle quick terminal requested");
                let Some(state) = &mut self.state else { return };
                toggle_quick_terminal(state);
            }
            UserEvent::AppIpc {
                request,
                response_tx,
                connection_alive,
            } => {
                let Some(state) = &mut self.state else { return };
                if let Some(response) = handle_app_ipc_request(
                    state,
                    &request,
                    &self.proxy,
                    &self.pty_buffers,
                    event_loop,
                    &response_tx,
                    connection_alive,
                ) {
                    let _ = response_tx.send(response);
                }
                // If None, the response is deferred (PermissionRequest).
                state.window.request_redraw();
            }
            UserEvent::IpcClientDisconnected => {
                let Some(state) = &mut self.state else { return };
                // Remove pending IPC responses whose connection is dead.
                let stale_ids: Vec<u64> = state
                    .pending_ipc_responses
                    .iter()
                    .filter(|(_, (_, alive))| !alive.load(Ordering::SeqCst))
                    .map(|(id, _)| *id)
                    .collect();

                if stale_ids.is_empty() {
                    return;
                }

                log::info!(
                    "IPC client disconnected — cleaning up {} stale pending request(s)",
                    stale_ids.len()
                );

                for req_id in &stale_ids {
                    state.pending_ipc_responses.remove(req_id);
                    state.allow_flow.engine.dismiss_request(*req_id);
                }

                // Update pane hint visibility.
                if state.allow_flow.engine.pending_requests().is_empty() {
                    state.allow_flow.pane_hint_visible = false;
                }

                // Reset agent state for workspaces that no longer have pending requests.
                for wi in 0..state.agent_infos.len() {
                    if state.agent_infos[wi].active
                        && matches!(
                            state.agent_infos[wi].state,
                            AgentState::WaitingForPermission
                        )
                        && !state.allow_flow.has_pending_for_workspace(wi)
                    {
                        state.agent_infos[wi].state = AgentState::Idle;
                        state.agent_infos[wi].last_updated = std::time::Instant::now();
                    }
                }

                state.window.request_redraw();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Search key handling (Feature 5)
// ---------------------------------------------------------------------------

enum SearchKeyResult {
    Consumed,
    Dismiss,
    Pass,
}

/// Scroll the focused pane's terminal so the current search match is visible.
///
/// If the current match row is outside the visible viewport, adjust
/// `scroll_offset` to bring it into view. Currently, the search only
/// runs against the visible grid, so this is largely a no-op, but it
/// serves as the hook for when search is extended to scrollback.
fn scroll_to_search_match(state: &mut AppState) {
    let (match_row, rows) = {
        let search = match state.search.as_ref() {
            Some(s) if !s.matches.is_empty() => s,
            _ => return,
        };
        let (m_row, _, _) = search.matches[search.current];

        let ws_idx = state.active_workspace;
        let tab_idx = state.workspaces[ws_idx].active_tab;
        let focused_id = state.workspaces[ws_idx].tabs[tab_idx].layout.focused();
        let rows = state.workspaces[ws_idx].tabs[tab_idx]
            .panes
            .get(&focused_id)
            .map(|p| p.terminal.grid().rows())
            .unwrap_or(0);
        (m_row, rows)
    };

    if rows == 0 {
        return;
    }

    // If match row is within the visible viewport (0..rows), no scroll needed.
    if match_row < rows {
        return;
    }

    // Match is outside the visible area — adjust scroll offset.
    // Place the match row roughly in the middle of the viewport.
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    let focused_id = state.workspaces[ws_idx].tabs[tab_idx].layout.focused();
    if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx]
        .panes
        .get_mut(&focused_id)
    {
        let half = rows / 2;
        let new_offset = match_row.saturating_sub(half);
        pane.terminal.set_scroll_offset(new_offset);
    }
}

fn handle_search_key(state: &mut AppState, event: &winit::event::KeyEvent) -> SearchKeyResult {
    if state.search.is_none() {
        return SearchKeyResult::Pass;
    }

    match &event.logical_key {
        Key::Named(NamedKey::Escape) => {
            return SearchKeyResult::Dismiss;
        }
        Key::Named(NamedKey::Enter)
        | Key::Named(NamedKey::ArrowDown)
        | Key::Named(NamedKey::ArrowUp) => {
            let go_prev = matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp))
                || (matches!(&event.logical_key, Key::Named(NamedKey::Enter))
                    && state.modifiers.shift_key());
            if let Some(ref mut search) = state.search {
                if go_prev {
                    search.prev_match();
                } else {
                    search.next_match();
                }
            }
            scroll_to_search_match(state);
            return SearchKeyResult::Consumed;
        }
        Key::Named(NamedKey::Backspace) => {
            if let Some(ref mut search) = state.search {
                search.query.pop();
            }
            // Re-search after modifying query.
            search_in_focused_pane(state);
            return SearchKeyResult::Consumed;
        }
        // Ctrl+N / Ctrl+P: navigate matches (same as arrow keys).
        Key::Character(c)
            if state.modifiers.control_key() && (c.as_str() == "n" || c.as_str() == "\x0e") =>
        {
            if let Some(ref mut search) = state.search {
                search.next_match();
            }
            scroll_to_search_match(state);
            return SearchKeyResult::Consumed;
        }
        Key::Character(c)
            if state.modifiers.control_key() && (c.as_str() == "p" || c.as_str() == "\x10") =>
        {
            if let Some(ref mut search) = state.search {
                search.prev_match();
            }
            scroll_to_search_match(state);
            return SearchKeyResult::Consumed;
        }
        _ => {
            if let Some(ref text) = event.text {
                if !text.is_empty() && !text.contains('\r') && !text.contains('\x1b') {
                    // Skip control characters (Ctrl+key combos produce control codes).
                    if text.chars().all(|c| c.is_control()) {
                        return SearchKeyResult::Consumed;
                    }
                    let text = text.clone();
                    if let Some(ref mut search) = state.search {
                        search.query.push_str(&text);
                    }
                    search_in_focused_pane(state);
                    return SearchKeyResult::Consumed;
                }
            }
        }
    }

    SearchKeyResult::Pass
}

// ---------------------------------------------------------------------------
// Command Timeline keyboard handling
// ---------------------------------------------------------------------------

enum TimelineKeyResult {
    Consumed,
    JumpToCommand(u64),
    RerunCommand(String),
    Dismiss,
    Pass,
}

fn handle_timeline_key(state: &mut AppState, event: &winit::event::KeyEvent) -> TimelineKeyResult {
    use winit::keyboard::{Key, NamedKey};

    // W2: Work with references instead of cloning the entire history.
    let focused_id = active_tab(state).layout.focused();
    let filter = state.timeline_input.to_lowercase();

    // Build filtered index list from a borrow (no clone).
    let filtered_len = active_tab(state)
        .panes
        .get(&focused_id)
        .map(|p| {
            p.terminal
                .command_history()
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, cmd)| {
                    filter.is_empty() || cmd.command_text.to_lowercase().contains(&filter)
                })
                .count()
        })
        .unwrap_or(0);

    match &event.logical_key {
        Key::Named(NamedKey::Escape) => TimelineKeyResult::Dismiss,
        Key::Named(NamedKey::ArrowUp) => {
            if state.timeline_selected > 0 {
                state.timeline_selected -= 1;
            }
            TimelineKeyResult::Consumed
        }
        Key::Named(NamedKey::ArrowDown) => {
            if filtered_len > 0 && state.timeline_selected < filtered_len - 1 {
                state.timeline_selected += 1;
            }
            TimelineKeyResult::Consumed
        }
        Key::Named(NamedKey::Enter) => {
            // Only clone the single selected record for Enter action.
            let result = active_tab(state).panes.get(&focused_id).and_then(|p| {
                let history = p.terminal.command_history();
                let filtered: Vec<usize> = history
                    .iter()
                    .enumerate()
                    .rev()
                    .filter(|(_, cmd)| {
                        filter.is_empty() || cmd.command_text.to_lowercase().contains(&filter)
                    })
                    .map(|(i, _)| i)
                    .collect();
                filtered.get(state.timeline_selected).map(|&cmd_idx| {
                    let cmd = &history[cmd_idx];
                    if state.modifiers.super_key() {
                        TimelineKeyResult::RerunCommand(cmd.command_text.clone())
                    } else {
                        TimelineKeyResult::JumpToCommand(cmd.id)
                    }
                })
            });
            result.unwrap_or(TimelineKeyResult::Consumed)
        }
        Key::Named(NamedKey::Backspace) => {
            state.timeline_input.pop();
            state.timeline_selected = 0;
            state.timeline_scroll_offset = 0;
            TimelineKeyResult::Consumed
        }
        _ => {
            if let Some(ref text) = event.text {
                if !text.is_empty() && !text.contains('\r') && !text.contains('\x1b') {
                    // S7: Cap timeline input length to prevent rendering issues.
                    if state.timeline_input.len() < 256 {
                        state.timeline_input.push_str(text);
                    }
                    state.timeline_selected = 0;
                    state.timeline_scroll_offset = 0;
                    return TimelineKeyResult::Consumed;
                }
            }
            TimelineKeyResult::Pass
        }
    }
}

/// Run search on the focused pane's grid.
fn search_in_focused_pane(state: &mut AppState) {
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    let focused_id = state.workspaces[ws_idx].tabs[tab_idx].layout.focused();
    // We need to get a reference to the grid, then run search.
    // Build search matches from grid data, then update state.search.
    let query = state
        .search
        .as_ref()
        .map(|s| s.query.clone())
        .unwrap_or_default();
    if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx]
        .panes
        .get(&focused_id)
    {
        let grid = pane.terminal.grid();
        // Perform inline search to avoid borrow issues.
        let mut matches = Vec::new();
        if !query.is_empty() {
            let query_lower = query.to_lowercase();
            let query_chars: Vec<char> = query_lower.chars().collect();
            let qlen = query_chars.len();
            for row in 0..grid.rows() {
                let mut row_lower = Vec::with_capacity(grid.cols());
                for col in 0..grid.cols() {
                    let cell = grid.cell(col, row);
                    let c = if cell.c == '\0' { ' ' } else { cell.c };
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    row_lower.push(s.to_lowercase().chars().next().unwrap_or(c));
                }
                for start_col in 0..row_lower.len() {
                    if start_col + qlen > row_lower.len() {
                        break;
                    }
                    let mut found = true;
                    for (qi, qc) in query_chars.iter().enumerate() {
                        if row_lower[start_col + qi] != *qc {
                            found = false;
                            break;
                        }
                    }
                    if found {
                        matches.push((row, start_col, start_col + qlen - 1));
                    }
                }
            }
        }
        if let Some(ref mut search) = state.search {
            search.matches = matches;
            search.current = 0;
        }
    }
}

/// Dispatch an action from keybinding or command palette.
///
/// Returns `true` if the action was handled (caller should `return`),
/// or `false` if the key should fall through to PTY (e.g., `Passthrough`).
pub(crate) fn resize_all_panes(state: &mut AppState) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width as u32;
    let cell_h = cell_size.height as u32;
    let pane_rects = active_pane_rects(state);
    for (pid, rect) in &pane_rects {
        let (cols, rows) = state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
        let cols = cols.max(1);
        let rows = rows.max(1);
        let tab = active_tab_mut(state);
        if let Some(pane) = tab.panes.get_mut(pid) {
            pane.terminal.resize(cols as usize, rows as usize);
            pane.terminal.image_store.set_cell_size(cell_w, cell_h);
            daemon_pty_resize(&pane.session_id, cols, rows);
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
    pane_rects: &[(PaneId, termojinal_layout::Rect)],
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
                if y_bot > y_top && my >= y_top && my <= y_bot && (mx - r1_right).abs() <= threshold
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

/// Detach all PTY handles from the GUI so that shells survive the GUI exit.
/// The daemon keeps tracking these sessions; users can reconnect later or
/// use `tm kill --all` to terminate them.
pub(crate) fn detach_all_ptys(_state: &mut AppState) {
    // No-op: daemon owns PTYs, GUI exit does not affect sessions.
}

/// Compute the drop zone for a cursor position within a pane rect.
///
/// Layout (no dead zone):
/// - Top 20%: Top
/// - Bottom 20%: Bottom
/// - Middle 60%, left half: Left
/// - Middle 60%, right half: Right
fn compute_drop_zone(cx: f32, cy: f32, rect: &termojinal_layout::Rect) -> DropZone {
    let rel_y = (cy - rect.y) / rect.h;
    if rel_y < 0.2 {
        return DropZone::Top;
    }
    if rel_y > 0.8 {
        return DropZone::Bottom;
    }
    let rel_x = (cx - rect.x) / rect.w;
    if rel_x < 0.5 {
        DropZone::Left
    } else {
        DropZone::Right
    }
}

/// Close the focused pane. If it is the last pane in the tab, close the tab.
/// If it is the last tab in the workspace, close the workspace.
/// If it is the last workspace, exit the app.
pub(crate) fn close_focused_pane(
    state: &mut AppState,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) {
    let tab = active_tab_mut(state);
    let focused_id = tab.layout.focused();
    // Drop the pane (this sends SIGHUP to the PTY child).
    tab.panes.remove(&focused_id);
    buffers.lock().unwrap_or_else(|e| e.into_inner()).remove(&focused_id);

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
                    let removed_idx = state.active_workspace;
                    // Clean up PTY buffers and daemon registrations for all
                    // remaining panes in the workspace being removed.
                    {
                        let ws = &state.workspaces[removed_idx];
                        let pane_ids: Vec<PaneId> = ws
                            .tabs
                            .iter()
                            .flat_map(|t| t.panes.keys().copied())
                            .collect();
                        if let Ok(mut bufs) = buffers.lock() {
                            for pid in &pane_ids {
                                bufs.remove(pid);
                            }
                        }
                    }
                    state.workspaces.remove(removed_idx);
                    if removed_idx < state.workspace_infos.len() {
                        state.workspace_infos.remove(removed_idx);
                    }
                    if removed_idx < state.agent_infos.len() {
                        state.agent_infos.remove(removed_idx);
                    }
                    if removed_idx < state.dir_trees.len() {
                        state.dir_trees.remove(removed_idx);
                    }
                    cleanup_session_to_workspace(state, removed_idx);
                    if state.active_workspace >= state.workspaces.len() {
                        state.active_workspace = state.workspaces.len() - 1;
                    }
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
            } else {
                let tab_idx = ws.active_tab;
                ws.tabs.remove(tab_idx);
                if ws.active_tab >= ws.tabs.len() {
                    ws.active_tab = ws.tabs.len() - 1;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
        }
    }
}




/// Truncate a string to at most `max_chars` characters, appending an
/// ellipsis if truncation occurs.
/// Escape a path for safe use in shell commands.
pub(crate) fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[allow(dead_code)]
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('\u{2026}'); // ellipsis
        out
    }
}

/// Extract a clickable target (URL or file path) from the terminal text at
/// the given screen position. Scans the row containing (click_row, click_col)
/// for URLs (`https://`, `http://`) and absolute file paths starting with `/`.
///
/// Returns `Some(target_string)` if a clickable target is found at or around
/// the click position, `None` otherwise.
fn extract_clickable_target(
    terminal: &termojinal_vt::Terminal,
    click_row: usize,
    click_col: usize,
) -> Option<String> {
    let grid = terminal.grid();
    let cols = grid.cols();
    if click_row >= grid.rows() {
        return None;
    }

    // Extract the full row text as a string, tracking character-to-column mapping.
    let mut row_text = String::new();
    let mut col_to_char_idx: Vec<usize> = Vec::with_capacity(cols);
    for col in 0..cols {
        let cell = grid.cell(col, click_row);
        let char_idx = row_text.len();
        col_to_char_idx.push(char_idx);
        if cell.width > 0 && cell.c != '\0' {
            row_text.push(cell.c);
        } else if cell.width == 0 {
            // Continuation of wide char — map to same char index as the lead cell.
            // col_to_char_idx already pushed, overwrite with previous value.
            if col > 0 {
                let prev_val = col_to_char_idx[col - 1];
                if let Some(last) = col_to_char_idx.last_mut() {
                    *last = prev_val;
                }
            }
        } else {
            row_text.push(' ');
        }
    }
    let row_text = row_text.trim_end().to_string();

    if row_text.is_empty() {
        return None;
    }

    let click_char_idx = if click_col < col_to_char_idx.len() {
        col_to_char_idx[click_col]
    } else {
        row_text.len()
    };

    // Characters that can appear in URLs or paths.
    let is_target_char = |c: char| -> bool {
        !c.is_whitespace() && c != '\'' && c != '"' && c != '<' && c != '>' && c != '|'
    };

    // Scan for URL patterns first (higher priority).
    for prefix in &["https://", "http://"] {
        if let Some(start_byte) = row_text.find(prefix) {
            // Find the end of the URL.
            let end_byte = row_text[start_byte..]
                .find(|c: char| !is_target_char(c))
                .map(|e| start_byte + e)
                .unwrap_or(row_text.len());
            // Trim trailing punctuation that is unlikely to be part of the URL.
            let url = row_text[start_byte..end_byte]
                .trim_end_matches(|c: char| matches!(c, ')' | ']' | '}' | '.' | ',' | ';' | ':'));
            // Check if the click position falls within this URL.
            if click_char_idx >= start_byte && click_char_idx < start_byte + url.len() {
                return Some(url.to_string());
            }
        }
    }

    // Scan for absolute file paths (starting with /).
    // Walk backwards from click position to find start, forwards to find end.
    let chars: Vec<char> = row_text.chars().collect();
    if click_char_idx < chars.len() {
        // Check if the character at the click position could be part of a path.
        if is_target_char(chars[click_char_idx]) {
            let mut start = click_char_idx;
            while start > 0 && is_target_char(chars[start - 1]) {
                start -= 1;
            }
            let mut end = click_char_idx;
            while end < chars.len() && is_target_char(chars[end]) {
                end += 1;
            }
            let candidate: String = chars[start..end].iter().collect();
            // Trim trailing punctuation.
            let candidate = candidate
                .trim_end_matches(|c: char| matches!(c, ')' | ']' | '}' | '.' | ',' | ';' | ':'));
            // Accept if it looks like an absolute path.
            if candidate.starts_with('/') && candidate.len() > 1 {
                return Some(candidate.to_string());
            }
            // Also accept ~/... paths.
            if candidate.starts_with("~/") && candidate.len() > 2 {
                // Expand ~ to home directory.
                if let Some(home) = dirs::home_dir() {
                    return Some(format!("{}{}", home.display(), &candidate[1..]));
                }
                return Some(candidate.to_string());
            }
        }
    }

    None
}






















// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let quick_terminal_mode = std::env::args().any(|a| a == "--quick-terminal");
    if quick_terminal_mode {
        log::info!("quick terminal mode enabled via --quick-terminal flag");
    }

    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }

    let event_loop = builder.build().expect("failed to create event loop");

    let proxy = event_loop.create_proxy();

    // Shared shutdown flag for background threads.
    let app_shutdown = Arc::new(AtomicBool::new(false));

    // Start app-side IPC listener for daemon commands (toggle_quick_terminal, etc.)
    {
        let proxy = proxy.clone();
        let shutdown = Arc::clone(&app_shutdown);
        std::thread::Builder::new()
            .name("app-ipc-listener".into())
            .spawn(move || {
                app_ipc_listener(proxy, shutdown);
            })
            .expect("failed to spawn app IPC listener");
    }

    // Start global hotkey monitor (Ctrl+`, Cmd+Shift+P, etc.) directly in the app.
    // Running inside the .app bundle lets macOS show the Accessibility permission
    // dialog and remember the grant by bundle ID.
    let _hotkey_handle = {
        use termojinal_session::hotkey::{GlobalHotkey, HotkeyEvent};
        let proxy = proxy.clone();
        match GlobalHotkey::start(move |event| {
            let user_event = match event {
                HotkeyEvent::QuickTerminal => UserEvent::ToggleQuickTerminal,
                HotkeyEvent::CommandPalette | HotkeyEvent::AllowFlowPanel => {
                    // These are handled via normal keybindings when the app is focused.
                    // Global hotkey for these is only useful via daemon when app is hidden.
                    return;
                }
            };
            let _ = proxy.send_event(user_event);
        }) {
            Ok(handle) => {
                log::info!("global hotkey monitor active (Cmd+` for Quick Terminal)");
                Some(handle)
            }
            Err(e) => {
                log::warn!("global hotkey unavailable: {e}");
                None
            }
        }
    };

    let config = load_config();
    log::info!(
        "config: font.size={}, window={}x{}",
        config.font.size,
        config.window.width,
        config.window.height
    );
    let mut app = App::new(proxy, config, Arc::clone(&app_shutdown));
    app.quick_terminal_mode = quick_terminal_mode;

    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
    }

    // Clean up the app IPC socket on exit.
    let sock_path = app_ipc_socket_path();
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
        log::info!("removed app IPC socket at {}", sock_path.display());
    }
}
