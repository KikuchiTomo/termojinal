//! Keybinding system with 3-layer configuration.
//!
//! Layers:
//! - **normal**: Active when termojinal is focused and a regular shell is running.
//! - **global**: Active even when termojinal is not focused (via CGEventTap on macOS).
//! - **alternate_screen**: Active when a TUI application (e.g., nvim) is running.
//!
//! Keybindings are loaded from `~/.config/termojinal/keybindings.toml` with
//! sensible defaults matching Ghostty-compatible bindings.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// An action that a keybinding can trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Split the current pane to the right.
    SplitRight,
    /// Split the current pane downward.
    SplitDown,
    /// Toggle zoom on the current pane.
    ZoomPane,
    /// Focus the next pane.
    NextPane,
    /// Focus the previous pane.
    PrevPane,
    /// Create a new tab in the current workspace.
    NewTab,
    /// Close the current pane (cascades to tab/workspace/app).
    CloseTab,
    /// Create a new workspace.
    NewWorkspace,
    /// Switch to next tab within the current workspace.
    NextTab,
    /// Switch to previous tab within the current workspace.
    PrevTab,
    /// Switch to workspace N (1-9).
    Workspace(u8),
    /// Open the command palette.
    CommandPalette,
    /// Open the AllowFlow AI panel.
    AllowFlowPanel,
    /// Jump to the next unread notification.
    UnreadJump,
    /// Increase font size.
    FontIncrease,
    /// Decrease font size.
    FontDecrease,
    /// Copy selection to clipboard.
    Copy,
    /// Paste from clipboard.
    Paste,
    /// Open search.
    Search,
    /// Open settings.
    OpenSettings,
    /// Clear the screen (send ESC[2J ESC[H to PTY).
    ClearScreen,
    /// Clear scrollback + screen.
    ClearScrollback,
    /// Select all visible text in the terminal.
    SelectAll,
    /// Force the key through to the PTY (bypass termojinal).
    Passthrough,
    /// Quit the application.
    Quit,
    /// Toggle the sidebar.
    ToggleSidebar,
    /// Switch to next workspace.
    NextWorkspace,
    /// Switch to previous workspace.
    PrevWorkspace,
    /// Toggle the Quick Terminal visor window.
    ToggleQuickTerminal,
    /// Ignore the key entirely.
    None,
    /// Show the About screen (license, credits, version).
    About,
    /// Toggle the directory tree in the sidebar.
    ToggleDirectoryTree,
    /// Run a named command or plugin.
    Command(String),
    /// Jump to the previous command output (time travel).
    PrevCommand,
    /// Jump to the next command output (time travel).
    NextCommand,
    /// Jump to the very first command in history.
    FirstCommand,
    /// Jump to the latest (live) view from time travel.
    LastCommand,
    /// Open the command timeline UI.
    CommandTimeline,
    /// Create a named snapshot of the current terminal state.
    CreateSnapshot,
}

/// 3-layer keybinding configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingConfig {
    /// Layer 1: keybindings active during normal shell usage.
    #[serde(default)]
    pub normal: HashMap<String, Action>,

    /// Layer 2: keybindings active even when termojinal is not focused.
    #[serde(default)]
    pub global: HashMap<String, Action>,

    /// Layer 3: keybindings active when an alternate-screen TUI is running.
    #[serde(default)]
    pub alternate_screen: HashMap<String, Action>,
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        let mut normal = HashMap::new();

        // Pane management
        normal.insert("cmd+d".to_string(), Action::SplitRight);
        normal.insert("cmd+shift+d".to_string(), Action::SplitDown);
        normal.insert("cmd+shift+enter".to_string(), Action::ZoomPane);
        normal.insert("cmd+]".to_string(), Action::NextPane);
        normal.insert("cmd+[".to_string(), Action::PrevPane);

        // Tab management
        normal.insert("cmd+t".to_string(), Action::NewTab);
        normal.insert("cmd+w".to_string(), Action::CloseTab);
        normal.insert("cmd+n".to_string(), Action::NewWorkspace);

        // Workspace switching (Cmd+1 through Cmd+9)
        for i in 1u8..=9 {
            normal.insert(format!("cmd+{i}"), Action::Workspace(i));
        }

        // Command palette
        normal.insert("cmd+shift+p".to_string(), Action::CommandPalette);

        // Settings
        normal.insert("cmd+,".to_string(), Action::OpenSettings);

        // Copy / Paste
        normal.insert("cmd+c".to_string(), Action::Copy);
        normal.insert("cmd+v".to_string(), Action::Paste);

        // Select All
        normal.insert("cmd+a".to_string(), Action::SelectAll);

        // Search
        normal.insert("cmd+f".to_string(), Action::Search);

        // Clear
        normal.insert("cmd+k".to_string(), Action::ClearScrollback);
        normal.insert("cmd+l".to_string(), Action::ClearScreen);

        // Font sizing
        normal.insert("cmd+=".to_string(), Action::FontIncrease);
        normal.insert("cmd+-".to_string(), Action::FontDecrease);

        // Quit
        normal.insert("cmd+q".to_string(), Action::Quit);

        // Sidebar toggle
        normal.insert("cmd+b".to_string(), Action::ToggleSidebar);

        // Directory tree toggle
        normal.insert("cmd+shift+e".to_string(), Action::ToggleDirectoryTree);

        // Tab navigation (Cmd+Shift+{ and Cmd+Shift+} produce these characters)
        normal.insert("cmd+shift+{".to_string(), Action::PrevTab);
        normal.insert("cmd+shift+}".to_string(), Action::NextTab);

        // Workspace navigation
        normal.insert("cmd+shift+[".to_string(), Action::PrevWorkspace);
        normal.insert("cmd+shift+]".to_string(), Action::NextWorkspace);

        // Time travel: command navigation
        normal.insert("cmd+up".to_string(), Action::PrevCommand);
        normal.insert("cmd+down".to_string(), Action::NextCommand);
        normal.insert("cmd+shift+up".to_string(), Action::FirstCommand);
        normal.insert("cmd+shift+down".to_string(), Action::LastCommand);

        // Time travel: timeline UI
        normal.insert("cmd+shift+t".to_string(), Action::CommandTimeline);

        // Time travel: named snapshots (no default keybinding until fully implemented)

        // Global keybindings (active even when termojinal is not focused).
        let mut global = HashMap::new();
        global.insert("ctrl+`".to_string(), Action::ToggleQuickTerminal);

        Self {
            normal,
            global,
            alternate_screen: HashMap::new(),
        }
    }
}

impl KeybindingConfig {
    /// Load keybinding config from the default path
    /// (`~/.config/termojinal/keybindings.toml`), falling back to defaults
    /// if the file does not exist.
    pub fn load() -> Self {
        match Self::config_path() {
            Some(path) if path.exists() => match Self::load_from(&path) {
                Ok(config) => config,
                Err(e) => {
                    log::warn!(
                        "failed to load keybindings from {}: {e}; using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            _ => Self::default(),
        }
    }

    /// Load keybinding config from a specific file path.
    ///
    /// The file is parsed as TOML. Any keys not present in the file
    /// will use their default values.
    pub fn load_from(path: &std::path::Path) -> Result<Self, KeybindingError> {
        let contents = std::fs::read_to_string(path)?;
        Self::parse_toml(&contents)
    }

    /// Parse a TOML string into a keybinding config.
    ///
    /// The parsed config is merged with defaults: user-specified bindings
    /// override defaults, but unspecified defaults are preserved.
    pub fn parse_toml(toml_str: &str) -> Result<Self, KeybindingError> {
        let user: KeybindingConfig = toml::from_str(toml_str)?;
        let mut config = Self::default();

        // User overrides win; merge into defaults.
        for (key, action) in user.normal {
            config.normal.insert(key, action);
        }
        for (key, action) in user.global {
            config.global.insert(key, action);
        }
        for (key, action) in user.alternate_screen {
            config.alternate_screen.insert(key, action);
        }

        Ok(config)
    }

    /// Look up an action in the normal layer.
    pub fn lookup_normal(&self, key: &str) -> Option<&Action> {
        self.normal.get(key)
    }

    /// Look up an action in the global layer.
    pub fn lookup_global(&self, key: &str) -> Option<&Action> {
        self.global.get(key)
    }

    /// Look up an action in the alternate screen layer.
    pub fn lookup_alternate_screen(&self, key: &str) -> Option<&Action> {
        self.alternate_screen.get(key)
    }

    /// Get the default config file path.
    /// Prefers `~/.config/termojinal/` (XDG-style) over `dirs::config_dir()`
    /// (which returns `~/Library/Application Support/` on macOS).
    pub fn config_path() -> Option<PathBuf> {
        let xdg = dirs::home_dir().map(|h| h.join(".config").join("termojinal").join("keybindings.toml"));
        let system = dirs::config_dir().map(|d| d.join("termojinal").join("keybindings.toml"));
        match (&xdg, &system) {
            (Some(x), _) if x.exists() => xdg,
            (_, Some(s)) if s.exists() => system,
            (Some(_), _) => xdg,
            _ => system,
        }
    }
}

/// Errors that can occur when loading keybinding config.
#[derive(Debug, thiserror::Error)]
pub enum KeybindingError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_exist() {
        let config = KeybindingConfig::default();
        assert_eq!(
            config.lookup_normal("cmd+d"),
            Some(&Action::SplitRight)
        );
        assert_eq!(
            config.lookup_normal("cmd+shift+d"),
            Some(&Action::SplitDown)
        );
        assert_eq!(
            config.lookup_normal("cmd+shift+enter"),
            Some(&Action::ZoomPane)
        );
        assert_eq!(
            config.lookup_normal("cmd+t"),
            Some(&Action::NewTab)
        );
        assert_eq!(
            config.lookup_normal("cmd+w"),
            Some(&Action::CloseTab)
        );
        assert_eq!(
            config.lookup_normal("cmd+1"),
            Some(&Action::Workspace(1))
        );
        assert_eq!(
            config.lookup_normal("cmd+9"),
            Some(&Action::Workspace(9))
        );
        assert_eq!(
            config.lookup_normal("cmd+shift+p"),
            Some(&Action::CommandPalette)
        );
        assert_eq!(
            config.lookup_normal("cmd+,"),
            Some(&Action::OpenSettings)
        );
        assert_eq!(
            config.lookup_normal("cmd+c"),
            Some(&Action::Copy)
        );
        assert_eq!(
            config.lookup_normal("cmd+v"),
            Some(&Action::Paste)
        );
        assert_eq!(
            config.lookup_normal("cmd+f"),
            Some(&Action::Search)
        );
        assert_eq!(
            config.lookup_normal("cmd+="),
            Some(&Action::FontIncrease)
        );
        assert_eq!(
            config.lookup_normal("cmd+-"),
            Some(&Action::FontDecrease)
        );
    }

    #[test]
    fn test_workspace_bindings() {
        let config = KeybindingConfig::default();
        for i in 1u8..=9 {
            let key = format!("cmd+{i}");
            assert_eq!(
                config.lookup_normal(&key),
                Some(&Action::Workspace(i)),
                "workspace binding for {key}"
            );
        }
    }

    #[test]
    fn test_global_layer_has_quick_terminal_by_default() {
        let config = KeybindingConfig::default();
        assert_eq!(
            config.lookup_global("ctrl+`"),
            Some(&Action::ToggleQuickTerminal)
        );
    }

    #[test]
    fn test_alternate_screen_layer_empty_by_default() {
        let config = KeybindingConfig::default();
        assert!(config.alternate_screen.is_empty());
    }

    #[test]
    fn test_parse_toml_override() {
        let toml = r#"
[normal]
"cmd+d" = "new_tab"
"cmd+x" = { "command" = "my_plugin" }

[global]
"cmd+shift+space" = "command_palette"

[alternate_screen]
"cmd+c" = "passthrough"
"#;
        let config = KeybindingConfig::parse_toml(toml).unwrap();

        // User override replaces default.
        assert_eq!(
            config.lookup_normal("cmd+d"),
            Some(&Action::NewTab)
        );

        // User-added binding is present.
        assert_eq!(
            config.lookup_normal("cmd+x"),
            Some(&Action::Command("my_plugin".to_string()))
        );

        // Other defaults are still present.
        assert_eq!(
            config.lookup_normal("cmd+t"),
            Some(&Action::NewTab)
        );

        // Global layer populated.
        assert_eq!(
            config.lookup_global("cmd+shift+space"),
            Some(&Action::CommandPalette)
        );

        // Alternate screen layer populated.
        assert_eq!(
            config.lookup_alternate_screen("cmd+c"),
            Some(&Action::Passthrough)
        );
    }

    #[test]
    fn test_parse_toml_empty() {
        let config = KeybindingConfig::parse_toml("").unwrap();
        // Should still have all defaults.
        assert_eq!(
            config.lookup_normal("cmd+d"),
            Some(&Action::SplitRight)
        );
    }

    #[test]
    fn test_parse_toml_partial() {
        let toml = r#"
[normal]
"cmd+d" = "split_down"
"#;
        let config = KeybindingConfig::parse_toml(toml).unwrap();
        assert_eq!(
            config.lookup_normal("cmd+d"),
            Some(&Action::SplitDown)
        );
        // Other defaults remain.
        assert_eq!(
            config.lookup_normal("cmd+t"),
            Some(&Action::NewTab)
        );
    }

    #[test]
    fn test_parse_toml_invalid() {
        let result = KeybindingConfig::parse_toml("not valid {{ toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_action_serialization_roundtrip() {
        let actions = vec![
            Action::SplitRight,
            Action::SplitDown,
            Action::ZoomPane,
            Action::NextPane,
            Action::PrevPane,
            Action::NewTab,
            Action::CloseTab,
            Action::NewWorkspace,
            Action::NextTab,
            Action::PrevTab,
            Action::Workspace(3),
            Action::CommandPalette,
            Action::AllowFlowPanel,
            Action::UnreadJump,
            Action::FontIncrease,
            Action::FontDecrease,
            Action::Copy,
            Action::Paste,
            Action::Search,
            Action::OpenSettings,
            Action::SelectAll,
            Action::Passthrough,
            Action::Quit,
            Action::ToggleSidebar,
            Action::NextWorkspace,
            Action::PrevWorkspace,
            Action::ToggleQuickTerminal,
            Action::None,
            Action::Command("test_cmd".to_string()),
            Action::PrevCommand,
            Action::NextCommand,
            Action::FirstCommand,
            Action::LastCommand,
            Action::CommandTimeline,
            Action::CreateSnapshot,
        ];

        for action in &actions {
            let json = serde_json::to_string(action).unwrap();
            let deserialized: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(action, &deserialized, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn test_lookup_missing_key() {
        let config = KeybindingConfig::default();
        assert_eq!(config.lookup_normal("ctrl+alt+delete"), Option::None);
        assert_eq!(config.lookup_global("cmd+d"), Option::None);
        assert_eq!(config.lookup_alternate_screen("cmd+d"), Option::None);
    }

    #[test]
    fn test_none_action() {
        let toml = r#"
[normal]
"cmd+q" = "none"
"#;
        let config = KeybindingConfig::parse_toml(toml).unwrap();
        assert_eq!(config.lookup_normal("cmd+q"), Some(&Action::None));
    }

    #[test]
    fn test_passthrough_action() {
        let toml = r#"
[alternate_screen]
"cmd+c" = "passthrough"
"cmd+v" = "passthrough"
"#;
        let config = KeybindingConfig::parse_toml(toml).unwrap();
        assert_eq!(
            config.lookup_alternate_screen("cmd+c"),
            Some(&Action::Passthrough)
        );
        assert_eq!(
            config.lookup_alternate_screen("cmd+v"),
            Some(&Action::Passthrough)
        );
    }
}
