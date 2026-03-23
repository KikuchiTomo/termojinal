//! PTY fork/exec management for termojinal.
//!
//! Provides PTY creation, shell spawning, and I/O for fish/zsh/bash.

use nix::pty::{openpty, OpenptyResult, Winsize};
use nix::sys::signal::{self, Signal};
use nix::unistd::{dup2, execve, fork, read, setsid, write, ForkResult, Pid};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

pub use nix::unistd::Pid as ChildPid;

mod error;
pub use error::PtyError;

/// Terminal dimensions.
#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
        }
    }
}

impl PtySize {
    fn to_winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

/// Configuration for spawning a PTY.
pub struct PtyConfig {
    pub shell: String,
    pub size: PtySize,
    pub env: HashMap<String, String>,
    pub working_dir: Option<String>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            shell: detect_shell(),
            size: PtySize::default(),
            env: default_env(),
            working_dir: None,
        }
    }
}

/// A PTY master with a running child process.
pub struct Pty {
    master: OwnedFd,
    pid: Pid,
}

impl Pty {
    /// Spawn a new PTY with the given configuration.
    pub fn spawn(config: &PtyConfig) -> Result<Self, PtyError> {
        let winsize = config.size.to_winsize();

        let OpenptyResult { master, slave } =
            openpty(&winsize, None).map_err(|e| PtyError::Open(e.to_string()))?;

        match unsafe { fork() }.map_err(|e| PtyError::Fork(e.to_string()))? {
            ForkResult::Child => {
                // Drop master in child — we only use the slave side.
                drop(master);

                // Create a new session and set controlling terminal.
                setsid().ok();
                unsafe {
                    libc::ioctl(slave.as_raw_fd(), libc::TIOCSCTTY as _, 0);
                }

                // Redirect stdio to slave PTY.
                dup2(slave.as_raw_fd(), libc::STDIN_FILENO).ok();
                dup2(slave.as_raw_fd(), libc::STDOUT_FILENO).ok();
                dup2(slave.as_raw_fd(), libc::STDERR_FILENO).ok();
                if slave.as_raw_fd() > 2 {
                    drop(slave);
                }

                // Change working directory if specified.
                if let Some(ref dir) = config.working_dir {
                    std::env::set_current_dir(dir).ok();
                }

                // Build environment variables.
                let mut env_vars: Vec<CString> = Vec::new();
                for (key, val) in &config.env {
                    let entry = format!("{key}={val}");
                    if let Ok(cs) = CString::new(entry) {
                        env_vars.push(cs);
                    }
                }

                // Execute the shell.
                let shell_cstr =
                    CString::new(config.shell.as_str()).expect("invalid shell path");
                let login_arg = format!("-{}", shell_basename(&config.shell));
                let login_cstr = CString::new(login_arg).expect("invalid login arg");
                let args = [login_cstr];

                // This never returns on success.
                let _ = execve(&shell_cstr, &args, &env_vars);
                std::process::exit(1);
            }
            ForkResult::Parent { child } => {
                drop(slave);
                log::info!("PTY spawned: pid={child}, shell={}", config.shell);
                Ok(Pty { master, pid: child })
            }
        }
    }

    /// Read bytes from the PTY master.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, PtyError> {
        read(self.master.as_raw_fd(), buf).map_err(|e| PtyError::Io(e.to_string()))
    }

    /// Write bytes to the PTY master.
    pub fn write(&self, data: &[u8]) -> Result<usize, PtyError> {
        write(&self.master, data).map_err(|e| PtyError::Io(e.to_string()))
    }

    /// Resize the PTY.
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        let ws = size.to_winsize();
        let ret = unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as _,
                &ws as *const Winsize,
            )
        };
        if ret == -1 {
            return Err(PtyError::Resize(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // Notify the child process group of the size change.
        signal::kill(self.pid, Signal::SIGWINCH).ok();
        Ok(())
    }

    /// Get the raw file descriptor for the master side (for polling).
    pub fn master_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Get the child process PID.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Check if the child process is still alive.
    pub fn is_alive(&self) -> bool {
        use nix::sys::wait::{waitpid, WaitPidFlag};
        matches!(
            waitpid(self.pid, Some(WaitPidFlag::WNOHANG)),
            Ok(nix::sys::wait::WaitStatus::StillAlive)
        )
    }

    /// Send a signal to the child process.
    pub fn signal(&self, sig: Signal) -> Result<(), PtyError> {
        signal::kill(self.pid, sig).map_err(|e| PtyError::Signal(e.to_string()))
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Send SIGHUP to the child when the PTY is dropped.
        let _ = signal::kill(self.pid, Signal::SIGHUP);
    }
}

/// Detect the user's default shell from $SHELL, falling back to /bin/zsh.
pub fn detect_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Build the default environment for a PTY session.
///
/// Inherits all environment variables from the parent process so that
/// SSH agent (SSH_AUTH_SOCK, SSH_AGENT_PID), Claude Code authentication,
/// and other credentials are available in split panes and new workspaces.
/// Terminal-specific variables are then overridden with values appropriate
/// for a termojinal PTY.
pub fn default_env() -> HashMap<String, String> {
    // Start with the full parent environment so that SSH agent forwarding,
    // auth tokens, and tool-specific variables are inherited.
    let mut env: HashMap<String, String> = std::env::vars().collect();

    // Remove variables that are per-session or would be stale/incorrect
    // in a child PTY.
    let exclude = [
        "TERM_SESSION_ID",   // macOS per-terminal-session identifier
        "WINDOWID",          // X11 window id, not valid for child PTYs
        "SECURITYSESSIONID", // macOS security session, inherited automatically by the kernel
        "OLDPWD",            // stale previous working directory
        "_",                 // last command, not meaningful for new shell
    ];
    for key in exclude {
        env.remove(key);
    }

    // Override terminal-specific variables.
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    // Identify as termojinal so tools can detect us.
    // Also set as ghostty-compatible for Claude Code OSC 777 notifications.
    env.insert("TERM_PROGRAM".to_string(), "termojinal".to_string());
    env.insert("TERM_PROGRAM_VERSION".to_string(), "0.1.0".to_string());

    env
}

fn shell_basename(shell: &str) -> String {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sh")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_shell() {
        let shell = detect_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn test_default_env() {
        let env = default_env();
        assert_eq!(env.get("TERM").unwrap(), "xterm-256color");
        assert_eq!(env.get("COLORTERM").unwrap(), "truecolor");
    }

    #[test]
    fn test_default_env_inherits_parent_vars() {
        // Set a test variable in the parent environment.
        std::env::set_var("TERMOJINAL_TEST_INHERIT", "inherited_value");
        let env = default_env();
        assert_eq!(
            env.get("TERMOJINAL_TEST_INHERIT").unwrap(),
            "inherited_value",
            "default_env should inherit arbitrary parent environment variables"
        );
        std::env::remove_var("TERMOJINAL_TEST_INHERIT");
    }

    #[test]
    fn test_default_env_inherits_ssh_vars() {
        // Simulate SSH agent variables being present in the parent.
        std::env::set_var("SSH_AUTH_SOCK", "/tmp/test-ssh-sock");
        std::env::set_var("SSH_AGENT_PID", "12345");
        let env = default_env();
        assert_eq!(
            env.get("SSH_AUTH_SOCK").unwrap(),
            "/tmp/test-ssh-sock",
            "SSH_AUTH_SOCK must be inherited for SSH agent forwarding"
        );
        assert_eq!(
            env.get("SSH_AGENT_PID").unwrap(),
            "12345",
            "SSH_AGENT_PID must be inherited for SSH agent forwarding"
        );
        std::env::remove_var("SSH_AUTH_SOCK");
        std::env::remove_var("SSH_AGENT_PID");
    }

    #[test]
    fn test_default_env_excludes_stale_vars() {
        std::env::set_var("TERM_SESSION_ID", "stale-session");
        std::env::set_var("OLDPWD", "/old/path");
        let env = default_env();
        assert!(
            env.get("TERM_SESSION_ID").is_none(),
            "TERM_SESSION_ID should be excluded"
        );
        assert!(
            env.get("OLDPWD").is_none(),
            "OLDPWD should be excluded"
        );
        std::env::remove_var("TERM_SESSION_ID");
        std::env::remove_var("OLDPWD");
    }

    #[test]
    fn test_pty_size_default() {
        let size = PtySize::default();
        assert_eq!(size.cols, 80);
        assert_eq!(size.rows, 24);
    }
}
