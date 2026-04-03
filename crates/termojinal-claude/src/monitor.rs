//! Claude Code session monitor — detects Claude Code processes running in PTY
//! panes and monitors their state.
//!
//! State detection uses two mechanisms:
//!
//! 1. **Hooks (primary)**: Claude Code hooks call `tm status running|done` which
//!    sends an IPC message. The monitor stores these events in a `HooksStateStore`
//!    and uses them as the authoritative state source.
//!
//! 2. **Process tree scan (fallback)**: A background thread walks PTY child
//!    process trees to find `claude` processes. When hooks are not configured,
//!    the process is detected but state defaults to `Idle`.
//!
//! Title reading from `~/.claude/` JSONL files is retained for the initial
//! detection (first user message = task title).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Claude Code is actively working (hook reported "running").
    Running,
    /// Claude Code process exists but no recent hook events.
    Idle,
    /// Task completed (hook reported "done" or process exited).
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

/// A hooks-based status update received via IPC (`tm status`).
#[derive(Debug, Clone)]
pub struct HooksStatusEvent {
    pub session_id: Option<String>,
    pub state: String,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub pid: Option<i32>,
    pub received_at: Instant,
}

/// Store for hooks-based state. Thread-safe; shared between IPC handler and
/// the monitor background thread.
#[derive(Clone)]
pub struct HooksStateStore {
    inner: Arc<Mutex<HooksStateInner>>,
}

struct HooksStateInner {
    /// PID -> latest event (for main Claude process state).
    pid_events: HashMap<i32, HooksStatusEvent>,
    /// (PID, agent_id) -> latest subagent event.
    subagent_events: HashMap<(i32, String), HooksStatusEvent>,
}

impl HooksStateStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HooksStateInner {
                pid_events: HashMap::new(),
                subagent_events: HashMap::new(),
            })),
        }
    }

    /// Record a status event from a hook.
    pub fn record_event(&self, event: HooksStatusEvent) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = event.pid {
            if let Some(ref agent_id) = event.agent_id {
                inner
                    .subagent_events
                    .insert((pid, agent_id.clone()), event);
            } else {
                inner.pid_events.insert(pid, event);
            }
        }
    }

    /// Look up the latest state for a given PID. Returns `None` if no hook
    /// event has been recorded for this PID.
    pub fn get_state(&self, pid: i32) -> Option<SessionState> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let event = inner.pid_events.get(&pid)?;
        Some(parse_hook_state(&event.state, event.received_at))
    }

    /// Get active subagents for a given PID.
    pub fn get_subagents(&self, pid: i32) -> Vec<SubAgentState> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut subagents = Vec::new();
        for ((p, _), event) in &inner.subagent_events {
            if *p != pid {
                continue;
            }
            let state = parse_hook_state(&event.state, event.received_at);
            // Only include running subagents.
            if state == SessionState::Running {
                subagents.push(SubAgentState {
                    agent_id: event.agent_id.clone().unwrap_or_default(),
                    agent_type: event.agent_type.clone().unwrap_or_default(),
                    description: event.description.clone().unwrap_or_default(),
                    state,
                });
            }
        }
        subagents
    }

    /// Evict entries for PIDs that are no longer alive or have not reported
    /// in a long time (> 10 minutes).
    pub fn evict_stale(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let cutoff = Instant::now() - Duration::from_secs(600);
        inner.pid_events.retain(|pid, evt| {
            evt.received_at > cutoff && is_process_alive(*pid)
        });
        inner.subagent_events.retain(|(pid, _), evt| {
            evt.received_at > cutoff && is_process_alive(*pid)
        });
    }
}

/// Parse a hook state string into a `SessionState`.
///
/// "running" maps to `Running`. "done" maps to `Done`. Anything else
/// (including "idle") maps to `Idle`.
///
/// For "running", if the event is older than 120 seconds without a new
/// event, we treat it as idle (hooks should fire frequently during active
/// work).
fn parse_hook_state(state: &str, received_at: Instant) -> SessionState {
    match state {
        "running" => {
            let age = Instant::now().duration_since(received_at);
            if age > Duration::from_secs(600) {
                SessionState::Idle
            } else {
                SessionState::Running
            }
        }
        "done" => SessionState::Done,
        _ => SessionState::Idle,
    }
}

fn is_process_alive(pid: i32) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    result == 0
}

/// Background thread that monitors Claude Code sessions.
pub struct ClaudeSessionMonitor {
    /// Latest detected sessions.
    sessions: Arc<Mutex<Vec<ClaudeSession>>>,
    /// Pane PID info to scan (submitted by render thread).
    pane_infos: Arc<Mutex<Vec<PaneInfo>>>,
    /// Wake signal.
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
    /// Hooks-based state store (shared with IPC handler).
    hooks_store: HooksStateStore,
}

impl ClaudeSessionMonitor {
    /// Start the monitor background thread.
    pub fn new() -> Self {
        let sessions: Arc<Mutex<Vec<ClaudeSession>>> = Arc::new(Mutex::new(Vec::new()));
        let pane_infos: Arc<Mutex<Vec<PaneInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let hooks_store = HooksStateStore::new();

        let sess = Arc::clone(&sessions);
        let panes = Arc::clone(&pane_infos);
        let wake = Arc::clone(&notify);
        let store = hooks_store.clone();

        std::thread::Builder::new()
            .name("claude-session-monitor".into())
            .spawn(move || {
                // Cache: claude_pid -> (session_id, title) to avoid re-reading files every cycle.
                let mut title_cache: HashMap<i32, (String, String)> = HashMap::new();
                let mut evict_counter: u32 = 0;

                loop {
                    // Wait 3 seconds or wake immediately.
                    {
                        let (lock, cvar) = &*wake;
                        let mut nudged = lock.lock().unwrap_or_else(|e| e.into_inner());
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
                    let infos: Vec<PaneInfo> = panes.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if infos.is_empty() {
                        *sess.lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();
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
                                    let sf = read_session_file(claude_pid);
                                    let session_changed = sf.as_ref()
                                        .map(|s| s.session_id != *sid)
                                        .unwrap_or(false);
                                    if session_changed {
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

                            // Determine state: hooks store takes priority, fall back to
                            // process-alive check (Idle if alive, Done if dead).
                            let state = store.get_state(claude_pid).unwrap_or_else(|| {
                                if is_process_alive(claude_pid) {
                                    SessionState::Idle
                                } else {
                                    SessionState::Done
                                }
                            });

                            // Subagents from hooks store.
                            let subagents = store.get_subagents(claude_pid);

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

                    *sess.lock().unwrap_or_else(|e| e.into_inner()) = detected;

                    // Periodically evict stale hooks store entries (every ~10 cycles = 30s).
                    evict_counter += 1;
                    if evict_counter >= 10 {
                        evict_counter = 0;
                        store.evict_stale();
                    }
                }
            })
            .expect("failed to spawn claude session monitor thread");

        Self { sessions, pane_infos, notify, hooks_store }
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

    /// Get a reference to the hooks state store.
    ///
    /// The GUI / daemon can use this to forward `ClaudeStatusUpdate` IPC
    /// events into the monitor without going through the background thread.
    pub fn hooks_store(&self) -> &HooksStateStore {
        &self.hooks_store
    }

    /// Wake the background thread immediately (e.g. after a hooks event).
    pub fn wake(&self) {
        let (lock, cvar) = &*self.notify;
        if let Ok(mut nudged) = lock.try_lock() {
            *nudged = true;
            cvar.notify_one();
        }
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
            if let Some(msg) = json.get("message").and_then(|m| m.get("content")) {
                if let Some(text) = msg.as_str() {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_state_store_record_and_get() {
        let store = HooksStateStore::new();
        assert!(store.get_state(100).is_none());

        store.record_event(HooksStatusEvent {
            session_id: Some("sess-1".to_string()),
            state: "running".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: Some(100),
            received_at: Instant::now(),
        });

        let state = store.get_state(100);
        assert_eq!(state, Some(SessionState::Running));
    }

    #[test]
    fn test_hooks_state_store_done() {
        let store = HooksStateStore::new();

        store.record_event(HooksStatusEvent {
            session_id: None,
            state: "done".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: Some(200),
            received_at: Instant::now(),
        });

        assert_eq!(store.get_state(200), Some(SessionState::Done));
    }

    #[test]
    fn test_hooks_state_store_overwrite() {
        let store = HooksStateStore::new();

        store.record_event(HooksStatusEvent {
            session_id: None,
            state: "running".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: Some(300),
            received_at: Instant::now(),
        });
        assert_eq!(store.get_state(300), Some(SessionState::Running));

        store.record_event(HooksStatusEvent {
            session_id: None,
            state: "done".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: Some(300),
            received_at: Instant::now(),
        });
        assert_eq!(store.get_state(300), Some(SessionState::Done));
    }

    #[test]
    fn test_hooks_state_store_subagents() {
        let store = HooksStateStore::new();

        store.record_event(HooksStatusEvent {
            session_id: Some("sess-1".to_string()),
            state: "running".to_string(),
            agent_id: Some("agent-42".to_string()),
            agent_type: Some("task".to_string()),
            description: Some("fixing tests".to_string()),
            pid: Some(400),
            received_at: Instant::now(),
        });

        let subs = store.get_subagents(400);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].agent_id, "agent-42");
        assert_eq!(subs[0].agent_type, "task");
        assert_eq!(subs[0].description, "fixing tests");
        assert_eq!(subs[0].state, SessionState::Running);

        assert!(store.get_subagents(999).is_empty());
    }

    #[test]
    fn test_hooks_state_store_subagent_done() {
        let store = HooksStateStore::new();

        store.record_event(HooksStatusEvent {
            session_id: None,
            state: "running".to_string(),
            agent_id: Some("a-1".to_string()),
            agent_type: Some("task".to_string()),
            description: Some("work".to_string()),
            pid: Some(500),
            received_at: Instant::now(),
        });
        assert_eq!(store.get_subagents(500).len(), 1);

        store.record_event(HooksStatusEvent {
            session_id: None,
            state: "done".to_string(),
            agent_id: Some("a-1".to_string()),
            agent_type: None,
            description: None,
            pid: Some(500),
            received_at: Instant::now(),
        });
        assert!(store.get_subagents(500).is_empty());
    }

    #[test]
    fn test_hooks_state_store_no_pid_ignored() {
        let store = HooksStateStore::new();

        store.record_event(HooksStatusEvent {
            session_id: Some("sess-1".to_string()),
            state: "running".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: None,
            received_at: Instant::now(),
        });

        assert!(store.get_state(0).is_none());
    }

    #[test]
    fn test_parse_hook_state_values() {
        let now = Instant::now();
        assert_eq!(parse_hook_state("running", now), SessionState::Running);
        assert_eq!(parse_hook_state("done", now), SessionState::Done);
        assert_eq!(parse_hook_state("idle", now), SessionState::Idle);
        assert_eq!(parse_hook_state("unknown", now), SessionState::Idle);
    }

    #[test]
    fn test_parse_hook_state_stale_running() {
        let old = Instant::now() - Duration::from_secs(200);
        assert_eq!(parse_hook_state("running", old), SessionState::Idle);
    }

    #[test]
    fn test_cwd_to_project_path() {
        assert_eq!(cwd_to_project_path("/home/user/project"), "-home-user-project");
        assert_eq!(cwd_to_project_path("/"), "-");
    }

    #[test]
    fn test_find_claude_child_empty_map() {
        let map = HashMap::new();
        assert!(find_claude_child(&map, 1).is_none());
    }

    #[test]
    fn test_find_claude_child_found() {
        let mut map: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
        map.insert(1, vec![(2, "/bin/bash".to_string())]);
        map.insert(2, vec![(3, "/usr/local/bin/claude --resume".to_string())]);
        assert_eq!(find_claude_child(&map, 1), Some(3));
    }

    #[test]
    fn test_find_claude_child_not_claude_app() {
        let mut map: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
        map.insert(1, vec![(2, "/Applications/Claude.app/Contents/MacOS/Claude".to_string())]);
        assert!(find_claude_child(&map, 1).is_none());
    }
}

// Public JSONL stats for the Claudes Dashboard
// ---------------------------------------------------------------------------

/// Statistics extracted from a Claude Code session's JSONL file.
#[derive(Debug, Clone, Default)]
pub struct SessionJsonlStats {
    /// Model name (e.g. "claude-opus-4-6", "claude-sonnet-4-20250514").
    pub model: String,
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens consumed.
    pub output_tokens: u64,
    /// Total cache-read tokens consumed.
    pub cache_read_tokens: u64,
    /// Estimated total cost in USD.
    pub cost_estimate: f64,
    /// Tool usage counts: tool_name -> count.
    pub tool_usage: HashMap<String, u32>,
    /// Estimated context window size for the model.
    pub context_max: u64,
}

/// Read stats from the JSONL file for a session.
///
/// Scans every line of the JSONL file and aggregates token counts, detects the
/// model name, and counts tool invocations. This function does file I/O and
/// should only be called from a background thread or when the dashboard is visible.
pub fn read_session_jsonl_stats(session_id: &str, cwd: &str) -> SessionJsonlStats {
    let mut stats = SessionJsonlStats::default();
    let Some(path) = session_jsonl_path(session_id, cwd) else {
        return stats;
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return stats;
    };
    use std::io::BufRead;
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else { continue };

        let entry_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if entry_type == "assistant" {
            // The Claude Code JSONL nests `model` and `usage` inside `message`.
            let msg = json.get("message");

            // Extract model name from assistant messages.
            if let Some(model) = msg.and_then(|m| m.get("model")).and_then(|v| v.as_str()) {
                if !model.is_empty() {
                    stats.model = model.to_string();
                }
            }

            // Extract usage stats.
            if let Some(usage) = msg.and_then(|m| m.get("usage")) {
                if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                    stats.input_tokens += input;
                }
                if let Some(output) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                    stats.output_tokens += output;
                }
                if let Some(cache) = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()) {
                    stats.cache_read_tokens += cache;
                }
            }

            // Count tool_use blocks in assistant content.
            if let Some(content) = msg
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        if let Some(name) = block.get("name").and_then(|v| v.as_str()) {
                            *stats.tool_usage.entry(name.to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }

    // Estimate context max from model name.
    stats.context_max = model_context_size(&stats.model);

    // Estimate cost based on model and tokens.
    stats.cost_estimate = estimate_cost(
        &stats.model,
        stats.input_tokens,
        stats.output_tokens,
        stats.cache_read_tokens,
    );

    stats
}

/// Estimate the context window size for a given model name.
///
/// Returns 0 if the context size cannot be determined from the model string.
/// The dashboard should handle 0 gracefully (e.g. show tokens without a max).
fn model_context_size(model: &str) -> u64 {
    // Parse explicit context annotation like "[1m]" or "[200k]".
    // This is the only reliable source; model version numbers change.
    let lower = model.to_lowercase();
    if let Some(start) = lower.find('[') {
        if let Some(end) = lower[start..].find(']') {
            let inner = &lower[start + 1..start + end];
            if let Some(num_end) = inner.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(num) = inner[..num_end].parse::<u64>() {
                    let suffix = &inner[num_end..];
                    return match suffix {
                        "m" => num * 1_000_000,
                        "k" => num * 1_000,
                        _ => 0,
                    };
                }
            }
        }
    }
    0
}

/// Estimate cost in USD based on model pricing and token usage.
///
/// Pricing (per 1M tokens, as of 2025):
/// - opus:   input=$15, output=$75, cache_read=$1.50
/// - sonnet: input=$3, output=$15, cache_read=$0.30
/// - haiku:  input=$0.80, output=$4, cache_read=$0.08
fn estimate_cost(model: &str, input: u64, output: u64, cache_read: u64) -> f64 {
    let (input_rate, output_rate, cache_rate) = if model.contains("opus") {
        (15.0, 75.0, 1.5)
    } else if model.contains("sonnet") {
        (3.0, 15.0, 0.30)
    } else if model.contains("haiku") {
        (0.80, 4.0, 0.08)
    } else {
        (3.0, 15.0, 0.30) // default to sonnet pricing
    };
    let per_million = 1_000_000.0;
    (input as f64 * input_rate / per_million)
        + (output as f64 * output_rate / per_million)
        + (cache_read as f64 * cache_rate / per_million)
}

/// Extract a short display name from a full model identifier.
///
/// e.g. "claude-opus-4-6" -> "opus"
///      "claude-sonnet-4-20250514" -> "sonnet"
pub fn model_short_name(model: &str) -> &str {
    if model.contains("opus") {
        "opus"
    } else if model.contains("sonnet") {
        "sonnet"
    } else if model.contains("haiku") {
        "haiku"
    } else if model.is_empty() {
        "unknown"
    } else {
        model
    }
}
