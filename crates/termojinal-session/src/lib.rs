//! Session management for termojinal.
//!
//! Manages PTY sessions with JSON persistence and daemon support.
//! In the "Daemon-owned PTY" model, the daemon fork/execs shells,
//! holds the master fd, and streams PTY data to connected GUI clients.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
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

    #[error("session already exists: {0}")]
    AlreadyExists(String),
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

// ---------------------------------------------------------------------------
// Session control commands (sent to per-session tokio task)
// ---------------------------------------------------------------------------

/// Commands sent to a per-session I/O task via an mpsc channel.
pub enum SessionControl {
    /// Write raw bytes to the PTY (key input from GUI).
    WriteInput(Vec<u8>),
    /// Resize the PTY.
    Resize { cols: u16, rows: u16 },
    /// Detach a specific client (by client_id).
    Detach(u64),
}

// ---------------------------------------------------------------------------
// Client sender (represents a connected GUI client)
// ---------------------------------------------------------------------------

/// Sender handle for a connected GUI client.
/// The daemon's per-session task uses these to broadcast PTY output.
#[derive(Clone)]
pub struct ClientSender {
    pub id: u64,
    tx: tokio::sync::mpsc::UnboundedSender<ClientMessage>,
}

/// Messages sent to a connected GUI client.
#[derive(Debug)]
pub enum ClientMessage {
    /// Raw PTY output bytes.
    PtyOutput { session_id: String, data: Vec<u8> },
    /// The session has exited.
    SessionExited {
        session_id: String,
        exit_code: Option<i32>,
    },
}

impl ClientSender {
    pub fn new(id: u64, tx: tokio::sync::mpsc::UnboundedSender<ClientMessage>) -> Self {
        Self { id, tx }
    }

    pub fn send_pty_output(&self, session_id: &str, data: &[u8]) -> Result<(), ()> {
        self.tx
            .send(ClientMessage::PtyOutput {
                session_id: session_id.to_string(),
                data: data.to_vec(),
            })
            .map_err(|_| ())
    }

    pub fn send_session_exited(&self, session_id: &str, exit_code: Option<i32>) -> Result<(), ()> {
        self.tx
            .send(ClientMessage::SessionExited {
                session_id: session_id.to_string(),
                exit_code,
            })
            .map_err(|_| ())
    }
}

// ---------------------------------------------------------------------------
// DaemonSession (daemon-owned PTY session)
// ---------------------------------------------------------------------------

/// A daemon-owned PTY session. The daemon holds the master fd and
/// runs a per-session tokio task that reads PTY output and broadcasts
/// it to connected GUI clients.
pub struct DaemonSession {
    pub state: SessionState,
    /// Control channel to the per-session I/O task.
    pub control_tx: tokio::sync::mpsc::Sender<SessionControl>,
    /// Connected GUI clients.
    pub clients: Arc<Mutex<Vec<ClientSender>>>,
    /// Daemon-side Terminal for re-attach snapshots.
    pub terminal: Arc<Mutex<termojinal_vt::Terminal>>,
    /// VT parser for the daemon-side terminal.
    pub vt_parser: Arc<Mutex<vte::Parser>>,
}

impl DaemonSession {
    /// Check if any GUI clients are attached.
    pub fn is_attached(&self) -> bool {
        self.clients.lock().map(|c| !c.is_empty()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// SessionManager (manages all daemon-owned sessions)
// ---------------------------------------------------------------------------

/// Manages multiple daemon-owned sessions.
pub struct SessionManager {
    sessions: HashMap<String, DaemonSession>,
    /// Externally-spawned sessions (e.g. UI-owned PTYs) tracked by pane ID.
    /// The daemon does not own the PTY -- it only records the state so that
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

    /// Create and spawn a new daemon-owned session.
    ///
    /// This fork/execs the shell, sets the master fd to non-blocking,
    /// and spawns a per-session tokio task for I/O.
    pub fn create_session(
        &mut self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<&DaemonSession, SessionError> {
        self.create_session_with_manager(shell, cwd, cols, rows, None)
    }

    /// Create and spawn a new daemon-owned session with a reference to the
    /// manager for automatic cleanup when the session exits.
    pub fn create_session_with_manager(
        &mut self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
        manager_ref: Option<Arc<tokio::sync::Mutex<SessionManager>>>,
    ) -> Result<&DaemonSession, SessionError> {
        let mut state = SessionState::new(shell, cwd, cols, rows);

        // Spawn PTY via termojinal-pty.
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
        state.pid = Some(pty.pid().as_raw());

        let session_id = state.id.clone();
        let shell_pid = pty.pid();

        // Save state to disk.
        self.persistence.save(&state)?;

        // Create channels and shared state.
        let (control_tx, control_rx) = tokio::sync::mpsc::channel::<SessionControl>(256);
        let clients: Arc<Mutex<Vec<ClientSender>>> = Arc::new(Mutex::new(Vec::new()));
        let terminal = Arc::new(Mutex::new(termojinal_vt::Terminal::new(
            cols as usize,
            rows as usize,
        )));
        let vt_parser = Arc::new(Mutex::new(vte::Parser::new()));

        let clients_clone = clients.clone();
        let terminal_clone = terminal.clone();
        let vt_parser_clone = vt_parser.clone();
        let sid = session_id.clone();

        // Spawn per-session I/O task.
        let mgr_ref = manager_ref
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(SessionManager::new().unwrap())));
        tokio::spawn(async move {
            session_io_task(
                pty,
                shell_pid,
                sid,
                clients_clone,
                control_rx,
                terminal_clone,
                vt_parser_clone,
                mgr_ref,
            )
            .await;
        });

        let daemon_session = DaemonSession {
            state,
            control_tx,
            clients,
            terminal,
            vt_parser,
        };

        self.sessions.insert(session_id.clone(), daemon_session);
        Ok(self.sessions.get(&session_id).unwrap())
    }

    /// Attach a client to an existing session.
    pub fn attach_session(
        &self,
        session_id: &str,
        client: ClientSender,
    ) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session.clients.lock().unwrap().push(client);
        Ok(())
    }

    /// Detach a client from a session.
    pub fn detach_session(&self, session_id: &str, client_id: u64) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .clients
            .lock()
            .unwrap()
            .retain(|c| c.id != client_id);
        Ok(())
    }

    /// Get a session by ID.
    pub fn get(&self, id: &str) -> Option<&DaemonSession> {
        self.sessions.get(id)
    }

    /// Remove a session (kills the shell).
    pub fn remove(&mut self, id: &str) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get(id) {
            if let Some(pid) = session.state.pid {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                let _ = killpg(Pid::from_raw(pid), Signal::SIGHUP);
            }
        }
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

    /// List full details for all sessions (daemon-owned + externally tracked).
    pub fn list_details(&self) -> Vec<&SessionState> {
        self.sessions
            .values()
            .map(|s| &s.state)
            .chain(self.tracked.values())
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
    pub fn remove_saved(&self, id: &str) -> Result<(), SessionError> {
        self.persistence.remove(id)
    }

    /// Update a session's CWD and persist it.
    pub fn update_session_cwd(&mut self, id: &str, cwd: &str) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.state.cwd = cwd.to_string();
            self.persistence.save(&session.state)?;
        }
        Ok(())
    }

    /// Clean up dead sessions.
    pub fn reap_dead(&mut self) -> Vec<String> {
        // Reap daemon-owned sessions whose shell PID is no longer alive.
        let dead: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                let Some(pid) = s.state.pid else { return true };
                use nix::sys::signal;
                use nix::unistd::Pid;
                signal::kill(Pid::from_raw(pid), None).is_err()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.sessions.remove(id);
            let _ = self.persistence.remove(id);
        }

        // Reap externally tracked sessions.
        let dead_tracked: Vec<u64> = self
            .tracked
            .iter()
            .filter(|(_, s)| {
                let Some(pid) = s.pid else { return true };
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

        dead.into_iter()
            .chain(dead_tracked.iter().map(|id| format!("tracked-pane-{id}")))
            .collect()
    }

    /// Register an externally-spawned session (UI-owned PTY).
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

    /// Kill all sessions (daemon-owned and externally tracked).
    /// Daemon-owned sessions are dropped (SIGHUP sent to PTY child).
    /// Externally tracked sessions are sent SIGKILL.
    pub fn kill_all(&mut self) -> usize {
        let count = self.sessions.len() + self.tracked.len();

        // Send SIGHUP to daemon-owned sessions.
        for session in self.sessions.values() {
            if let Some(pid) = session.state.pid {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                let _ = killpg(Pid::from_raw(pid), Signal::SIGHUP);
            }
        }

        // Kill externally tracked sessions by sending SIGKILL to their PIDs.
        for state in self.tracked.values() {
            if let Some(pid) = state.pid {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
        }

        // Remove all persistence files.
        let _ = self.persistence.clear();

        // Clear all sessions (dropping DaemonSession sends SIGHUP via PTY drop).
        self.sessions.clear();
        self.tracked.clear();

        count
    }

    /// Gracefully exit a session by ID.
    /// Returns `Ok(None)` if the session was exited cleanly.
    /// Returns `Ok(Some(proc_name))` if a foreground process is running
    /// (caller should confirm before forcing).
    /// Returns `Err` if the session was not found.
    pub fn exit_session(&mut self, id: &str) -> Result<Option<String>, SessionError> {
        // Check daemon-owned sessions first.
        if self.sessions.contains_key(id) {
            let pid = self.sessions[id].state.pid;
            if let Some(pid) = pid {
                // Check for foreground child process.
                if let Some(proc_name) = detect_foreground_child_of(pid) {
                    return Ok(Some(proc_name));
                }
            }
            // No foreground child — remove the session (PTY drop sends SIGHUP).
            self.remove(id)?;
            return Ok(None);
        }

        // Check externally tracked sessions.
        let tracked_entry = self
            .tracked
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(pane_id, s)| (*pane_id, s.pid));
        if let Some((pane_id, pid)) = tracked_entry {
            if let Some(pid) = pid {
                // Check for foreground child.
                if let Some(proc_name) = detect_foreground_child_of(pid) {
                    return Ok(Some(proc_name));
                }
                // Send SIGHUP to the tracked process.
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGHUP);
            }
            self.tracked.remove(&pane_id);
            let _ = self.persistence.remove(id);
            return Ok(None);
        }

        Err(SessionError::NotFound(id.to_string()))
    }

    /// Force-exit a session by ID, regardless of running processes.
    pub fn force_exit_session(&mut self, id: &str) -> Result<(), SessionError> {
        // Check daemon-owned sessions.
        if self.sessions.contains_key(id) {
            self.remove(id)?;
            return Ok(());
        }

        // Check externally tracked sessions.
        let tracked_entry = self
            .tracked
            .iter()
            .find(|(_, s)| s.id == id)
            .map(|(pane_id, s)| (*pane_id, s.pid));
        if let Some((pane_id, pid)) = tracked_entry {
            if let Some(pid) = pid {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
            self.tracked.remove(&pane_id);
            let _ = self.persistence.remove(id);
            return Ok(());
        }

        Err(SessionError::NotFound(id.to_string()))
    }

    /// Get a snapshot of a session's terminal state (for re-attach).
    pub fn get_snapshot(&self, session_id: &str) -> Option<termojinal_vt::TerminalSnapshot> {
        let session = self.sessions.get(session_id)?;
        let term = session.terminal.lock().ok()?;
        Some(term.snapshot())
    }

    /// Send input to a session's PTY via its control channel.
    pub async fn send_input(&self, session_id: &str, data: Vec<u8>) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .control_tx
            .send(SessionControl::WriteInput(data))
            .await
            .map_err(|_| {
                SessionError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "session I/O task has exited",
                ))
            })
    }

    /// Resize a session's PTY via its control channel.
    pub async fn resize_session(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .control_tx
            .send(SessionControl::Resize { cols, rows })
            .await
            .map_err(|_| {
                SessionError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "session I/O task has exited",
                ))
            })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Per-session I/O task
// ---------------------------------------------------------------------------

/// Per-session tokio task that reads PTY output and dispatches it to clients.
///
/// This task owns the `Pty` struct (which owns the master fd). When the task
/// exits (shell died), the Pty is dropped, which sends SIGHUP to the child.
async fn session_io_task(
    pty: termojinal_pty::Pty,
    shell_pid: nix::unistd::Pid,
    session_id: String,
    clients: Arc<Mutex<Vec<ClientSender>>>,
    mut control_rx: tokio::sync::mpsc::Receiver<SessionControl>,
    terminal: Arc<Mutex<termojinal_vt::Terminal>>,
    vt_parser: Arc<Mutex<vte::Parser>>,
    manager: Arc<tokio::sync::Mutex<SessionManager>>,
) {
    use tokio::io::unix::AsyncFd;

    // Set the master fd to non-blocking for AsyncFd.
    let master_fd = pty.master_fd();
    let flags = nix::fcntl::fcntl(master_fd, nix::fcntl::FcntlArg::F_GETFL).unwrap_or(0);
    let mut oflags = nix::fcntl::OFlag::from_bits_truncate(flags);
    oflags.insert(nix::fcntl::OFlag::O_NONBLOCK);
    let _ = nix::fcntl::fcntl(master_fd, nix::fcntl::FcntlArg::F_SETFL(oflags));

    // SAFETY: We create an AsyncFd from a borrowed fd. The Pty struct outlives
    // the AsyncFd because both are owned by this task. We use BorrowedFdWrapper
    // to avoid transferring ownership.
    let async_fd = match AsyncFd::new(BorrowedFdWrapper(master_fd)) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("session {session_id}: failed to create AsyncFd: {e}");
            return;
        }
    };

    let mut buf = vec![0u8; 65536];
    let mut exit_code: Option<i32> = None;

    loop {
        tokio::select! {
            // PTY output ready.
            guard = async_fd.readable() => {
                let mut guard = match guard {
                    Ok(g) => g,
                    Err(_) => break,
                };
                // SAFETY: master_fd is a valid fd borrowed from the Pty.
                let n = unsafe { libc::read(master_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                match n {
                    n if n > 0 => {
                        let n = n as usize;
                        let data = &buf[..n];
                        // Feed daemon-side terminal (for snapshots).
                        if let (Ok(mut term), Ok(mut parser)) = (terminal.lock(), vt_parser.lock()) {
                            term.feed(&mut parser, data);
                        }
                        // Broadcast to connected clients.
                        if let Ok(clients) = clients.lock() {
                            for client in clients.iter() {
                                let _ = client.send_pty_output(&session_id, data);
                            }
                        }
                    }
                    0 => {
                        // EOF -- shell exited.
                        break;
                    }
                    _ if n < 0 => {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        if errno == libc::EIO {
                            break;
                        } else if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                            guard.clear_ready();
                            continue;
                        } else {
                            log::error!("session {session_id}: PTY read error: errno={errno}");
                            break;
                        }
                    }
                    _ => {
                        break;
                    }
                }
            }

            // Control command from daemon handler.
            cmd = control_rx.recv() => {
                match cmd {
                    Some(SessionControl::WriteInput(data)) => {
                        let mut offset = 0usize;
                        while offset < data.len() {
                            let n = unsafe {
                                libc::write(
                                    master_fd,
                                    data[offset..].as_ptr() as *const libc::c_void,
                                    data.len() - offset,
                                )
                            };
                            if n > 0 {
                                offset += n as usize;
                            } else if n < 0 {
                                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                                if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                                    // Yield to the runtime and retry.
                                    tokio::task::yield_now().await;
                                    continue;
                                } else {
                                    log::error!("session {session_id}: PTY write error: errno={errno}");
                                    break;
                                }
                            } else {
                                // write returned 0, should not happen for PTY
                                break;
                            }
                        }
                    }
                    Some(SessionControl::Resize { cols, rows }) => {
                        let ws = libc::winsize {
                            ws_col: cols,
                            ws_row: rows,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        unsafe {
                            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
                        }
                        if let Ok(mut term) = terminal.lock() {
                            term.resize(cols as usize, rows as usize);
                        }
                        let _ = nix::sys::signal::kill(shell_pid, nix::sys::signal::Signal::SIGWINCH);
                    }
                    Some(SessionControl::Detach(client_id)) => {
                        if let Ok(mut clients) = clients.lock() {
                            clients.retain(|c| c.id != client_id);
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    // Shell has exited. Collect exit status with retry loop (max ~1 second).
    {
        let start = std::time::Instant::now();
        loop {
            match nix::sys::wait::waitpid(shell_pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
                Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
                    exit_code = Some(code);
                    break;
                }
                Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
                    exit_code = Some(128 + sig as i32);
                    break;
                }
                Ok(nix::sys::wait::WaitStatus::StillAlive) => {
                    if start.elapsed() > std::time::Duration::from_secs(1) {
                        log::warn!("session {session_id}: waitpid timed out after 1s");
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => {
                    log::warn!("session {session_id}: waitpid error: {e}");
                    break;
                }
                _ => break,
            }
        }
    }

    log::info!("session {session_id}: shell exited (code={exit_code:?})");

    // Notify all connected clients.
    if let Ok(clients) = clients.lock() {
        for client in clients.iter() {
            let _ = client.send_session_exited(&session_id, exit_code);
        }
    }

    // Remove the session from the manager so it's no longer listed.
    {
        let mut mgr = manager.lock().await;
        mgr.sessions.remove(&session_id);
        let _ = mgr.persistence.remove(&session_id);
        log::info!("session {session_id}: removed from manager");
    }
}

/// Wrapper around a raw fd that implements AsRawFd for AsyncFd.
/// SAFETY: The fd is valid for the lifetime of this wrapper because
/// it is borrowed from a Pty that outlives it within session_io_task.
struct BorrowedFdWrapper(std::os::fd::RawFd);

impl AsRawFd for BorrowedFdWrapper {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect if a process has a foreground child (i.e. something is running in the shell).
/// Returns the child process name if found, or `None` if the shell is idle.
fn detect_foreground_child_of(pid: i32) -> Option<String> {
    use std::process::Command;
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let child_pid = stdout.lines().next()?.trim().parse::<i32>().ok()?;
    let output = Command::new("ps")
        .args(["-p", &child_pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}
