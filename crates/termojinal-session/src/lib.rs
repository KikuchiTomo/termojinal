//! Session management for termojinal.
//!
//! Manages PTY sessions with JSON persistence and daemon support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

pub mod daemon;
pub mod hotkey;
pub mod persistence;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("PTY error: {0}")]
    Pty(#[from] termojinal_pty::PtyError),
}

/// Serializable session state for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: DateTime<Utc>,
    pub pid: Option<i32>,
}

impl SessionState {
    pub fn new(shell: &str, cwd: &str, cols: u16, rows: u16) -> Self {
        let id = Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            name: format!("session-{}", &id[..8]),
            shell: shell.to_string(),
            cwd: cwd.to_string(),
            env: termojinal_pty::default_env(),
            cols,
            rows,
            created_at: Utc::now(),
            pid: None,
        }
    }
}

/// A live session: state + active PTY.
pub struct Session {
    pub state: SessionState,
    pub pty: termojinal_pty::Pty,
}

impl Session {
    /// Create a new session by spawning a PTY.
    pub fn spawn(state: SessionState) -> Result<Self, SessionError> {
        let config = termojinal_pty::PtyConfig {
            shell: state.shell.clone(),
            size: termojinal_pty::PtySize {
                cols: state.cols,
                rows: state.rows,
            },
            env: state.env.clone(),
            working_dir: Some(state.cwd.clone()),
        };

        let pty = termojinal_pty::Pty::spawn(&config)?;
        let mut state = state;
        state.pid = Some(pty.pid().as_raw());

        Ok(Session { state, pty })
    }

    /// Resize the session's PTY.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.state.cols = cols;
        self.state.rows = rows;
        self.pty
            .resize(termojinal_pty::PtySize { cols, rows })
            .map_err(SessionError::from)
    }

    /// Check if the session's process is still alive.
    pub fn is_alive(&self) -> bool {
        self.pty.is_alive()
    }

    /// Update the session's current working directory (e.g. from OSC 7).
    pub fn update_cwd(&mut self, cwd: &str) {
        self.state.cwd = cwd.to_string();
    }
}

/// Manages multiple sessions.
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    /// Externally-spawned sessions (e.g. UI-owned PTYs) tracked by pane ID.
    /// The daemon does not own the PTY — it only records the state so that
    /// `tm list` can report them.
    tracked: HashMap<u64, SessionState>,
    persistence: persistence::SessionStore,
}

impl SessionManager {
    pub fn new() -> Result<Self, SessionError> {
        let persistence = persistence::SessionStore::new()?;
        Ok(Self {
            sessions: HashMap::new(),
            tracked: HashMap::new(),
            persistence,
        })
    }

    /// Create and spawn a new session.
    pub fn create_session(
        &mut self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<&Session, SessionError> {
        let state = SessionState::new(shell, cwd, cols, rows);
        let id = state.id.clone();
        let session = Session::spawn(state)?;
        self.persistence.save(&session.state)?;
        self.sessions.insert(id.clone(), session);
        Ok(self.sessions.get(&id).unwrap())
    }

    /// Get a session by ID.
    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// Get a mutable session by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    /// Remove a session.
    pub fn remove(&mut self, id: &str) -> Result<(), SessionError> {
        self.sessions.remove(id);
        self.persistence.remove(id)?;
        Ok(())
    }

    /// List all session IDs (daemon-owned + externally tracked).
    pub fn list(&self) -> Vec<&str> {
        self.sessions
            .keys()
            .map(|s| s.as_str())
            .chain(self.tracked.values().map(|s| s.id.as_str()))
            .collect()
    }

    /// Save all session states to disk.
    pub fn save_all(&self) -> Result<(), SessionError> {
        for session in self.sessions.values() {
            self.persistence.save(&session.state)?;
        }
        Ok(())
    }

    /// Load saved session states from disk (does not reattach PTYs).
    pub fn load_saved_states(&self) -> Result<Vec<SessionState>, SessionError> {
        self.persistence.load_all()
    }

    /// Remove a saved session file from disk without affecting live sessions.
    /// Used to clean up stale session files on daemon startup.
    pub fn remove_saved(&self, id: &str) -> Result<(), SessionError> {
        self.persistence.remove(id)
    }

    /// Update a session's CWD (e.g. when OSC 7 is received) and persist it.
    pub fn update_session_cwd(&mut self, id: &str, cwd: &str) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.update_cwd(cwd);
            self.persistence.save(&session.state)?;
        }
        Ok(())
    }

    /// Clean up dead sessions (daemon-owned and externally tracked).
    pub fn reap_dead(&mut self) -> Vec<String> {
        // Reap daemon-owned sessions.
        let dead: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !s.is_alive())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.sessions.remove(id);
            let _ = self.persistence.remove(id);
        }

        // Reap externally tracked sessions whose PIDs are no longer alive.
        let dead_tracked: Vec<u64> = self
            .tracked
            .iter()
            .filter(|(_, s)| {
                let Some(pid) = s.pid else {
                    return true;
                };
                use nix::sys::signal;
                use nix::unistd::Pid;
                signal::kill(Pid::from_raw(pid), None).is_err()
            })
            .map(|(pane_id, _)| *pane_id)
            .collect();
        for pane_id in &dead_tracked {
            if let Some(state) = self.tracked.remove(pane_id) {
                let _ = self.persistence.remove(&state.id);
            }
        }

        // Return all reaped IDs.
        dead.into_iter()
            .chain(dead_tracked.iter().map(|id| format!("tracked-pane-{id}")))
            .collect()
    }

    /// Register an externally-spawned session (UI-owned PTY).
    ///
    /// The daemon does not own or manage the PTY — it only records the
    /// session state so that `tm list` reports it.  The session is keyed
    /// by `pane_id` so the UI can unregister it later.
    pub fn register_external_session(
        &mut self,
        pane_id: u64,
        pid: i32,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> String {
        let mut state = SessionState::new(shell, cwd, cols, rows);
        state.pid = Some(pid);
        state.name = format!("pane-{}", pane_id);
        let id = state.id.clone();
        self.persistence.save(&state).ok();
        self.tracked.insert(pane_id, state);
        id
    }

    /// Unregister an externally-spawned session by pane ID.
    pub fn unregister_external_session(&mut self, pane_id: u64) -> bool {
        if let Some(state) = self.tracked.remove(&pane_id) {
            let _ = self.persistence.remove(&state.id);
            true
        } else {
            false
        }
    }
}
