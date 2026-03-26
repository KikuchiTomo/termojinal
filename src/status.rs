//! Status bar data collection and formatting.

use crate::platform::{detect_ssh_from_pid, get_child_cwd};
use crate::UserEvent;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winit::event_loop::EventLoopProxy;

#[derive(Clone, Default)]
pub(crate) struct StatusSnapshot {
    pub(crate) cwd: String,
    pub(crate) git_branch: String,
    pub(crate) git_worktree: String,
    pub(crate) git_stash: usize,
    pub(crate) git_ahead: usize,
    pub(crate) git_behind: usize,
    pub(crate) git_dirty: usize,
    pub(crate) git_untracked: usize,
    pub(crate) git_remote: String,
    pub(crate) ssh_user: String,
    pub(crate) ssh_host: String,
}

/// Async status info collector. Runs heavy commands (lsof, git, ps, ssh -G)
/// on a background thread so the render loop is never blocked.
pub(crate) struct AsyncStatusCollector {
    /// Latest snapshot, read by the render thread.
    snapshot: Arc<Mutex<StatusSnapshot>>,
    /// PID + OSC CWD to request from the background thread.
    request: Arc<Mutex<(i32, String)>>,
    /// Notify the background thread to wake up early (e.g., after PTY output).
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
    /// Global shutdown flag — checked each loop iteration.
    pub(crate) shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl AsyncStatusCollector {
    pub(crate) fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let snapshot = Arc::new(Mutex::new(StatusSnapshot::default()));
        let request = Arc::new(Mutex::new((0i32, String::new())));
        let notify = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let snap = Arc::clone(&snapshot);
        let req = Arc::clone(&request);
        let wake = Arc::clone(&notify);
        let shut = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("status-collector".into())
            .spawn(move || {
                loop {
                    if shut.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    // Wait up to 2 seconds, or wake immediately if nudged.
                    {
                        let (lock, cvar) = &*wake;
                        let mut nudged = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if !*nudged {
                            let (mut g, _) = cvar
                                .wait_timeout(nudged, std::time::Duration::from_secs(2))
                                .unwrap_or_else(|e| e.into_inner());
                            *g = false;
                        } else {
                            *nudged = false;
                        }
                    }

                    let (pid, osc_cwd) = {
                        let r = req.lock().unwrap_or_else(|e| e.into_inner());
                        (r.0, r.1.clone())
                    };
                    if pid == 0 {
                        continue;
                    }

                    // Resolve CWD: prefer OSC 7, fallback to lsof.
                    let cwd = if !osc_cwd.is_empty() {
                        osc_cwd
                    } else {
                        get_child_cwd(pid).unwrap_or_default()
                    };

                    let mut s = StatusSnapshot::default();
                    s.cwd = cwd.clone();

                    // Always collect git info (branch may change even if CWD doesn't).
                    if !cwd.is_empty() {
                        Self::collect_git(&cwd, &mut s);
                    }

                    // Always detect SSH (connection may start/stop).
                    if let Some((user, host)) = detect_ssh_from_pid(pid) {
                        s.ssh_user = user.unwrap_or_default();
                        s.ssh_host = host;
                    }

                    // Update snapshot and trigger redraw.
                    let changed = {
                        let mut current = snap.lock().unwrap_or_else(|e| e.into_inner());
                        let changed = current.cwd != s.cwd
                            || current.git_branch != s.git_branch
                            || current.git_dirty != s.git_dirty
                            || current.ssh_host != s.ssh_host;
                        *current = s;
                        changed
                    };
                    if changed {
                        let _ = proxy.send_event(UserEvent::StatusUpdate);
                    }
                }
            })
            .expect("failed to spawn status collector thread");

        Self {
            snapshot,
            request,
            notify,
            shutdown,
        }
    }

    /// Update the request (called from render thread — non-blocking).
    pub(crate) fn update_request(&self, pid: i32, osc_cwd: &str) {
        if let Ok(mut r) = self.request.try_lock() {
            r.0 = pid;
            r.1 = osc_cwd.to_string();
        }
    }

    /// Wake the background thread immediately (e.g., after PTY output).
    pub(crate) fn nudge(&self) {
        let (lock, cvar) = &*self.notify;
        if let Ok(mut nudged) = lock.try_lock() {
            *nudged = true;
            cvar.notify_one();
        }
    }

    /// Get the latest snapshot (called from render thread — non-blocking).
    pub(crate) fn get(&self) -> StatusSnapshot {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn collect_git(cwd: &str, s: &mut StatusSnapshot) {
        // Branch.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if out.status.success() {
                let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if b != "HEAD" {
                    s.git_branch = b;
                }
            }
        }
        // Worktree.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "rev-parse", "--show-toplevel"])
            .output()
        {
            if out.status.success() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                s.git_worktree = std::path::Path::new(&p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
        // Stash.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "stash", "list"])
            .output()
        {
            if out.status.success() {
                s.git_stash = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .count();
            }
        }
        // Ahead/behind.
        if let Ok(out) = std::process::Command::new("git")
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
            if out.status.success() {
                let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let parts: Vec<&str> = t.split_whitespace().collect();
                if parts.len() == 2 {
                    s.git_ahead = parts[0].parse().unwrap_or(0);
                    s.git_behind = parts[1].parse().unwrap_or(0);
                }
            }
        }
        // Dirty/untracked.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "status", "--porcelain"])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| !l.is_empty())
                {
                    if line.starts_with("??") {
                        s.git_untracked += 1;
                    } else {
                        s.git_dirty += 1;
                    }
                }
            }
        }
        // Remote URL.
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", cwd, "remote", "get-url", "origin"])
            .output()
        {
            if out.status.success() {
                s.git_remote = String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
        }
    }
}

pub(crate) struct PaneGitCache {
    /// The CWD that was used to compute this cache.
    pub(crate) cwd: String,
    pub(crate) git_branch: String,
    pub(crate) git_worktree: String,
    pub(crate) git_stash: usize,
    pub(crate) git_ahead: usize,
    pub(crate) git_behind: usize,
    pub(crate) git_dirty: usize,
    pub(crate) git_untracked: usize,
    pub(crate) git_remote: String,
    pub(crate) ssh_user: String,
    pub(crate) ssh_host: String,
    pub(crate) last_updated: Instant,
}

impl PaneGitCache {
    pub(crate) fn new() -> Self {
        Self {
            cwd: String::new(),
            git_branch: String::new(),
            git_worktree: String::new(),
            git_stash: 0,
            git_ahead: 0,
            git_behind: 0,
            git_dirty: 0,
            git_untracked: 0,
            git_remote: String::new(),
            ssh_user: String::new(),
            ssh_host: String::new(),
            last_updated: Instant::now() - std::time::Duration::from_secs(999),
        }
    }

    /// Update from async snapshot.
    pub(crate) fn update_from_snapshot(&mut self, snap: &StatusSnapshot) {
        self.cwd = snap.cwd.clone();
        self.git_branch = snap.git_branch.clone();
        self.git_worktree = snap.git_worktree.clone();
        self.git_stash = snap.git_stash;
        self.git_ahead = snap.git_ahead;
        self.git_behind = snap.git_behind;
        self.git_dirty = snap.git_dirty;
        self.git_untracked = snap.git_untracked;
        self.git_remote = snap.git_remote.clone();
        self.ssh_user = snap.ssh_user.clone();
        self.ssh_host = snap.ssh_host.clone();
        self.last_updated = Instant::now();
    }
}

// ---------------------------------------------------------------------------
// Status bar context — resolved variable values, collected once per frame
// ---------------------------------------------------------------------------

pub(crate) struct StatusContext {
    pub(crate) user: String,
    pub(crate) host: String,
    pub(crate) cwd: String,
    pub(crate) cwd_short: String,
    pub(crate) git_branch: String,
    pub(crate) git_status: String,
    pub(crate) git_remote: String,
    pub(crate) git_worktree: String,
    pub(crate) git_stash: String,
    pub(crate) git_ahead: String,
    pub(crate) git_behind: String,
    pub(crate) git_dirty: String,
    pub(crate) git_untracked: String,
    pub(crate) ports: String,
    pub(crate) shell: String,
    pub(crate) pid: String,
    pub(crate) pane_size: String,
    pub(crate) font_size: String,
    pub(crate) workspace: String,
    pub(crate) workspace_index: String,
    pub(crate) tab: String,
    pub(crate) tab_index: String,
    pub(crate) time: String,
    pub(crate) date: String,
}

/// Cached values that rarely change (user, host, shell).
pub(crate) struct StatusCache {
    pub(crate) user: String,
    pub(crate) host: String,
    pub(crate) shell: String,
    /// Last second for which time/date were computed.
    pub(crate) last_time_secs: u64,
    pub(crate) cached_time: String,
    pub(crate) cached_date: String,
}

impl StatusCache {
    pub(crate) fn new() -> Self {
        let user = std::env::var("USER").unwrap_or_default();
        let host = gethostname::gethostname().to_string_lossy().to_string();
        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::Path::new(&s)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        Self {
            user,
            host,
            shell,
            last_time_secs: 0,
            cached_time: String::new(),
            cached_date: String::new(),
        }
    }

    /// Update time/date cache if the second has changed. Returns (time, date).
    pub(crate) fn time_date(&mut self) -> (&str, &str) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if secs != self.last_time_secs {
            self.last_time_secs = secs;
            // Compute HH:MM and YYYY-MM-DD from unix timestamp (UTC-local approximation
            // via the `time` crate is unavailable, so we shell out or use a simple approach).
            // For simplicity, use chrono-free manual UTC conversion; the status bar will show
            // local time if we use libc localtime.
            #[cfg(unix)]
            {
                let t = secs as i64;
                let mut tm: libc::tm = unsafe { std::mem::zeroed() };
                unsafe { libc::localtime_r(&t as *const i64, &mut tm) };
                self.cached_time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
                self.cached_date = format!(
                    "{:04}-{:02}-{:02}",
                    tm.tm_year + 1900,
                    tm.tm_mon + 1,
                    tm.tm_mday
                );
            }
            #[cfg(not(unix))]
            {
                // Fallback: leave empty on non-unix.
                let _ = secs;
            }
        }
        (&self.cached_time, &self.cached_date)
    }
}

/// Expand `{variable}` placeholders in a status segment content string.
pub(crate) fn expand_status_variables(template: &str, ctx: &StatusContext) -> String {
    template
        .replace("{user}", &ctx.user)
        .replace("{host}", &ctx.host)
        .replace("{cwd_short}", &ctx.cwd_short)
        .replace("{cwd}", &ctx.cwd)
        .replace("{git_branch}", &ctx.git_branch)
        .replace("{git_status}", &ctx.git_status)
        .replace("{git_remote}", &ctx.git_remote)
        .replace("{git_worktree}", &ctx.git_worktree)
        .replace("{git_stash}", &ctx.git_stash)
        .replace("{git_ahead}", &ctx.git_ahead)
        .replace("{git_behind}", &ctx.git_behind)
        .replace("{git_dirty}", &ctx.git_dirty)
        .replace("{git_untracked}", &ctx.git_untracked)
        .replace("{ports}", &ctx.ports)
        .replace("{shell}", &ctx.shell)
        .replace("{pid}", &ctx.pid)
        .replace("{pane_size}", &ctx.pane_size)
        .replace("{font_size}", &ctx.font_size)
        .replace("{workspace}", &ctx.workspace)
        .replace("{workspace_index}", &ctx.workspace_index)
        .replace("{tab}", &ctx.tab)
        .replace("{tab_index}", &ctx.tab_index)
        .replace("{time}", &ctx.time)
        .replace("{date}", &ctx.date)
}

/// Check if an expanded segment is "empty" — only whitespace after variable expansion.
pub(crate) fn segment_is_empty(expanded: &str) -> bool {
    expanded.trim().is_empty()
}

// ---------------------------------------------------------------------------

