//! Configuration loading for jterm.
//!
//! Loads settings from `~/.config/jterm/config.toml` with sane defaults.

use serde::Deserialize;

/// Top-level jterm configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct JtermConfig {
    #[serde(default)]
    pub font: FontSection,
    #[serde(default)]
    pub window: WindowSection,
    #[serde(default)]
    pub theme: ThemeSection,
    #[serde(default)]
    pub tab_bar: TabBarConfig,
}

impl Default for JtermConfig {
    fn default() -> Self {
        Self {
            font: FontSection::default(),
            window: WindowSection::default(),
            theme: ThemeSection::default(),
            tab_bar: TabBarConfig::default(),
        }
    }
}

/// Theme/color configuration section (`[theme]`).
#[derive(Debug, Clone, Deserialize)]
pub struct ThemeSection {
    #[serde(default = "default_bg")]
    pub background: String,
    #[serde(default = "default_fg")]
    pub foreground: String,
    #[serde(default = "default_cursor_color")]
    pub cursor: String,
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,
}

fn default_bg() -> String { "#11111A".into() }
fn default_fg() -> String { "#D9D9D9".into() }
fn default_cursor_color() -> String { "#D9D9D9".into() }
fn default_selection_bg() -> String { "#3A3A50".into() }

impl Default for ThemeSection {
    fn default() -> Self {
        Self {
            background: default_bg(),
            foreground: default_fg(),
            cursor: default_cursor_color(),
            selection_bg: default_selection_bg(),
        }
    }
}

/// Parse a hex color string (#RRGGBB) to [f32; 4] RGBA.
pub fn parse_hex_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 { return None; }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
}

/// Font configuration section (`[font]`).
#[derive(Debug, Clone, Deserialize)]
pub struct FontSection {
    #[serde(default = "default_font_family")]
    pub family: String,
    #[serde(default = "default_font_size")]
    pub size: f32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
}

fn default_font_family() -> String { "monospace".into() }
fn default_font_size() -> f32 { 16.0 }
fn default_line_height() -> f32 { 1.2 }

impl Default for FontSection {
    fn default() -> Self {
        Self {
            family: default_font_family(),
            size: default_font_size(),
            line_height: default_line_height(),
        }
    }
}

/// Window configuration section (`[window]`).
#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
}

fn default_width() -> u32 { 960 }
fn default_height() -> u32 { 640 }
fn default_opacity() -> f32 { 1.0 }
fn default_padding_x() -> f32 { 1.0 }
fn default_padding_y() -> f32 { 0.5 }
fn default_sidebar_width() -> f32 { 200.0 }

impl Default for WindowSection {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            opacity: default_opacity(),
            padding_x: default_padding_x(),
            padding_y: default_padding_y(),
            sidebar_width: default_sidebar_width(),
        }
    }
}

/// Tab bar configuration section (`[tab_bar]`).
#[derive(Debug, Clone, Deserialize)]
pub struct TabBarConfig {
    /// Format string for tab title.
    ///
    /// Available variables: `{title}`, `{cwd}`, `{cwd_base}`, `{pid}`, `{index}`.
    /// Use `|` as a fallback separator — first non-empty value wins.
    /// Example: `"{title|cwd_base|Tab {index}}"`.
    #[serde(default = "default_tab_format")]
    pub format: String,

    /// Show the tab bar even when a workspace has a single tab.
    #[serde(default)]
    pub always_show: bool,

    /// Tab bar position: `"top"` or `"bottom"`.
    #[allow(dead_code)]
    #[serde(default = "default_tab_position")]
    pub position: String,

    /// Maximum tab width in pixels.
    #[serde(default = "default_max_width")]
    pub max_width: f32,
}

fn default_tab_format() -> String {
    "{title|cwd_base|Tab {index}}".into()
}
fn default_tab_position() -> String {
    "top".into()
}
fn default_max_width() -> f32 {
    200.0
}

impl Default for TabBarConfig {
    fn default() -> Self {
        Self {
            format: default_tab_format(),
            always_show: false,
            position: default_tab_position(),
            max_width: default_max_width(),
        }
    }
}

/// Load the jterm config from `~/.config/jterm/config.toml`.
///
/// Returns the default configuration if the file does not exist or cannot be parsed.
pub fn load_config() -> JtermConfig {
    let path = dirs::config_dir()
        .unwrap_or_default()
        .join("jterm")
        .join("config.toml");
    if let Ok(content) = std::fs::read_to_string(&path) {
        toml::from_str(&content).unwrap_or_default()
    } else {
        JtermConfig::default()
    }
}

/// Format a tab title using the user's format string with fallback chains.
///
/// The format string supports `|`-separated fallback chains within `{}`.
/// For example, `"{title|cwd_base|Tab {index}}"` tries `title` first,
/// then `cwd_base`, then the literal `"Tab {index}"` (with `{index}` expanded).
pub fn format_tab_title(format: &str, title: &str, cwd: &str, index: usize) -> String {
    // Check if the format is a single `{...}` (common case).
    let trimmed = format.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.matches('{').count() == 1 {
        // Simple fallback chain: {a|b|c}
        let inner = &trimmed[1..trimmed.len() - 1];
        let alternatives: Vec<&str> = inner.split('|').collect();
        for alt in &alternatives {
            let alt = alt.trim();
            let resolved = resolve_variable(alt, title, cwd, index);
            if !resolved.is_empty() {
                return resolved;
            }
        }
        // All alternatives empty — return the last one literally expanded.
        return format!("Tab {}", index);
    }

    // General case: expand variables in the format string.
    expand_variables(format, title, cwd, index)
}

/// Resolve a single variable name to its value.
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
        other => {
            // Could be a literal with embedded variables, e.g. "Tab {index}".
            expand_variables(other, title, cwd, index)
        }
    }
}

/// Expand `{variable}` placeholders in a string.
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
