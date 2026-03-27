//! Workspace info, refresh, and agent state.

use crate::daemon_client::daemon_socket_path;
use crate::platform::get_child_cwd;
use crate::UserEvent;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use termojinal_layout::PaneId;
use winit::event_loop::EventLoopProxy;

pub(crate) struct WorkspaceRefreshRequest {
    /// Workspace index.
    pub(crate) wi: usize,
    /// Resolved CWD (from OSC 7 or empty).
    pub(crate) osc_cwd: String,
    /// PTY PID for lsof fallback + port detection.
    pub(crate) pty_pid: Option<i32>,
}

/// Background thread that refreshes workspace info (git, lsof, daemon)
/// without blocking the render thread.
pub(crate) struct AsyncWorkspaceRefresher {
    /// Latest results per workspace index, read by the render thread.
    results: Arc<Mutex<Vec<WorkspaceInfo>>>,
    /// Pending requests (replaced atomically by the render thread).
    requests: Arc<Mutex<Vec<WorkspaceRefreshRequest>>>,
    /// Latest daemon sessions result.
    daemon_sessions: Arc<Mutex<Vec<DaemonSessionInfo>>>,
    /// Signal the background thread to wake up.
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
    /// Global shutdown flag.
    pub(crate) shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl AsyncWorkspaceRefresher {
    pub(crate) fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let results: Arc<Mutex<Vec<WorkspaceInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let requests: Arc<Mutex<Vec<WorkspaceRefreshRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let daemon_sessions: Arc<Mutex<Vec<DaemonSessionInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let res = Arc::clone(&results);
        let req = Arc::clone(&requests);
        let ds = Arc::clone(&daemon_sessions);
        let wake = Arc::clone(&notify);
        let shut = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("workspace-refresher".into())
            .spawn(move || {
                let mut last_daemon_refresh = std::time::Instant::now();
                loop {
                    if shut.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    // Wait up to 3 seconds, or wake immediately if nudged.
                    {
                        let (lock, cvar) = &*wake;
                        let mut nudged = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if !*nudged {
                            let (mut g, _) = cvar
                                .wait_timeout(nudged, std::time::Duration::from_secs(3))
                                .unwrap_or_else(|e| e.into_inner());
                            *g = false;
                        } else {
                            *nudged = false;
                        }
                    }

                    // Grab pending requests.
                    let pending: Vec<WorkspaceRefreshRequest> = {
                        let mut r = req.lock().unwrap_or_else(|e| e.into_inner());
                        std::mem::take(&mut *r)
                    };

                    if !pending.is_empty() {
                        // Process each workspace request.
                        let mut new_results = { res.lock().unwrap_or_else(|e| e.into_inner()).clone() };
                        // Ensure capacity.
                        while new_results.len() <= pending.iter().map(|r| r.wi).max().unwrap_or(0) {
                            new_results.push(WorkspaceInfo::new());
                        }

                        let mut changed = false;
                        for request in &pending {
                            let wi = request.wi;
                            // Resolve CWD with lsof fallback.
                            let cwd = if !request.osc_cwd.is_empty() {
                                request.osc_cwd.clone()
                            } else if let Some(pid) = request.pty_pid {
                                get_child_cwd(pid).unwrap_or_default()
                            } else {
                                String::new()
                            };

                            let old_name = new_results[wi].name.clone();
                            let old_branch = new_results[wi].git_branch.clone();
                            refresh_workspace_info(&mut new_results[wi], &cwd, request.pty_pid);
                            if new_results[wi].name != old_name
                                || new_results[wi].git_branch != old_branch
                            {
                                changed = true;
                            }
                        }

                        *res.lock().unwrap_or_else(|e| e.into_inner()) = new_results;
                        if changed {
                            let _ = proxy.send_event(UserEvent::StatusUpdate);
                        }
                    }

                    // Refresh daemon sessions every 10 seconds.
                    if last_daemon_refresh.elapsed().as_secs() >= 10 {
                        let sessions = query_daemon_sessions();
                        *ds.lock().unwrap_or_else(|e| e.into_inner()) = sessions;
                        last_daemon_refresh = std::time::Instant::now();
                        let _ = proxy.send_event(UserEvent::StatusUpdate);
                    }
                }
            })
            .expect("failed to spawn workspace refresher thread");

        Self {
            results,
            requests,
            daemon_sessions,
            notify,
            shutdown,
        }
    }

    /// Submit refresh requests (called from render thread — non-blocking).
    pub(crate) fn submit(&self, requests: Vec<WorkspaceRefreshRequest>) {
        if let Ok(mut r) = self.requests.try_lock() {
            *r = requests;
        }
        // Wake the background thread.
        let (lock, cvar) = &*self.notify;
        if let Ok(mut nudged) = lock.try_lock() {
            *nudged = true;
            cvar.notify_one();
        }
    }

    /// Get latest workspace infos (called from render thread — non-blocking).
    pub(crate) fn get_results(&self) -> Vec<WorkspaceInfo> {
        self.results
            .try_lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Get latest daemon sessions (called from render thread — non-blocking).
    pub(crate) fn get_daemon_sessions(&self) -> Vec<DaemonSessionInfo> {
        self.daemon_sessions
            .try_lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceInfo {
    pub(crate) name: String,
    /// Resolved CWD (from OSC 7 or lsof fallback). Cached so render code
    /// doesn't need to call lsof on every frame.
    pub(crate) cwd: String,
    pub(crate) git_branch: Option<String>,
    pub(crate) git_dirty: usize,
    pub(crate) git_untracked: usize,
    pub(crate) git_ahead: usize,
    pub(crate) git_behind: usize,
    pub(crate) ports: Vec<u16>,
    pub(crate) last_updated: Instant,
    pub(crate) has_unread: bool,
}

impl WorkspaceInfo {
    pub(crate) fn new() -> Self {
        Self {
            name: String::new(),
            cwd: String::new(),
            git_branch: None,
            git_dirty: 0,
            git_untracked: 0,
            git_ahead: 0,
            git_behind: 0,
            ports: Vec::new(),
            last_updated: Instant::now(),
            has_unread: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent session info for sidebar AI status display
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum AgentState {
    Running,
    Idle,
    WaitingForPermission,
    Inactive,
}

#[derive(Debug, Clone)]
pub(crate) struct SubAgentInfo {
    pub(crate) title: String,
    pub(crate) state: AgentState,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentSessionInfo {
    pub(crate) active: bool,
    pub(crate) session_id: Option<String>,
    /// The pane where this agent session is running.
    pub(crate) pane_id: Option<PaneId>,
    /// Stable title for this agent session (e.g. Claude Code task name).
    /// Set once via IPC and NOT overwritten by pane/workspace switches.
    pub(crate) title: Option<String>,
    pub(crate) subagent_count: usize,
    pub(crate) subagents: Vec<SubAgentInfo>,
    pub(crate) summary: String,
    pub(crate) state: AgentState,
    pub(crate) last_updated: std::time::Instant,
}

impl Default for AgentSessionInfo {
    fn default() -> Self {
        Self {
            active: false,
            session_id: None,
            pane_id: None,
            title: None,
            subagent_count: 0,
            subagents: Vec::new(),
            summary: String::new(),
            state: AgentState::Inactive,
            last_updated: std::time::Instant::now(),
        }
    }
}

/// Rotating palette for workspace indicator dots (Arc browser inspired).
pub(crate) const WORKSPACE_COLORS: [[f32; 4]; 6] = [
    [0.29, 0.62, 1.0, 1.0],  // blue
    [0.55, 0.82, 0.33, 1.0], // green
    [1.0, 0.58, 0.26, 1.0],  // orange
    [0.87, 0.44, 0.85, 1.0], // purple
    [1.0, 0.42, 0.42, 1.0],  // red
    [0.36, 0.84, 0.77, 1.0], // teal
];

/// Refresh workspace info by running git commands and detecting ports.
pub(crate) fn refresh_workspace_info(info: &mut WorkspaceInfo, cwd: &str, pty_pid: Option<i32>) {
    info.cwd = cwd.to_string();
    if cwd.is_empty() {
        info.name = String::new();
        info.git_branch = None;
        info.git_dirty = 0;
        info.git_untracked = 0;
        info.git_ahead = 0;
        info.git_behind = 0;
        return;
    }
    // Name = basename of CWD.
    info.name = std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    // Git branch.
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
    {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info.git_branch = if branch.is_empty() {
                None
            } else {
                Some(branch)
            };
        } else {
            info.git_branch = None;
        }
    }

    // Git dirty and untracked counts via porcelain status.
    info.git_dirty = 0;
    info.git_untracked = 0;
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().filter(|l| !l.is_empty()) {
                if line.starts_with("??") {
                    info.git_untracked += 1;
                } else {
                    info.git_dirty += 1;
                }
            }
        }
    }

    // Git ahead/behind counts.
    info.git_ahead = 0;
    info.git_behind = 0;
    if let Ok(output) = std::process::Command::new("git")
        .args([
            "-C",
            cwd,
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let parts: Vec<&str> = text.split_whitespace().collect();
            if parts.len() == 2 {
                info.git_ahead = parts[0].parse().unwrap_or(0);
                info.git_behind = parts[1].parse().unwrap_or(0);
            }
        }
    }

    // Detect listening ports from focused pane's PTY process.
    if let Some(pid) = pty_pid {
        info.ports = detect_listening_ports(pid);
    } else {
        info.ports.clear();
    }

    info.last_updated = Instant::now();
}

/// Detect TCP listening ports for a given PID and its children.
/// Uses `lsof -iTCP -sTCP:LISTEN -n -P -a -p <pid>` on macOS.
pub(crate) fn detect_listening_ports(pid: i32) -> Vec<u16> {
    let pid_str = pid.to_string();
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-iTCP", "-sTCP:LISTEN", "-n", "-P", "-a", "-p", &pid_str])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut ports: Vec<u16> = Vec::new();
    for line in text.lines().skip(1) {
        // NAME column is last, format like "*:3000" or "127.0.0.1:8080"
        if let Some(name) = line.split_whitespace().last() {
            if let Some(port_str) = name.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if !ports.contains(&port) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports.sort();
    ports
}

/// Query the session daemon for all tracked sessions.
/// Returns an empty Vec on connection failure (daemon not running, etc.).
pub(crate) fn query_daemon_sessions() -> Vec<DaemonSessionInfo> {
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream;

    let sock_path = daemon_socket_path();
    let Ok(mut stream) = UnixStream::connect(&sock_path) else {
        return Vec::new();
    };
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    let req = serde_json::json!({"type": "list_session_details"});
    let msg = format!("{}\n", req);
    if stream.write_all(msg.as_bytes()).is_err() {
        return Vec::new();
    }
    let mut line = String::new();
    if std::io::BufReader::new(&stream)
        .read_line(&mut line)
        .is_err()
    {
        return Vec::new();
    }
    let Ok(resp) = serde_json::from_str::<serde_json::Value>(&line) else {
        return Vec::new();
    };
    let Some(sessions) = resp
        .get("data")
        .and_then(|d| d.get("sessions"))
        .and_then(|s| s.as_array())
    else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?.to_string();
            let name = s
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let shell = s
                .get("shell")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cwd = s
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pid = s.get("pid").and_then(|v| v.as_i64()).map(|p| p as i32);
            let pane_id = s.get("pane_id").and_then(|v| v.as_u64());
            let attached = s
                .get("attached")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let workspace_name = s
                .get("workspace_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let created_at = s
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(DaemonSessionInfo {
                id,
                name,
                shell,
                cwd,
                pid,
                pane_id,
                attached,
                workspace_name,
                created_at,
            })
        })
        .collect()
}

/// Terminal session info fetched from the daemon.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DaemonSessionInfo {
    pub(crate) id: String,
    pub(crate) shell: String,
    pub(crate) cwd: String,
    pub(crate) pid: Option<i32>,
    pub(crate) pane_id: Option<u64>,
    pub(crate) attached: bool,
    pub(crate) workspace_name: Option<String>,
    pub(crate) name: String,
    pub(crate) created_at: String,
}

