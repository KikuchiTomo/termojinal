//! Command discovery and metadata loading.
//!
//! Commands are external scripts stored under `~/.config/termojinal/commands/`.
//! Each command lives in its own directory and has a `command.toml` file
//! describing its metadata and entry point.
//!
//! Directory layout:
//! ```text
//! ~/.config/termojinal/commands/<name>/
//! ├── command.toml   ← metadata
//! └── run.sh         ← executable script (any language)
//! ```

use crate::command_signer::{self, VerifyResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Metadata parsed from a `command.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandMeta {
    /// Human-readable command name shown in the command palette.
    pub name: String,

    /// Short description of what the command does.
    pub description: String,

    /// SF Symbol name for the command icon.
    #[serde(default)]
    pub icon: String,

    /// Semantic version string.
    #[serde(default)]
    pub version: String,

    /// Author name or identifier.
    #[serde(default)]
    pub author: String,

    /// Relative path to the executable script (relative to `command.toml`).
    pub run: String,

    /// Tags for search and categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Reserved for a future code-signing / trust system.
    #[serde(default)]
    pub signature: Option<String>,
}

/// TOML file structure: the `[command]` table wraps [`CommandMeta`].
#[derive(Debug, Deserialize)]
struct CommandToml {
    command: CommandMeta,
}

/// A fully-resolved command ready to be executed.
#[derive(Debug, Clone)]
pub struct LoadedCommand {
    /// Parsed metadata from `command.toml`.
    pub meta: CommandMeta,

    /// The directory containing `command.toml`.
    pub dir: PathBuf,

    /// Absolute path to the run script.
    pub run_path: PathBuf,

    /// Result of signature verification.
    pub verify_result: VerifyResult,
}

/// Scan `~/.config/termojinal/commands/` and load all valid command definitions.
///
/// Invalid or malformed commands are logged and skipped.
pub fn load_commands() -> Vec<LoadedCommand> {
    let commands_dir = match default_commands_dir() {
        Some(dir) => dir,
        None => {
            log::debug!("could not determine commands directory");
            return Vec::new();
        }
    };

    if !commands_dir.exists() {
        log::debug!(
            "commands directory does not exist: {}",
            commands_dir.display()
        );
        return Vec::new();
    }

    load_commands_from(&commands_dir)
}

/// Load command definitions from an arbitrary directory.
///
/// Each immediate subdirectory that contains a `command.toml` is treated
/// as a command. Subdirectories without a `command.toml` are silently
/// skipped; parse errors are logged as warnings.
pub fn load_commands_from(dir: &Path) -> Vec<LoadedCommand> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("failed to read commands directory {}: {e}", dir.display());
            return Vec::new();
        }
    };

    let mut commands = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("failed to read directory entry: {e}");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let toml_path = path.join("command.toml");
        if !toml_path.exists() {
            continue;
        }

        match load_single_command(&path, &toml_path) {
            Ok(cmd) => commands.push(cmd),
            Err(e) => {
                log::warn!("failed to load command from {}: {e}", path.display());
            }
        }
    }

    // Sort by name for deterministic ordering.
    commands.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));

    commands
}

/// Load a single command from its directory and `command.toml` path.
fn load_single_command(dir: &Path, toml_path: &Path) -> Result<LoadedCommand, CommandLoaderError> {
    let contents = std::fs::read_to_string(toml_path)?;
    let parsed: CommandToml = toml::from_str(&contents)?;
    let meta = parsed.command;

    let run_path = dir.join(&meta.run);
    if !run_path.exists() {
        return Err(CommandLoaderError::MissingRunScript(
            run_path.display().to_string(),
        ));
    }

    let verify_result = command_signer::verify_command(&contents, meta.signature.as_deref());

    Ok(LoadedCommand {
        meta,
        dir: dir.to_path_buf(),
        run_path,
        verify_result,
    })
}

/// Get the default commands directory (`~/.config/termojinal/commands/`).
pub fn default_commands_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("termojinal").join("commands"))
}

/// Errors that can occur when loading commands.
#[derive(Debug, thiserror::Error)]
pub enum CommandLoaderError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("run script not found: {0}")]
    MissingRunScript(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary command directory with a `command.toml` and a run script.
    fn create_test_command(base: &Path, name: &str, toml_content: &str, run_name: &str) {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("command.toml"), toml_content).unwrap();
        fs::write(dir.join(run_name), "#!/bin/sh\necho hello").unwrap();
    }

    #[test]
    fn test_parse_command_meta_full() {
        let toml_str = r#"
[command]
name = "Start PR Review"
description = "Review待ちPRを選んでworktree + Claude Codeを起動"
icon = "arrow.triangle.branch"
version = "1.0.0"
author = "termojinal"
run = "./run.ts"
tags = ["github", "review", "claude"]
"#;

        let parsed: CommandToml = toml::from_str(toml_str).unwrap();
        let meta = parsed.command;

        assert_eq!(meta.name, "Start PR Review");
        assert_eq!(
            meta.description,
            "Review待ちPRを選んでworktree + Claude Codeを起動"
        );
        assert_eq!(meta.icon, "arrow.triangle.branch");
        assert_eq!(meta.version, "1.0.0");
        assert_eq!(meta.author, "termojinal");
        assert_eq!(meta.run, "./run.ts");
        assert_eq!(meta.tags, vec!["github", "review", "claude"]);
        assert!(meta.signature.is_none());
    }

    #[test]
    fn test_parse_command_meta_minimal() {
        let toml_str = r#"
[command]
name = "Hello"
description = "A test command"
run = "./run.sh"
"#;

        let parsed: CommandToml = toml::from_str(toml_str).unwrap();
        let meta = parsed.command;

        assert_eq!(meta.name, "Hello");
        assert_eq!(meta.description, "A test command");
        assert_eq!(meta.run, "./run.sh");
        assert_eq!(meta.icon, "");
        assert_eq!(meta.version, "");
        assert_eq!(meta.author, "");
        assert!(meta.tags.is_empty());
        assert!(meta.signature.is_none());
    }

    #[test]
    fn test_parse_command_meta_with_signature() {
        let toml_str = r#"
[command]
name = "Signed"
description = "A signed command"
run = "./run.sh"
signature = "abc123"
"#;

        let parsed: CommandToml = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.command.signature.as_deref(), Some("abc123"));
    }

    #[test]
    fn test_parse_command_meta_missing_name() {
        let toml_str = r#"
[command]
description = "No name"
run = "./run.sh"
"#;
        let result = toml::from_str::<CommandToml>(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_command_meta_missing_run() {
        let toml_str = r#"
[command]
name = "No run"
description = "Missing run field"
"#;
        let result = toml::from_str::<CommandToml>(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_commands_from_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        create_test_command(
            base,
            "hello",
            r#"
[command]
name = "Hello World"
description = "Greets the user"
run = "./run.sh"
tags = ["demo"]
"#,
            "run.sh",
        );

        create_test_command(
            base,
            "goodbye",
            r#"
[command]
name = "Goodbye"
description = "Says goodbye"
run = "./run.sh"
"#,
            "run.sh",
        );

        let commands = load_commands_from(base);
        assert_eq!(commands.len(), 2);

        // Sorted by name.
        assert_eq!(commands[0].meta.name, "Goodbye");
        assert_eq!(commands[1].meta.name, "Hello World");

        // Paths are resolved correctly.
        assert!(commands[0].run_path.ends_with("run.sh"));
        assert!(commands[0].dir.ends_with("goodbye"));
    }

    #[test]
    fn test_load_commands_skips_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // Valid command.
        create_test_command(
            base,
            "valid",
            r#"
[command]
name = "Valid"
description = "Works fine"
run = "./run.sh"
"#,
            "run.sh",
        );

        // Invalid TOML.
        let bad_dir = base.join("invalid");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("command.toml"), "not valid {{ toml").unwrap();

        // Missing run script.
        let missing_dir = base.join("missing-run");
        fs::create_dir_all(&missing_dir).unwrap();
        fs::write(
            missing_dir.join("command.toml"),
            r#"
[command]
name = "Missing"
description = "No run script"
run = "./nonexistent.sh"
"#,
        )
        .unwrap();

        // Directory without command.toml (silently skipped).
        let empty_dir = base.join("no-toml");
        fs::create_dir_all(&empty_dir).unwrap();

        // Regular file (not a directory, silently skipped).
        fs::write(base.join("a-file.txt"), "not a command").unwrap();

        let commands = load_commands_from(base);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].meta.name, "Valid");
    }

    #[test]
    fn test_load_commands_from_nonexistent_directory() {
        let commands = load_commands_from(Path::new("/nonexistent/path"));
        assert!(commands.is_empty());
    }

    #[test]
    fn test_command_meta_serialization_roundtrip() {
        let meta = CommandMeta {
            name: "Test".to_string(),
            description: "A test".to_string(),
            icon: "star".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            run: "./run.sh".to_string(),
            tags: vec!["a".to_string(), "b".to_string()],
            signature: Some("sig".to_string()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: CommandMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn test_default_commands_dir() {
        // Should return Some on any system with a home directory.
        let dir = default_commands_dir();
        assert!(dir.is_some());
        let dir = dir.unwrap();
        assert!(dir.ends_with("commands"));
        assert!(dir.to_string_lossy().contains(".config/termojinal"));
    }
}
