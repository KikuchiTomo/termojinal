//! Configuration loading for jterm.
//!
//! Loads settings from `~/.config/jterm/config.toml` with sane defaults.

use serde::Deserialize;

/// Top-level jterm configuration.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct JtermConfig {
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
}

impl Default for JtermConfig {
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
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: parse hex color
// ---------------------------------------------------------------------------

/// Parse a hex color string (#RRGGBB or #RRGGBBAA) to [f32; 4] RGBA.
pub fn parse_hex_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
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
}

fn default_bg() -> String { "#11111A".into() }
fn default_fg() -> String { "#D9D9D9".into() }
fn default_cursor_color() -> String { "#D9D9D9".into() }
fn default_selection_bg() -> String { "#3A3A50".into() }
fn default_preedit_bg() -> String { "#262633".into() }
fn default_search_highlight_bg() -> String { "#998019".into() }
fn default_search_highlight_fg() -> String { "#000000".into() }
fn default_bold_brightness() -> f32 { 1.2 }
fn default_dim_opacity() -> f32 { 0.6 }

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
fn default_font_size() -> f32 { 16.0 }
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
}

fn default_sidebar_width() -> f32 { 240.0 }
fn default_sidebar_min_width() -> f32 { 120.0 }
fn default_sidebar_max_width() -> f32 { 400.0 }
fn default_sidebar_bg() -> String { "#0D0D12".into() }
fn default_sidebar_active_entry_bg() -> String { "#1A1A24".into() }
fn default_sidebar_active_fg() -> String { "#F2F2F8".into() }
fn default_sidebar_inactive_fg() -> String { "#8C8C99".into() }
fn default_sidebar_dim_fg() -> String { "#666670".into() }
fn default_sidebar_git_branch_fg() -> String { "#5AB3D9".into() }
fn default_sidebar_separator_color() -> String { "#333338".into() }
fn default_sidebar_notification_dot() -> String { "#FF941A".into() }
fn default_sidebar_git_dirty_color() -> String { "#CCB34D".into() }
fn default_sidebar_top_pad() -> f32 { 8.0 }
fn default_sidebar_side_pad() -> f32 { 10.0 }
fn default_sidebar_entry_gap() -> f32 { 12.0 }
fn default_sidebar_info_line_gap() -> f32 { 4.0 }

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
fn default_tab_padding_x() -> f32 { 8.0 }
fn default_tab_padding_y() -> f32 { 4.0 }
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
        StatusSegment { content: " {user}@{host} ".into(), fg: "#FFFFFF".into(), bg: "#3A3AFF".into() },
        StatusSegment { content: " {cwd_short} ".into(), fg: "#CCCCCC".into(), bg: "#2A2A34".into() },
        StatusSegment { content: " {git_branch} {git_status} ".into(), fg: "#A6E3A1".into(), bg: "#1A1A24".into() },
    ]
}

fn default_right_segments() -> Vec<StatusSegment> {
    vec![
        StatusSegment { content: " {ports} ".into(), fg: "#94E2D5".into(), bg: "#1A1A24".into() },
        StatusSegment { content: " {shell} ".into(), fg: "#888888".into(), bg: "#2A2A34".into() },
        StatusSegment { content: " {pane_size} ".into(), fg: "#888888".into(), bg: "#1A1A24".into() },
        StatusSegment { content: " {font_size}px ".into(), fg: "#888888".into(), bg: "#2A2A34".into() },
        StatusSegment { content: " {time} ".into(), fg: "#FFFFFF".into(), bg: "#3A3AFF".into() },
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
// Config loading
// ---------------------------------------------------------------------------

/// Load the jterm config from `~/.config/jterm/config.toml`.
///
/// Returns the default configuration if the file does not exist or cannot be parsed.
pub fn load_config() -> JtermConfig {
    // Prefer XDG-style ~/.config/jterm/ (common on macOS CLI tools),
    // fall back to dirs::config_dir() (~/Library/Application Support/ on macOS).
    let xdg_path = dirs::home_dir()
        .map(|h| h.join(".config").join("jterm").join("config.toml"));
    let system_path = dirs::config_dir()
        .map(|d| d.join("jterm").join("config.toml"));
    let path = match (&xdg_path, &system_path) {
        (Some(xdg), _) if xdg.exists() => xdg.clone(),
        (_, Some(sys)) if sys.exists() => sys.clone(),
        (Some(xdg), _) => xdg.clone(),
        (_, Some(sys)) => sys.clone(),
        _ => std::path::PathBuf::from("config.toml"),
    };
    log::info!("loading config from {}", path.display());
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<JtermConfig>(&content) {
            Ok(cfg) => {
                log::info!("config loaded successfully");
                cfg
            }
            Err(e) => {
                log::error!("config parse error: {e}");
                JtermConfig::default()
            }
        },
        Err(e) => {
            log::warn!("config file not found ({e}), using defaults");
            JtermConfig::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Tab title formatting
// ---------------------------------------------------------------------------

/// Format a tab title using the user's format string with fallback chains.
pub fn format_tab_title(format: &str, title: &str, cwd: &str, index: usize) -> String {
    let trimmed = format.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.matches('{').count() == 1 {
        let inner = &trimmed[1..trimmed.len() - 1];
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
