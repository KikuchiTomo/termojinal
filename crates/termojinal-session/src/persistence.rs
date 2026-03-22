//! Session state persistence to JSON files.

use crate::{SessionError, SessionState};
use std::path::PathBuf;

/// Handles reading/writing session state to disk.
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new() -> Result<Self, SessionError> {
        let dir = session_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Save a session state to disk.
    pub fn save(&self, state: &SessionState) -> Result<(), SessionError> {
        let path = self.dir.join(format!("{}.json", state.id));
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a session state from disk.
    pub fn load(&self, id: &str) -> Result<SessionState, SessionError> {
        let path = self.dir.join(format!("{id}.json"));
        if !path.exists() {
            return Err(SessionError::NotFound(id.to_string()));
        }
        let json = std::fs::read_to_string(path)?;
        let state: SessionState = serde_json::from_str(&json)?;
        Ok(state)
    }

    /// Load all saved session states.
    pub fn load_all(&self) -> Result<Vec<SessionState>, SessionError> {
        let mut states = Vec::new();
        if !self.dir.exists() {
            return Ok(states);
        }
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                match std::fs::read_to_string(&path) {
                    Ok(json) => match serde_json::from_str::<SessionState>(&json) {
                        Ok(state) => states.push(state),
                        Err(e) => {
                            log::warn!("failed to parse {}: {e}", path.display());
                        }
                    },
                    Err(e) => {
                        log::warn!("failed to read {}: {e}", path.display());
                    }
                }
            }
        }
        Ok(states)
    }

    /// Remove a session state file.
    pub fn remove(&self, id: &str) -> Result<(), SessionError> {
        let path = self.dir.join(format!("{id}.json"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Remove all session state files.
    pub fn clear(&self) -> Result<(), SessionError> {
        if self.dir.exists() {
            for entry in std::fs::read_dir(&self.dir)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|e| e == "json") {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(())
    }
}

/// Get the session data directory.
fn session_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("termojinal")
        .join("sessions")
}

// ---------------------------------------------------------------------------
// Terminal Snapshot Persistence (Time Travel)
// ---------------------------------------------------------------------------

/// Handles terminal snapshot and command history persistence for session restoration.
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new() -> Result<Self, SessionError> {
        let dir = snapshot_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Save a terminal snapshot for a session.
    pub fn save_snapshot(
        &self,
        session_id: &str,
        snapshot: &termojinal_vt::TerminalSnapshot,
    ) -> Result<(), SessionError> {
        let path = self.dir.join(format!("{session_id}.snapshot.json"));
        let json = serde_json::to_string(snapshot)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a terminal snapshot for a session.
    pub fn load_snapshot(
        &self,
        session_id: &str,
    ) -> Result<termojinal_vt::TerminalSnapshot, SessionError> {
        let path = self.dir.join(format!("{session_id}.snapshot.json"));
        if !path.exists() {
            return Err(SessionError::NotFound(format!(
                "snapshot for {session_id}"
            )));
        }
        let json = std::fs::read_to_string(path)?;
        let snapshot: termojinal_vt::TerminalSnapshot = serde_json::from_str(&json)?;
        Ok(snapshot)
    }

    /// Save a named snapshot.
    pub fn save_named_snapshot(
        &self,
        session_id: &str,
        snapshot: &termojinal_vt::NamedSnapshot,
    ) -> Result<(), SessionError> {
        let dir = self.dir.join(format!("{session_id}.snapshots"));
        std::fs::create_dir_all(&dir)?;
        // S6: Use sanitized name + timestamp suffix to avoid collisions
        // (e.g., "my command!" and "my command?" both become "my_command_").
        let safe_name: String = snapshot
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let ts = snapshot.created_at.format("%Y%m%d%H%M%S").to_string();
        let path = dir.join(format!("{safe_name}_{ts}.json"));
        let json = serde_json::to_string(snapshot)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load all named snapshots for a session.
    pub fn load_named_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Vec<termojinal_vt::NamedSnapshot>, SessionError> {
        let dir = self.dir.join(format!("{session_id}.snapshots"));
        let mut snapshots = Vec::new();
        if !dir.exists() {
            return Ok(snapshots);
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        match serde_json::from_str::<termojinal_vt::NamedSnapshot>(&json) {
                            Ok(s) => snapshots.push(s),
                            Err(e) => log::warn!("failed to parse snapshot {}: {e}", path.display()),
                        }
                    }
                    Err(e) => log::warn!("failed to read {}: {e}", path.display()),
                }
            }
        }
        snapshots.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(snapshots)
    }

    /// Remove a snapshot file.
    pub fn remove_snapshot(&self, session_id: &str) -> Result<(), SessionError> {
        let path = self.dir.join(format!("{session_id}.snapshot.json"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Get the snapshot data directory.
fn snapshot_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("termojinal")
        .join("snapshots")
}
