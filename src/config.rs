//! Configuration loading for termojinal.
//!
//! Loads settings from `~/.config/termojinal/config.toml` with sane defaults.

use serde::Deserialize;

/// Top-level termojinal configuration.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TermojinalConfig {
    #[serde(default)]
    pub font: FontSection,
    #[serde(default)]
    pub window: WindowSection,
    #[serde(default)]
    pub theme: ThemeSection,
    #[serde(default)]
    pub sidebar: SidebarConfig,
    #[serde(default)]
    pub tab_bar: TabBarConfig,
    #[serde(default)]
    pub pane: PaneConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub palette: PaletteConfig,
    #[serde(default)]
    pub status_bar: StatusBarConfig,
    #[serde(default)]
    pub allow_flow: termojinal_claude::AllowFlowConfig,
    #[serde(default)]
    pub allow_flow_ui: AllowFlowUiConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub quick_terminal: QuickTerminalConfig,
    #[serde(default)]
    pub startup: StartupConfig,
    #[serde(default)]
    pub directory_tree: DirectoryTreeConfig,
    #[serde(default)]
    pub time_travel: TimeTravelConfig,
}

impl Default for TermojinalConfig {
    fn default() -> Self {
        Self {
            font: FontSection::default(),
            window: WindowSection::default(),
            theme: ThemeSection::default(),
            sidebar: SidebarConfig::default(),
            tab_bar: TabBarConfig::default(),
            pane: PaneConfig::default(),
            search: SearchConfig::default(),
            palette: PaletteConfig::default(),
            status_bar: StatusBarConfig::default(),
            allow_flow: termojinal_claude::AllowFlowConfig::default(),
            allow_flow_ui: AllowFlowUiConfig::default(),
            notifications: NotificationConfig::default(),
            quick_terminal: QuickTerminalConfig::default(),
            startup: StartupConfig::default(),
            directory_tree: DirectoryTreeConfig::default(),
            time_travel: TimeTravelConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// [allow_flow_ui]
// ---------------------------------------------------------------------------

/// Allow Flow hint bar appearance settings (`[allow_flow_ui]`).
///
/// Controls the colors of the thin permission-hint bar rendered at the
/// bottom of the focused pane when an AI tool needs approval.
#[derive(Debug, Clone, Deserialize)]
pub struct AllowFlowUiConfig {
    /// Background color of the permission hint bar (hex, default: orange).
    #[serde(default = "default_hint_bar_bg")]
    pub hint_bar_bg: String,
    /// Foreground (text) color of the permission hint bar (hex, default: dark).
    #[serde(default = "default_hint_bar_fg")]
    pub hint_bar_fg: String,
    /// Accent line color at top of hint bar (hex, default: bright amber).
    #[serde(default = "default_hint_bar_accent")]
    pub hint_bar_accent: String,
}

fn default_hint_bar_bg() -> String { "#D97706E0".into() }
fn default_hint_bar_fg() -> String { "#1A1A24".into() }
fn default_hint_bar_accent() -> String { "#F59E0B".into() }

impl Default for AllowFlowUiConfig {
    fn default() -> Self {
        Self {
            hint_bar_bg: default_hint_bar_bg(),
            hint_bar_fg: default_hint_bar_fg(),
            hint_bar_accent: default_hint_bar_accent(),
        }
    }
}

// ---------------------------------------------------------------------------
// [notifications]
// ---------------------------------------------------------------------------

/// Desktop notification configuration section (`[notifications]`).
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationConfig {
    /// Whether desktop notifications are enabled.
    #[serde(default = "default_notifications_enabled")]
    pub enabled: bool,
    /// Whether to play a sound with notifications.
    #[serde(default = "default_notification_sound")]
    pub sound: bool,
}

fn default_notifications_enabled() -> bool { true }
fn default_notification_sound() -> bool { false }

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: default_notifications_enabled(),
            sound: default_notification_sound(),
        }
    }
}

// ---------------------------------------------------------------------------
// [directory_tree]
// ---------------------------------------------------------------------------

/// How to determine the root directory for the tree display.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeRootMode {
    /// Auto-detect: use git root if inside a repo, otherwise use CWD.
    Auto,
    /// Always use the terminal's current working directory.
    Cwd,
    /// Always use the git repository root (falls back to CWD if not in a repo).
    GitRoot,
}

impl Default for TreeRootMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Directory tree configuration section (`[directory_tree]`).
#[derive(Debug, Clone, Deserialize)]
pub struct DirectoryTreeConfig {
    /// How to determine the tree root directory.
    #[serde(default)]
    pub root_mode: TreeRootMode,
    /// Foreground color for directory names.
    #[serde(default = "default_tree_dir_fg")]
    pub dir_fg: String,
    /// Foreground color for file names.
    #[serde(default = "default_tree_file_fg")]
    pub file_fg: String,
    /// Foreground color for the selected/highlighted entry.
    #[serde(default = "default_tree_selected_fg")]
    pub selected_fg: String,
    /// Background color for the selected/highlighted entry.
    #[serde(default = "default_tree_selected_bg")]
    pub selected_bg: String,
    /// Foreground color for tree guide lines (▸/▾ arrows).
    #[serde(default = "default_tree_guide_fg")]
    pub guide_fg: String,
    /// Maximum number of visible tree lines before scrolling.
    #[serde(default = "default_tree_max_lines")]
    pub max_visible_lines: usize,
    /// Editor command used when pressing `v` on a file.
    /// Defaults to $EDITOR, falling back to "nvim".
    #[serde(default = "default_tree_editor")]
    pub editor: String,
    /// Double-click interval in milliseconds for cd action.
    #[serde(default = "default_tree_double_click_ms")]
    pub double_click_ms: u64,
}

fn default_tree_dir_fg() -> String { "#89B4FA".into() }  // blue (matches Catppuccin)
fn default_tree_file_fg() -> String { "#BAC2DE".into() }  // subtext
fn default_tree_selected_fg() -> String { "#F2F2F8".into() }
fn default_tree_selected_bg() -> String { "#313244".into() }
fn default_tree_guide_fg() -> String { "#6C7086".into() }
fn default_tree_max_lines() -> usize { 20 }
fn default_tree_editor() -> String { String::new() }  // empty = use $EDITOR or "nvim"
fn default_tree_double_click_ms() -> u64 { 400 }

impl Default for DirectoryTreeConfig {
    fn default() -> Self {
        Self {
            root_mode: TreeRootMode::default(),
            dir_fg: default_tree_dir_fg(),
            file_fg: default_tree_file_fg(),
            selected_fg: default_tree_selected_fg(),
            selected_bg: default_tree_selected_bg(),
            guide_fg: default_tree_guide_fg(),
            max_visible_lines: default_tree_max_lines(),
            editor: default_tree_editor(),
            double_click_ms: default_tree_double_click_ms(),
        }
    }
}

// ---------------------------------------------------------------------------
// [time_travel]
// ---------------------------------------------------------------------------

/// Time Travel feature configuration (`[time_travel]`).
///
/// Controls command history tracking, navigation, timeline UI,
/// session persistence, and named snapshots.
#[derive(Debug, Clone, Deserialize)]
pub struct TimeTravelConfig {
    /// Enable command history recording (OSC 133 based).
    #[serde(default = "default_true")]
    pub command_history: bool,
    /// Maximum number of command records to keep per session.
    #[serde(default = "default_max_command_history")]
    pub max_command_history: usize,
    /// Enable Cmd+Up/Down command navigation.
    #[serde(default = "default_true")]
    pub command_navigation: bool,
    /// Show command boundary markers in the left gutter.
    #[serde(default = "default_true")]
    pub show_command_marker: bool,
    /// Show command position (e.g. "Command 15/42") in the status bar.
    #[serde(default = "default_true")]
    pub show_command_position: bool,
    /// Enable the Command Timeline UI (Cmd+Shift+T).
    #[serde(default = "default_true")]
    pub timeline_ui: bool,
    /// Save full session state on exit.
    #[serde(default = "default_true")]
    pub session_persistence: bool,
    /// Restore previous session on startup.
    #[serde(default = "default_true")]
    pub restore_on_startup: bool,
    /// Enable named snapshots.
    #[serde(default = "default_true")]
    pub snapshots: bool,
    /// Maximum number of named snapshots per session.
    #[serde(default = "default_max_snapshots")]
    pub max_snapshots_per_session: usize,
}

fn default_max_command_history() -> usize { 10_000 }
fn default_max_snapshots() -> usize { 50 }

impl Default for TimeTravelConfig {
    fn default() -> Self {
        Self {
            command_history: true,
            max_command_history: default_max_command_history(),
            command_navigation: true,
            show_command_marker: true,
            show_command_position: true,
            timeline_ui: true,
            session_persistence: true,
            restore_on_startup: true,
            snapshots: true,
            max_snapshots_per_session: default_max_snapshots(),
        }
    }
}

// ---------------------------------------------------------------------------
// [startup]
// ---------------------------------------------------------------------------

/// Startup directory behavior.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    /// Use the default directory (typically $HOME).
    Default,
    /// Always open a fixed directory.
    Fixed,
    /// Restore the last working directory from the previous session.
    Restore,
}

impl std::default::Default for StartupMode {
    fn default() -> Self {
        Self::Default
    }
}

/// Startup configuration section (`[startup]`).
#[derive(Debug, Clone, Deserialize)]
pub struct StartupConfig {
    /// How to determine the initial working directory.
    #[serde(default)]
    pub mode: StartupMode,
    /// Fixed directory path (used when `mode = "fixed"`).
    #[serde(default)]
    pub directory: String,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            mode: StartupMode::Default,
            directory: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: parse hex color
// ---------------------------------------------------------------------------

/// Parse a hex color string (#RGB, #RRGGBB, or #RRGGBBAA) to [f32; 4] RGBA.
pub fn parse_hex_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim_start_matches('#');
    if s.len() == 3 {
        // #RGB -> expand each nibble: R -> RR, G -> GG, B -> BB
        let r = u8::from_str_radix(&s[0..1], 16).ok()?;
        let g = u8::from_str_radix(&s[1..2], 16).ok()?;
        let b = u8::from_str_radix(&s[2..3], 16).ok()?;
        Some([
            (r * 17) as f32 / 255.0,
            (g * 17) as f32 / 255.0,
            (b * 17) as f32 / 255.0,
            1.0,
        ])
    } else if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = u8::from_str_radix(&s[6..8], 16).ok()?;
        Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0])
    } else {
        None
    }
}

/// Shorthand: parse hex or return fallback.
pub fn color_or(s: &str, fallback: [f32; 4]) -> [f32; 4] {
    parse_hex_color(s).unwrap_or(fallback)
}

// ---------------------------------------------------------------------------
// [theme]
// ---------------------------------------------------------------------------

/// Theme/color configuration section (`[theme]`).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ThemeSection {
    #[serde(default = "default_bg")]
    pub background: String,
    #[serde(default = "default_fg")]
    pub foreground: String,
    #[serde(default = "default_cursor_color")]
    pub cursor: String,
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,
    #[serde(default = "default_preedit_bg")]
    pub preedit_bg: String,
    #[serde(default = "default_search_highlight_bg")]
    pub search_highlight_bg: String,
    #[serde(default = "default_search_highlight_fg")]
    pub search_highlight_fg: String,
    #[serde(default = "default_bold_brightness")]
    pub bold_brightness: f32,
    #[serde(default = "default_dim_opacity")]
    pub dim_opacity: f32,

    // ANSI 16 colors
    #[serde(default = "default_ansi_black")]
    pub black: String,
    #[serde(default = "default_ansi_bright_black")]
    pub bright_black: String,
    #[serde(default = "default_ansi_red")]
    pub red: String,
    #[serde(default = "default_ansi_bright_red")]
    pub bright_red: String,
    #[serde(default = "default_ansi_green")]
    pub green: String,
    #[serde(default = "default_ansi_bright_green")]
    pub bright_green: String,
    #[serde(default = "default_ansi_yellow")]
    pub yellow: String,
    #[serde(default = "default_ansi_bright_yellow")]
    pub bright_yellow: String,
    #[serde(default = "default_ansi_blue")]
    pub blue: String,
    #[serde(default = "default_ansi_bright_blue")]
    pub bright_blue: String,
    #[serde(default = "default_ansi_magenta")]
    pub magenta: String,
    #[serde(default = "default_ansi_bright_magenta")]
    pub bright_magenta: String,
    #[serde(default = "default_ansi_cyan")]
    pub cyan: String,
    #[serde(default = "default_ansi_bright_cyan")]
    pub bright_cyan: String,
    #[serde(default = "default_ansi_white")]
    pub white: String,
    #[serde(default = "default_ansi_bright_white")]
    pub bright_white: String,

    // Dark/Light auto-switch
    #[serde(default)]
    pub auto_switch: bool,
    #[serde(default)]
    pub dark: String,
    #[serde(default)]
    pub light: String,
}

fn default_bg() -> String { "#1E1E2E".into() }
fn default_fg() -> String { "#CDD6F4".into() }
fn default_cursor_color() -> String { "#F5E0DC".into() }
fn default_selection_bg() -> String { "#45475A".into() }
fn default_preedit_bg() -> String { "#313244".into() }
fn default_search_highlight_bg() -> String { "#F9E2AF".into() }
fn default_search_highlight_fg() -> String { "#1E1E2E".into() }
fn default_bold_brightness() -> f32 { 1.2 }
fn default_dim_opacity() -> f32 { 0.6 }

// ANSI 16 color defaults (Catppuccin Mocha)
fn default_ansi_black() -> String { "#45475A".into() }
fn default_ansi_bright_black() -> String { "#585B70".into() }
fn default_ansi_red() -> String { "#F38BA8".into() }
fn default_ansi_bright_red() -> String { "#F38BA8".into() }
fn default_ansi_green() -> String { "#A6E3A1".into() }
fn default_ansi_bright_green() -> String { "#A6E3A1".into() }
fn default_ansi_yellow() -> String { "#F9E2AF".into() }
fn default_ansi_bright_yellow() -> String { "#F9E2AF".into() }
fn default_ansi_blue() -> String { "#89B4FA".into() }
fn default_ansi_bright_blue() -> String { "#89B4FA".into() }
fn default_ansi_magenta() -> String { "#F5C2E7".into() }
fn default_ansi_bright_magenta() -> String { "#F5C2E7".into() }
fn default_ansi_cyan() -> String { "#94E2D5".into() }
fn default_ansi_bright_cyan() -> String { "#94E2D5".into() }
fn default_ansi_white() -> String { "#BAC2DE".into() }
fn default_ansi_bright_white() -> String { "#A6ADC8".into() }

impl Default for ThemeSection {
    fn default() -> Self {
        Self {
            background: default_bg(),
            foreground: default_fg(),
            cursor: default_cursor_color(),
            selection_bg: default_selection_bg(),
            preedit_bg: default_preedit_bg(),
            search_highlight_bg: default_search_highlight_bg(),
            search_highlight_fg: default_search_highlight_fg(),
            bold_brightness: default_bold_brightness(),
            dim_opacity: default_dim_opacity(),
            black: default_ansi_black(),
            bright_black: default_ansi_bright_black(),
            red: default_ansi_red(),
            bright_red: default_ansi_bright_red(),
            green: default_ansi_green(),
            bright_green: default_ansi_bright_green(),
            yellow: default_ansi_yellow(),
            bright_yellow: default_ansi_bright_yellow(),
            blue: default_ansi_blue(),
            bright_blue: default_ansi_bright_blue(),
            magenta: default_ansi_magenta(),
            bright_magenta: default_ansi_bright_magenta(),
            cyan: default_ansi_cyan(),
            bright_cyan: default_ansi_bright_cyan(),
            white: default_ansi_white(),
            bright_white: default_ansi_bright_white(),
            auto_switch: false,
            dark: String::new(),
            light: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// [font]
// ---------------------------------------------------------------------------

/// Font configuration section (`[font]`).
#[derive(Debug, Clone, Deserialize)]
pub struct FontSection {
    #[serde(default = "default_font_family")]
    pub family: String,
    #[serde(default = "default_font_size")]
    pub size: f32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    #[serde(default = "default_max_font_size")]
    pub max_size: f32,
    #[serde(default = "default_font_size_step")]
    pub size_step: f32,
}

fn default_font_family() -> String { "monospace".into() }
fn default_font_size() -> f32 { 14.0 }
fn default_line_height() -> f32 { 1.2 }
fn default_max_font_size() -> f32 { 72.0 }
fn default_font_size_step() -> f32 { 1.0 }

impl Default for FontSection {
    fn default() -> Self {
        Self {
            family: default_font_family(),
            size: default_font_size(),
            line_height: default_line_height(),
            max_size: default_max_font_size(),
            size_step: default_font_size_step(),
        }
    }
}

// ---------------------------------------------------------------------------
// [window]
// ---------------------------------------------------------------------------

/// Window configuration section (`[window]`).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct WindowSection {
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_padding_x")]
    pub padding_x: f32,
    #[serde(default = "default_padding_y")]
    pub padding_y: f32,
}

fn default_width() -> u32 { 960 }
fn default_height() -> u32 { 640 }
fn default_opacity() -> f32 { 1.0 }
fn default_padding_x() -> f32 { 1.0 }
fn default_padding_y() -> f32 { 0.5 }

impl Default for WindowSection {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            opacity: default_opacity(),
            padding_x: default_padding_x(),
            padding_y: default_padding_y(),
        }
    }
}

// ---------------------------------------------------------------------------
// [sidebar]
// ---------------------------------------------------------------------------

/// Sidebar configuration section (`[sidebar]`).
#[derive(Debug, Clone, Deserialize)]
pub struct SidebarConfig {
    #[serde(default = "default_sidebar_width")]
    pub width: f32,
    #[serde(default = "default_sidebar_min_width")]
    pub min_width: f32,
    #[serde(default = "default_sidebar_max_width")]
    pub max_width: f32,
    #[serde(default = "default_sidebar_bg")]
    pub bg: String,
    #[serde(default = "default_sidebar_active_entry_bg")]
    pub active_entry_bg: String,
    #[serde(default = "default_sidebar_active_fg")]
    pub active_fg: String,
    #[serde(default = "default_sidebar_inactive_fg")]
    pub inactive_fg: String,
    #[serde(default = "default_sidebar_dim_fg")]
    pub dim_fg: String,
    #[serde(default = "default_sidebar_git_branch_fg")]
    pub git_branch_fg: String,
    #[serde(default = "default_sidebar_separator_color")]
    pub separator_color: String,
    #[serde(default = "default_sidebar_notification_dot")]
    pub notification_dot: String,
    #[serde(default = "default_sidebar_git_dirty_color")]
    pub git_dirty_color: String,
    #[serde(default = "default_sidebar_top_pad")]
    pub top_padding: f32,
    #[serde(default = "default_sidebar_side_pad")]
    pub side_padding: f32,
    #[serde(default = "default_sidebar_entry_gap")]
    pub entry_gap: f32,
    #[serde(default = "default_sidebar_info_line_gap")]
    pub info_line_gap: f32,
    /// Accent color for the left stripe on workspaces with pending Allow Flow requests.
    #[serde(default = "default_sidebar_allow_accent_color")]
    pub allow_accent_color: String,
    /// Foreground color for Allow Flow hint text in the sidebar.
    #[serde(default = "default_sidebar_allow_hint_fg")]
    pub allow_hint_fg: String,
    /// Whether to show AI agent session status per workspace.
    #[serde(default = "default_agent_status_enabled")]
    pub agent_status_enabled: bool,
    /// Agent indicator style: "pulse", "color", or "none".
    #[serde(default = "default_agent_indicator_style")]
    pub agent_indicator_style: String,
    /// Speed of the pulse animation (cycles per second).
    #[serde(default = "default_agent_pulse_speed")]
    pub agent_pulse_speed: f32,
    /// Color for the agent indicator when Claude Code is active.
    #[serde(default = "default_agent_active_color")]
    pub agent_active_color: String,
    /// Color for the agent indicator when Claude Code is idle.
    #[serde(default = "default_agent_idle_color")]
    pub agent_idle_color: String,
}

fn default_sidebar_width() -> f32 { 240.0 }
fn default_sidebar_min_width() -> f32 { 120.0 }
fn default_sidebar_max_width() -> f32 { 400.0 }
fn default_sidebar_bg() -> String { "#0D0D12".into() }
fn default_sidebar_active_entry_bg() -> String { "#1E1E2A".into() }
fn default_sidebar_active_fg() -> String { "#F2F2F8".into() }
fn default_sidebar_inactive_fg() -> String { "#A0A0AC".into() }
fn default_sidebar_dim_fg() -> String { "#77778A".into() }
fn default_sidebar_git_branch_fg() -> String { "#5AB3D9".into() }
fn default_sidebar_separator_color() -> String { "#333338".into() }
fn default_sidebar_notification_dot() -> String { "#FF941A".into() }
fn default_sidebar_git_dirty_color() -> String { "#CCB34D".into() }
fn default_sidebar_top_pad() -> f32 { 6.0 }
fn default_sidebar_side_pad() -> f32 { 6.0 }
fn default_sidebar_entry_gap() -> f32 { 8.0 }
fn default_sidebar_info_line_gap() -> f32 { 2.0 }
fn default_sidebar_allow_accent_color() -> String { "#4FC1FF".into() }
fn default_sidebar_allow_hint_fg() -> String { "#7DC8FF".into() }
fn default_agent_status_enabled() -> bool { true }
fn default_agent_indicator_style() -> String { "pulse".into() }
fn default_agent_pulse_speed() -> f32 { 2.0 }
fn default_agent_active_color() -> String { "#A78BFA".into() }
fn default_agent_idle_color() -> String { "#FBBF24".into() }

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            width: default_sidebar_width(),
            min_width: default_sidebar_min_width(),
            max_width: default_sidebar_max_width(),
            bg: default_sidebar_bg(),
            active_entry_bg: default_sidebar_active_entry_bg(),
            active_fg: default_sidebar_active_fg(),
            inactive_fg: default_sidebar_inactive_fg(),
            dim_fg: default_sidebar_dim_fg(),
            git_branch_fg: default_sidebar_git_branch_fg(),
            separator_color: default_sidebar_separator_color(),
            notification_dot: default_sidebar_notification_dot(),
            git_dirty_color: default_sidebar_git_dirty_color(),
            top_padding: default_sidebar_top_pad(),
            side_padding: default_sidebar_side_pad(),
            entry_gap: default_sidebar_entry_gap(),
            info_line_gap: default_sidebar_info_line_gap(),
            allow_accent_color: default_sidebar_allow_accent_color(),
            allow_hint_fg: default_sidebar_allow_hint_fg(),
            agent_status_enabled: default_agent_status_enabled(),
            agent_indicator_style: default_agent_indicator_style(),
            agent_pulse_speed: default_agent_pulse_speed(),
            agent_active_color: default_agent_active_color(),
            agent_idle_color: default_agent_idle_color(),
        }
    }
}

// ---------------------------------------------------------------------------
// [tab_bar]
// ---------------------------------------------------------------------------

/// Tab bar configuration section (`[tab_bar]`).
#[derive(Debug, Clone, Deserialize)]
pub struct TabBarConfig {
    #[serde(default = "default_tab_format")]
    pub format: String,
    #[serde(default)]
    pub always_show: bool,
    #[allow(dead_code)]
    #[serde(default = "default_tab_position")]
    pub position: String,
    #[serde(default = "default_max_width")]
    pub max_width: f32,
    #[serde(default = "default_tab_bar_height")]
    pub height: f32,
    #[serde(default = "default_min_tab_width")]
    pub min_tab_width: f32,
    #[serde(default = "default_new_tab_button_width")]
    pub new_tab_button_width: f32,
    #[serde(default = "default_tab_bar_bg")]
    pub bg: String,
    #[serde(default = "default_tab_active_bg")]
    pub active_tab_bg: String,
    #[serde(default = "default_tab_active_fg")]
    pub active_tab_fg: String,
    #[serde(default = "default_tab_inactive_fg")]
    pub inactive_tab_fg: String,
    #[serde(default = "default_tab_accent_color")]
    pub accent_color: String,
    #[serde(default = "default_tab_separator_color")]
    pub separator_color: String,
    #[serde(default = "default_tab_close_button_fg")]
    pub close_button_fg: String,
    #[serde(default = "default_tab_new_button_fg")]
    pub new_button_fg: String,
    #[allow(dead_code)]
    #[serde(default = "default_tab_padding_x")]
    pub padding_x: f32,
    #[serde(default = "default_tab_padding_y")]
    pub padding_y: f32,
    #[serde(default = "default_tab_accent_height")]
    pub accent_height: u32,
    #[serde(default = "default_tab_bottom_border")]
    pub bottom_border: bool,
    #[serde(default = "default_tab_bottom_border_color")]
    pub bottom_border_color: String,
}

fn default_tab_format() -> String { "{title|cwd_base|Tab {index}}".into() }
fn default_tab_position() -> String { "top".into() }
fn default_max_width() -> f32 { 200.0 }
fn default_tab_bar_height() -> f32 { 36.0 }
fn default_min_tab_width() -> f32 { 60.0 }
fn default_new_tab_button_width() -> f32 { 32.0 }
fn default_tab_bar_bg() -> String { "#1A1A1F".into() }
fn default_tab_active_bg() -> String { "#2E2E38".into() }
fn default_tab_active_fg() -> String { "#F2F2F8".into() }
fn default_tab_inactive_fg() -> String { "#8C8C99".into() }
fn default_tab_accent_color() -> String { "#4D8CFF".into() }
fn default_tab_separator_color() -> String { "#383840".into() }
fn default_tab_close_button_fg() -> String { "#808088".into() }
fn default_tab_new_button_fg() -> String { "#808088".into() }
fn default_tab_padding_x() -> f32 { 6.0 }
fn default_tab_padding_y() -> f32 { 6.0 }
fn default_tab_accent_height() -> u32 { 2 }
fn default_tab_bottom_border() -> bool { true }
fn default_tab_bottom_border_color() -> String { "#2A2A34".into() }

impl Default for TabBarConfig {
    fn default() -> Self {
        Self {
            format: default_tab_format(),
            always_show: false,
            position: default_tab_position(),
            max_width: default_max_width(),
            height: default_tab_bar_height(),
            min_tab_width: default_min_tab_width(),
            new_tab_button_width: default_new_tab_button_width(),
            bg: default_tab_bar_bg(),
            active_tab_bg: default_tab_active_bg(),
            active_tab_fg: default_tab_active_fg(),
            inactive_tab_fg: default_tab_inactive_fg(),
            accent_color: default_tab_accent_color(),
            separator_color: default_tab_separator_color(),
            close_button_fg: default_tab_close_button_fg(),
            new_button_fg: default_tab_new_button_fg(),
            padding_x: default_tab_padding_x(),
            padding_y: default_tab_padding_y(),
            accent_height: default_tab_accent_height(),
            bottom_border: default_tab_bottom_border(),
            bottom_border_color: default_tab_bottom_border_color(),
        }
    }
}

// ---------------------------------------------------------------------------
// [pane]
// ---------------------------------------------------------------------------

/// How to determine the working directory when creating new panes/tabs.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneWorkingDirectory {
    /// Inherit the CWD of the focused pane.
    Inherit,
    /// Always use $HOME.
    Home,
    /// Use a fixed directory specified in `fixed_directory`.
    Fixed,
}

impl std::default::Default for PaneWorkingDirectory {
    fn default() -> Self {
        Self::Inherit
    }
}

fn default_pane_working_directory() -> PaneWorkingDirectory { PaneWorkingDirectory::Inherit }

/// Pane configuration section (`[pane]`).
#[derive(Debug, Clone, Deserialize)]
pub struct PaneConfig {
    #[serde(default = "default_pane_separator_color")]
    pub separator_color: String,
    #[serde(default = "default_pane_focus_border_color")]
    pub focus_border_color: String,
    #[serde(default = "default_pane_separator_width")]
    pub separator_width: u32,
    #[serde(default = "default_pane_focus_border_width")]
    pub focus_border_width: u32,
    #[serde(default = "default_pane_separator_tolerance")]
    pub separator_tolerance: f32,
    #[serde(default = "default_scrollbar_thumb_opacity")]
    pub scrollbar_thumb_opacity: f32,
    #[serde(default = "default_scrollbar_track_opacity")]
    pub scrollbar_track_opacity: f32,
    /// How to determine the working directory for new panes and tabs.
    /// "inherit" = use the cwd of the current pane (default).
    /// "home" = always use $HOME.
    /// "fixed" = use `fixed_directory`.
    #[serde(default = "default_pane_working_directory")]
    pub working_directory: PaneWorkingDirectory,
    /// Fixed directory for new panes (used when `working_directory = "fixed"`).
    #[serde(default)]
    pub fixed_directory: String,
}

fn default_pane_separator_color() -> String { "#4D4D4D".into() }
fn default_pane_focus_border_color() -> String { "#3399FFCC".into() }
fn default_pane_separator_width() -> u32 { 2 }
fn default_pane_focus_border_width() -> u32 { 2 }
fn default_pane_separator_tolerance() -> f32 { 4.0 }
fn default_scrollbar_thumb_opacity() -> f32 { 0.5 }
fn default_scrollbar_track_opacity() -> f32 { 0.1 }

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            separator_color: default_pane_separator_color(),
            focus_border_color: default_pane_focus_border_color(),
            separator_width: default_pane_separator_width(),
            focus_border_width: default_pane_focus_border_width(),
            separator_tolerance: default_pane_separator_tolerance(),
            scrollbar_thumb_opacity: default_scrollbar_thumb_opacity(),
            scrollbar_track_opacity: default_scrollbar_track_opacity(),
            working_directory: default_pane_working_directory(),
            fixed_directory: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// [search]
// ---------------------------------------------------------------------------

/// Search bar configuration section (`[search]`).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_search_bar_bg")]
    pub bar_bg: String,
    #[serde(default = "default_search_input_fg")]
    pub input_fg: String,
    #[serde(default = "default_search_border_color")]
    pub border_color: String,
}

fn default_search_bar_bg() -> String { "#262633F2".into() }
fn default_search_input_fg() -> String { "#F2F2F2".into() }
fn default_search_border_color() -> String { "#4D4D66".into() }

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            bar_bg: default_search_bar_bg(),
            input_fg: default_search_input_fg(),
            border_color: default_search_border_color(),
        }
    }
}

// ---------------------------------------------------------------------------
// [palette]
// ---------------------------------------------------------------------------

/// Command palette configuration section (`[palette]`).
#[derive(Debug, Clone, Deserialize)]
pub struct PaletteConfig {
    #[serde(default = "default_palette_bg")]
    pub bg: String,
    #[serde(default = "default_palette_border_color")]
    pub border_color: String,
    #[serde(default = "default_palette_input_fg")]
    pub input_fg: String,
    #[serde(default = "default_palette_separator_color")]
    pub separator_color: String,
    #[serde(default = "default_palette_command_fg")]
    pub command_fg: String,
    #[serde(default = "default_palette_selected_bg")]
    pub selected_bg: String,
    #[serde(default = "default_palette_description_fg")]
    pub description_fg: String,
    #[serde(default = "default_palette_overlay_color")]
    pub overlay_color: String,
    #[serde(default = "default_palette_max_height")]
    pub max_height: f32,
    #[serde(default = "default_palette_max_visible_items")]
    pub max_visible_items: usize,
    #[serde(default = "default_palette_width_ratio")]
    pub width_ratio: f32,
    /// Corner radius in pixels for the SDF rounded rectangle background.
    #[serde(default = "default_palette_corner_radius")]
    pub corner_radius: f32,
    /// Gaussian blur radius in pixels for frosted-glass effect (0 = disabled).
    #[allow(dead_code)]
    #[serde(default = "default_palette_blur_radius")]
    pub blur_radius: f32,
    /// Drop shadow blur radius in pixels.
    #[serde(default = "default_palette_shadow_radius")]
    pub shadow_radius: f32,
    /// Drop shadow opacity (0.0 - 1.0).
    #[serde(default = "default_palette_shadow_opacity")]
    pub shadow_opacity: f32,
    /// Border width in pixels for the rounded rectangle outline.
    #[serde(default = "default_palette_border_width")]
    pub border_width: f32,
}

fn default_palette_bg() -> String { "#1F1F29F2".into() }
fn default_palette_border_color() -> String { "#4D4D66".into() }
fn default_palette_input_fg() -> String { "#F2F2F2".into() }
fn default_palette_separator_color() -> String { "#40404D".into() }
fn default_palette_command_fg() -> String { "#CCCCD1".into() }
fn default_palette_selected_bg() -> String { "#383852".into() }
fn default_palette_description_fg() -> String { "#808088".into() }
fn default_palette_overlay_color() -> String { "#00000080".into() }
fn default_palette_max_height() -> f32 { 400.0 }
fn default_palette_max_visible_items() -> usize { 10 }
fn default_palette_width_ratio() -> f32 { 0.6 }
fn default_palette_corner_radius() -> f32 { 12.0 }
fn default_palette_blur_radius() -> f32 { 20.0 }
fn default_palette_shadow_radius() -> f32 { 8.0 }
fn default_palette_shadow_opacity() -> f32 { 0.3 }
fn default_palette_border_width() -> f32 { 1.0 }

impl Default for PaletteConfig {
    fn default() -> Self {
        Self {
            bg: default_palette_bg(),
            border_color: default_palette_border_color(),
            input_fg: default_palette_input_fg(),
            separator_color: default_palette_separator_color(),
            command_fg: default_palette_command_fg(),
            selected_bg: default_palette_selected_bg(),
            description_fg: default_palette_description_fg(),
            overlay_color: default_palette_overlay_color(),
            max_height: default_palette_max_height(),
            max_visible_items: default_palette_max_visible_items(),
            width_ratio: default_palette_width_ratio(),
            corner_radius: default_palette_corner_radius(),
            blur_radius: default_palette_blur_radius(),
            shadow_radius: default_palette_shadow_radius(),
            shadow_opacity: default_palette_shadow_opacity(),
            border_width: default_palette_border_width(),
        }
    }
}

// ---------------------------------------------------------------------------
// [status_bar]
// ---------------------------------------------------------------------------

/// A single segment in the status bar with content, foreground, and background colors.
#[derive(Debug, Clone, Deserialize)]
pub struct StatusSegment {
    pub content: String,
    #[serde(default = "default_segment_fg")]
    pub fg: String,
    #[serde(default = "default_segment_bg")]
    pub bg: String,
}

fn default_segment_fg() -> String { "#CCCCCC".into() }
fn default_segment_bg() -> String { "#1A1A24".into() }

/// Status bar configuration section (`[status_bar]`).
#[derive(Debug, Clone, Deserialize)]
pub struct StatusBarConfig {
    #[serde(default = "default_status_enabled")]
    pub enabled: bool,
    #[serde(default = "default_status_height")]
    pub height: f32,
    #[serde(default = "default_status_bg")]
    pub background: String,
    #[allow(dead_code)]
    #[serde(default = "default_status_padding_x")]
    pub padding_x: f32,
    #[serde(default = "default_status_top_border")]
    pub top_border: bool,
    #[serde(default = "default_status_top_border_color")]
    pub top_border_color: String,
    #[serde(default = "default_left_segments")]
    pub left: Vec<StatusSegment>,
    #[serde(default = "default_right_segments")]
    pub right: Vec<StatusSegment>,
}

fn default_status_enabled() -> bool { true }
fn default_status_height() -> f32 { 28.0 }
fn default_status_bg() -> String { "#141420".into() }
fn default_status_padding_x() -> f32 { 8.0 }
fn default_status_top_border() -> bool { true }
fn default_status_top_border_color() -> String { "#2A2A34".into() }

fn default_left_segments() -> Vec<StatusSegment> {
    vec![
        StatusSegment { content: "{user}@{host}".into(), fg: "#FFFFFF".into(), bg: "#3A3AFF".into() },
        StatusSegment { content: "{cwd_short}".into(), fg: "#CCCCCC".into(), bg: "#2A2A34".into() },
        StatusSegment { content: "{git_branch} {git_status}".into(), fg: "#A6E3A1".into(), bg: "#1A1A24".into() },
    ]
}

fn default_right_segments() -> Vec<StatusSegment> {
    vec![
        StatusSegment { content: "{ports}".into(), fg: "#94E2D5".into(), bg: "#1A1A24".into() },
        StatusSegment { content: "{shell}".into(), fg: "#888888".into(), bg: "#2A2A34".into() },
        StatusSegment { content: "{pane_size}".into(), fg: "#888888".into(), bg: "#1A1A24".into() },
        StatusSegment { content: "{font_size}px".into(), fg: "#888888".into(), bg: "#2A2A34".into() },
        StatusSegment { content: "{time}".into(), fg: "#FFFFFF".into(), bg: "#3A3AFF".into() },
    ]
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: default_status_enabled(),
            height: default_status_height(),
            background: default_status_bg(),
            padding_x: default_status_padding_x(),
            top_border: default_status_top_border(),
            top_border_color: default_status_top_border_color(),
            left: default_left_segments(),
            right: default_right_segments(),
        }
    }
}

// ---------------------------------------------------------------------------
// [quick_terminal]
// ---------------------------------------------------------------------------

/// Quick Terminal (drop-down / quake-style) configuration section (`[quick_terminal]`).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct QuickTerminalConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_qt_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_qt_animation")]
    pub animation: String, // "slide_down", "slide_up", "fade", "none"

    #[serde(default = "default_qt_animation_duration")]
    pub animation_duration_ms: u32,

    #[serde(default = "default_qt_height_ratio")]
    pub height_ratio: f32,

    #[serde(default = "default_qt_width_ratio")]
    pub width_ratio: f32,

    #[serde(default = "default_qt_position")]
    pub position: String, // "left", "center", "right"

    #[serde(default = "default_qt_screen_edge")]
    pub screen_edge: String, // "top", "bottom"

    #[serde(default)]
    pub hide_on_focus_loss: bool,

    #[serde(default = "default_true")]
    pub dismiss_on_esc: bool,

    #[serde(default)]
    pub show_sidebar: bool,

    #[serde(default)]
    pub show_tab_bar: bool,

    #[serde(default = "default_true")]
    pub show_status_bar: bool,

    #[serde(default = "default_qt_window_level")]
    pub window_level: String, // "normal", "floating", "above_all"

    #[serde(default = "default_qt_corner_radius")]
    pub corner_radius: f32,

    #[serde(default = "default_true")]
    pub own_workspace: bool,
}

fn default_true() -> bool { true }
fn default_qt_hotkey() -> String { "ctrl+`".into() }
fn default_qt_animation() -> String { "slide_down".into() }
fn default_qt_animation_duration() -> u32 { 200 }
fn default_qt_height_ratio() -> f32 { 0.4 }
fn default_qt_width_ratio() -> f32 { 1.0 }
fn default_qt_position() -> String { "center".into() }
fn default_qt_screen_edge() -> String { "top".into() }
fn default_qt_window_level() -> String { "floating".into() }
fn default_qt_corner_radius() -> f32 { 12.0 }

impl Default for QuickTerminalConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            hotkey: default_qt_hotkey(),
            animation: default_qt_animation(),
            animation_duration_ms: default_qt_animation_duration(),
            height_ratio: default_qt_height_ratio(),
            width_ratio: default_qt_width_ratio(),
            position: default_qt_position(),
            screen_edge: default_qt_screen_edge(),
            hide_on_focus_loss: false,
            dismiss_on_esc: default_true(),
            show_sidebar: false,
            show_tab_bar: false,
            show_status_bar: default_true(),
            window_level: default_qt_window_level(),
            corner_radius: default_qt_corner_radius(),
            own_workspace: default_true(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Load the termojinal config from `~/.config/termojinal/config.toml`.
///
/// Returns the default configuration if the file does not exist or cannot be parsed.
pub fn load_config() -> TermojinalConfig {
    // Prefer XDG-style ~/.config/termojinal/ (common on macOS CLI tools),
    // fall back to dirs::config_dir() (~/Library/Application Support/ on macOS).
    let xdg_path = dirs::home_dir()
        .map(|h| h.join(".config").join("termojinal").join("config.toml"));
    let system_path = dirs::config_dir()
        .map(|d| d.join("termojinal").join("config.toml"));
    let path = match (&xdg_path, &system_path) {
        (Some(xdg), _) if xdg.exists() => xdg.clone(),
        (_, Some(sys)) if sys.exists() => sys.clone(),
        (Some(xdg), _) => xdg.clone(),
        (_, Some(sys)) => sys.clone(),
        _ => std::path::PathBuf::from("config.toml"),
    };
    log::info!("loading config from {}", path.display());
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<TermojinalConfig>(&content) {
            Ok(cfg) => {
                log::info!("config loaded successfully");
                cfg
            }
            Err(e) => {
                log::error!("config parse error: {e}");
                TermojinalConfig::default()
            }
        },
        Err(e) => {
            log::warn!("config file not found ({e}), using defaults");
            TermojinalConfig::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Theme file loading
// ---------------------------------------------------------------------------

/// Load a theme file from `~/.config/termojinal/themes/{name}.toml`.
///
/// Returns `None` if the file does not exist or cannot be parsed.
/// The theme file has the same structure as the `[theme]` section in config.toml.
pub fn load_theme_file(name: &str) -> Option<ThemeSection> {
    if name.is_empty() {
        return None;
    }
    let path = dirs::home_dir()?
        .join(".config")
        .join("termojinal")
        .join("themes")
        .join(format!("{name}.toml"));
    log::info!("loading theme file from {}", path.display());
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<ThemeSection>(&content) {
            Ok(theme) => {
                log::info!("theme '{}' loaded successfully", name);
                Some(theme)
            }
            Err(e) => {
                log::error!("theme '{}' parse error: {e}", name);
                None
            }
        },
        Err(e) => {
            log::warn!("theme file '{}' not found ({e})", name);
            None
        }
    }
}

/// System appearance (Dark or Light).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// Resolve the effective theme, applying auto-switch logic if enabled.
///
/// If `theme.auto_switch` is true and the appropriate theme file is set,
/// loads the theme file for the given appearance. Otherwise returns the
/// inline theme from config.
pub fn resolve_theme(config: &TermojinalConfig, appearance: Appearance) -> ThemeSection {
    let theme = &config.theme;
    if !theme.auto_switch {
        return theme.clone();
    }
    let name = match appearance {
        Appearance::Dark => &theme.dark,
        Appearance::Light => &theme.light,
    };
    if name.is_empty() {
        log::info!("auto_switch enabled but no {:?} theme file set, using inline theme", appearance);
        return theme.clone();
    }
    match load_theme_file(name) {
        Some(loaded) => loaded,
        None => {
            log::warn!("failed to load {:?} theme '{}', using inline theme", appearance, name);
            theme.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Tab title formatting
// ---------------------------------------------------------------------------

/// Format a tab title using the user's format string with fallback chains.
pub fn format_tab_title(format: &str, title: &str, cwd: &str, index: usize) -> String {
    let trimmed = format.trim();
    // Check if the format is a single top-level `{...}` fallback chain.
    // It may contain nested `{var}` inside alternatives, e.g. "{title|cwd_base|Tab {index}}".
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        // Find the matching closing brace for the opening one.
        let mut depth = 0;
        let mut end = 0;
        for (i, c) in trimmed.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 { end = i; break; }
                }
                _ => {}
            }
        }
        // If the entire string is one balanced `{...}`, treat as fallback chain.
        if end == trimmed.len() - 1 {
            let inner = &trimmed[1..end];
            let alternatives: Vec<&str> = inner.split('|').collect();
            for alt in &alternatives {
                let alt = alt.trim();
                let resolved = resolve_variable(alt, title, cwd, index);
                if !resolved.is_empty() {
                    return resolved;
                }
            }
            return format!("Tab {}", index);
        }
    }
    expand_variables(format, title, cwd, index)
}

fn resolve_variable(var: &str, title: &str, cwd: &str, index: usize) -> String {
    match var {
        "title" => title.to_string(),
        "cwd" => cwd.to_string(),
        "cwd_base" => std::path::Path::new(cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        "index" => index.to_string(),
        other => expand_variables(other, title, cwd, index),
    }
}

fn expand_variables(s: &str, title: &str, cwd: &str, index: usize) -> String {
    let cwd_base = std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("~");
    s.replace("{title}", title)
        .replace("{cwd}", cwd)
        .replace("{cwd_base}", cwd_base)
        .replace("{index}", &index.to_string())
}
