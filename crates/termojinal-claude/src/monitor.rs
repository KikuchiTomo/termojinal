//! Claude Code session monitor — detects Claude Code processes running in PTY
//! panes and monitors their state by reading session files from `~/.claude/`.
//!
//! The monitor runs a background thread that periodically:
//! 1. Walks PTY child process trees to find `claude` processes
//! 2. Reads `~/.claude/sessions/<pid>.json` for session metadata
//! 3. Reads JSONL files for task title and activity state
//! 4. Reads subagent metadata from session subdirectories
//!
//! Results are shared with the render thread via `Arc<Mutex>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Detected Claude Code session state.
#[derive(Debug, Clone)]
pub struct ClaudeSession {
    /// PTY pane ID in jterm.
    pub pane_id: u64,
    /// Workspace index this pane belongs to.
    pub workspace_idx: usize,
    /// Claude Code process PID.
    pub claude_pid: i32,
    /// Claude Code session UUID.
    pub session_id: String,
    /// Task title (first user message from JSONL).
    pub title: String,
    /// Working directory.
    pub cwd: String,
    /// Current state.
    pub state: SessionState,
    /// Active subagents.
    pub subagents: Vec<SubAgentState>,
    /// When this session was started (unix ms).
    pub started_at: u64,
}

/// Session activity state.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    /// JSONL modified within last 10 seconds.
    Running,
    /// JSONL modified 10-60 seconds ago.
    Idle,
    /// JSONL not modified for >60 seconds (or process exited).
    Done,
    /// A PermissionRequest is pending.
    WaitingForPermission,
}

/// Subagent state.
#[derive(Debug, Clone)]
pub struct SubAgentState {
    pub agent_id: String,
    pub agent_type: String,
    pub description: String,
    pub state: SessionState,
}

/// A request from the render thread: which pane PIDs to monitor.
#[derive(Clone)]
pub struct PaneInfo {
    pub pane_id: u64,
    pub workspace_idx: usize,
    pub pty_pid: i32,
}

/// Background thread that monitors Claude Code sessions.
pub struct ClaudeSessionMonitor {
    /// Latest detected sessions.
    sessions: Arc<Mutex<Vec<ClaudeSession>>>,
    /// Pane PID info to scan (submitted by render thread).
    pane_infos: Arc<Mutex<Vec<PaneInfo>>>,
    /// Wake signal.
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl ClaudeSessionMonitor {
    /// Start the monitor background thread.
    pub fn new() -> Self {
        let sessions: Arc<Mutex<Vec<ClaudeSession>>> = Arc::new(Mutex::new(Vec::new()));
        let pane_infos: Arc<Mutex<Vec<PaneInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

        let sess = Arc::clone(&sessions);
        let panes = Arc::clone(&pane_infos);
        let wake = Arc::clone(&notify);

        std::thread::Builder::new()
            .name("claude-session-monitor".into())
            .spawn(move || {
                // Cache: claude_pid -> (session_id, title) to avoid re-reading files every cycle.
                let mut title_cache: HashMap<i32, (String, String)> = HashMap::new();

                loop {
                    // Wait 3 seconds or wake immediately.
                    {
                        let (lock, cvar) = &*wake;
                        let mut nudged = lock.lock().unwrap();
                        if !*nudged {
                            let (mut g, _) = cvar.wait_timeout(
                                nudged,
                                Duration::from_secs(3),
                            ).unwrap();
                            *g = false;
                        } else {
                            *nudged = false;
                        }
                    }

                    // Get current pane list.
                    let infos: Vec<PaneInfo> = panes.lock().unwrap().clone();
                    if infos.is_empty() {
                        *sess.lock().unwrap() = Vec::new();
                        continue;
                    }

                    // Build process tree once.
                    let child_map = build_child_map();

                    let mut detected: Vec<ClaudeSession> = Vec::new();

                    for info in &infos {
                        // Find claude process among PTY children.
                        if let Some(claude_pid) = find_claude_child(&child_map, info.pty_pid) {
                            // Read session file.
                            let (session_id, title, cwd, started_at) =
                                if let Some((sid, t)) = title_cache.get(&claude_pid) {
                                    // Use cached title, but re-read session file for cwd
                                    // and verify session_id hasn't changed (new session
                                    // on the same PID should bust the cache).
                                    let sf = read_session_file(claude_pid);
                                    let session_changed = sf.as_ref()
                                        .map(|s| s.session_id != *sid)
                                        .unwrap_or(false);
                                    if session_changed {
                                        // Session changed — re-read title.
                                        let sf = sf.unwrap();
                                        let title = read_task_title(&sf.session_id, &sf.cwd);
                                        title_cache.insert(claude_pid, (sf.session_id.clone(), title.clone()));
                                        (sf.session_id, title, sf.cwd, sf.started_at)
                                    } else {
                                        let cwd = sf.as_ref().map(|s| s.cwd.clone()).unwrap_or_default();
                                        let started = sf.as_ref().map(|s| s.started_at).unwrap_or(0);
                                        (sid.clone(), t.clone(), cwd, started)
                                    }
                                } else if let Some(sf) = read_session_file(claude_pid) {
                                    let title = read_task_title(&sf.session_id, &sf.cwd);
                                    title_cache.insert(claude_pid, (sf.session_id.clone(), title.clone()));
                                    (sf.session_id, title, sf.cwd, sf.started_at)
                                } else {
                                    continue;
                                };

                            // Determine state from JSONL mtime + process liveness.
                            let state = detect_session_state(&session_id, &cwd, claude_pid);

                            // Read subagents.
                            let subagents = read_subagents(&session_id, &cwd);

                            detected.push(ClaudeSession {
                                pane_id: info.pane_id,
                                workspace_idx: info.workspace_idx,
                                claude_pid,
                                session_id,
                                title,
                                cwd,
                                state,
                                subagents,
                                started_at,
                            });
                        }
                    }

                    // Clean up title cache for dead sessions.
                    let live_pids: Vec<i32> = detected.iter().map(|s| s.claude_pid).collect();
                    title_cache.retain(|pid, _| live_pids.contains(pid));

                    *sess.lock().unwrap() = detected;
                }
            })
            .expect("failed to spawn claude session monitor thread");

        Self { sessions, pane_infos, notify }
    }

    /// Submit pane info for scanning (non-blocking).
    pub fn submit_panes(&self, panes: Vec<PaneInfo>) {
        if let Ok(mut p) = self.pane_infos.try_lock() {
            *p = panes;
        }
        let (lock, cvar) = &*self.notify;
        if let Ok(mut nudged) = lock.try_lock() {
            *nudged = true;
            cvar.notify_one();
        }
    }

    /// Get latest detected sessions (non-blocking).
    pub fn get_sessions(&self) -> Vec<ClaudeSession> {
        self.sessions.try_lock().map(|s| s.clone()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Session file data from `~/.claude/sessions/<pid>.json`.
struct SessionFileData {
    session_id: String,
    cwd: String,
    started_at: u64,
}

fn read_session_file(claude_pid: i32) -> Option<SessionFileData> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join("sessions").join(format!("{claude_pid}.json"));
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    Some(SessionFileData {
        session_id: json.get("sessionId")?.as_str()?.to_string(),
        cwd: json.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        started_at: json.get("startedAt").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

/// Convert CWD to Claude project path component.
fn cwd_to_project_path(cwd: &str) -> String {
    cwd.replace('/', "-")
}

/// Find the JSONL file path for a session.
fn session_jsonl_path(session_id: &str, cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let project_dir = cwd_to_project_path(cwd);
    Some(home.join(".claude").join("projects").join(&project_dir).join(format!("{session_id}.jsonl")))
}

/// Read the task title from the first user message in the JSONL file.
fn read_task_title(session_id: &str, cwd: &str) -> String {
    let Some(path) = session_jsonl_path(session_id, cwd) else {
        return String::new();
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return String::new();
    };
    use std::io::BufRead;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().take(10) {
        let Ok(line) = line else { continue; };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
        if json.get("type").and_then(|v| v.as_str()) == Some("user") {
            // Extract text from message content.
            if let Some(msg) = json.get("message").and_then(|m| m.get("content")) {
                if let Some(text) = msg.as_str() {
                    // Truncate to first line, max 80 chars.
                    let first_line = text.lines().next().unwrap_or("");
                    return first_line.chars().take(80).collect();
                }
                if let Some(arr) = msg.as_array() {
                    for item in arr {
                        if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                let first_line = text.lines().next().unwrap_or("");
                                return first_line.chars().take(80).collect();
                            }
                        }
                    }
                }
            }
        }
    }
    String::new()
}

/// Detect session state from JSONL file modification time and process liveness.
///
/// If the claude process is still alive (kill -0 succeeds), the state will
/// never drop below `Idle` — even if the JSONL hasn't been modified for a
/// long time (e.g. while Claude is thinking or executing a tool).
fn detect_session_state(session_id: &str, cwd: &str, claude_pid: i32) -> SessionState {
    let process_alive = unsafe { libc::kill(claude_pid, 0) } == 0;

    let Some(path) = session_jsonl_path(session_id, cwd) else {
        return if process_alive { SessionState::Idle } else { SessionState::Done };
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return if process_alive { SessionState::Idle } else { SessionState::Done };
    };
    let Ok(modified) = metadata.modified() else {
        return if process_alive { SessionState::Idle } else { SessionState::Done };
    };
    let elapsed = SystemTime::now().duration_since(modified).unwrap_or(Duration::from_secs(999));
    if elapsed.as_secs() < 10 {
        SessionState::Running
    } else if process_alive {
        // Process is alive but JSONL hasn't been updated recently —
        // Claude is likely thinking or executing a tool.
        SessionState::Running
    } else {
        SessionState::Done
    }
}

/// Read subagent metadata from the session's subagents directory.
fn read_subagents(session_id: &str, cwd: &str) -> Vec<SubAgentState> {
    let Some(home) = dirs::home_dir() else { return Vec::new(); };
    let project_dir = cwd_to_project_path(cwd);
    let subagent_dir = home.join(".claude").join("projects").join(&project_dir)
        .join(session_id).join("subagents");

    let Ok(entries) = std::fs::read_dir(&subagent_dir) else {
        return Vec::new();
    };

    let mut subagents = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".meta.json") { continue; }
        let agent_id = name.trim_start_matches("agent-").trim_end_matches(".meta.json").to_string();

        let Ok(content) = std::fs::read_to_string(entry.path()) else { continue; };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { continue; };

        let agent_type = json.get("agentType").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let description = json.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // Check activity from JSONL mtime.
        let jsonl_path = subagent_dir.join(format!("agent-{agent_id}.jsonl"));
        let state = if let Ok(meta) = std::fs::metadata(&jsonl_path) {
            if let Ok(modified) = meta.modified() {
                let elapsed = SystemTime::now().duration_since(modified).unwrap_or(Duration::from_secs(999));
                if elapsed.as_secs() < 15 { SessionState::Running } else { SessionState::Done }
            } else {
                SessionState::Done
            }
        } else {
            SessionState::Done
        };

        // Only include running subagents.
        if state == SessionState::Running {
            subagents.push(SubAgentState {
                agent_id,
                agent_type,
                description,
                state,
            });
        }
    }
    subagents
}

/// Build a child process map: parent_pid -> Vec<(child_pid, command)>.
fn build_child_map() -> HashMap<i32, Vec<(i32, String)>> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["ax", "-o", "ppid=,pid=,command="])
        .output()
    else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut map: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.trim().splitn(3, char::is_whitespace).collect();
        if parts.len() < 3 { continue; }
        let ppid: i32 = parts[0].trim().parse().unwrap_or(0);
        let pid: i32 = parts[1].trim().parse().unwrap_or(0);
        let cmd = parts[2].trim().to_string();
        if ppid > 0 && pid > 0 {
            map.entry(ppid).or_default().push((pid, cmd));
        }
    }
    map
}

/// BFS search for a "claude" process among children of the given PID.
fn find_claude_child(child_map: &HashMap<i32, Vec<(i32, String)>>, root_pid: i32) -> Option<i32> {
    let mut queue = vec![root_pid];
    let mut visited = std::collections::HashSet::new();
    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) { continue; }
        if let Some(children) = child_map.get(&pid) {
            for (child_pid, cmd) in children {
                // Match "claude" binary (not Claude.app).
                let basename = cmd.split('/').last().unwrap_or(cmd);
                let first_word = basename.split_whitespace().next().unwrap_or("");
                if first_word == "claude" {
                    return Some(*child_pid);
                }
                queue.push(*child_pid);
            }
        }
    }
    None
}
