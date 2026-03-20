//! Command execution engine.
//!
//! [`CommandRunner`] spawns a command script as a child process and manages
//! the stdio JSON protocol. Messages from the script arrive on stdout
//! and are read on a background thread (to avoid blocking the main event
//! loop). Responses are written to the script's stdin.

use crate::command_loader::LoadedCommand;
use crate::command_protocol::{CommandMessage, CommandResponse};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

/// Status of the command runner.
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerStatus {
    /// The runner is waiting for the next JSON message from the child process.
    WaitingForMessage,

    /// The runner has received a message that requires user input and is
    /// waiting for a [`CommandResponse`] via [`CommandRunner::respond`].
    WaitingForInput,

    /// The command finished successfully. The optional string is a
    /// notification message for macOS.
    Done(Option<String>),

    /// The command encountered an error.
    Error(String),
}

/// Events produced by the background reader thread.
enum ReaderEvent {
    /// A parsed [`CommandMessage`] from the child's stdout.
    Message(CommandMessage),

    /// The child's stdout reached EOF (process likely exited).
    Eof,

    /// An I/O or JSON parse error occurred while reading.
    ReadError(String),
}

/// Runs a command script and manages the stdio JSON protocol.
///
/// The runner spawns the script, reads its stdout on a background thread
/// via an `mpsc` channel, and provides a non-blocking [`poll`](Self::poll)
/// method that the main event loop can call each frame.
pub struct CommandRunner {
    child: Child,
    receiver: mpsc::Receiver<ReaderEvent>,
    current_message: Option<CommandMessage>,
    status: RunnerStatus,
}

impl CommandRunner {
    /// Start a command from a [`LoadedCommand`].
    ///
    /// The child process is spawned with:
    /// - `stdin` piped (for sending responses)
    /// - `stdout` piped (for reading messages)
    /// - `stderr` inherited (so diagnostic output goes to jterm's log)
    /// - Working directory set to the command's directory
    /// - `JTERM_SOCKET` env var pointing to jterm's IPC socket
    pub fn start(cmd: &LoadedCommand) -> Result<Self, std::io::Error> {
        Self::start_with_socket(cmd, &default_socket_path())
    }

    /// Start a command with an explicit socket path.
    ///
    /// This is useful for testing where no real jterm socket exists.
    pub fn start_with_socket(
        cmd: &LoadedCommand,
        socket_path: &str,
    ) -> Result<Self, std::io::Error> {
        let mut child = Command::new(&cmd.run_path)
            .current_dir(&cmd.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("JTERM_SOCKET", socket_path)
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "failed to capture child stdout"))?;

        let (tx, rx) = mpsc::channel();

        // Spawn a background thread to read JSON lines from the child's stdout.
        // This keeps the main event loop non-blocking.
        std::thread::Builder::new()
            .name("command-reader".to_string())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            let trimmed = line.trim().to_string();
                            if trimmed.is_empty() {
                                continue;
                            }
                            match serde_json::from_str::<CommandMessage>(&trimmed) {
                                Ok(msg) => {
                                    if tx.send(ReaderEvent::Message(msg)).is_err() {
                                        break; // receiver dropped
                                    }
                                }
                                Err(e) => {
                                    log::warn!("invalid JSON from command: {e} (line: {trimmed})");
                                    let _ = tx.send(ReaderEvent::ReadError(format!(
                                        "invalid JSON: {e}"
                                    )));
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(ReaderEvent::ReadError(format!("I/O error: {e}")));
                            break;
                        }
                    }
                }
                let _ = tx.send(ReaderEvent::Eof);
            })?;

        Ok(Self {
            child,
            receiver: rx,
            current_message: None,
            status: RunnerStatus::WaitingForMessage,
        })
    }

    /// Poll for the next message from the command (non-blocking).
    ///
    /// Returns `Some` if a new message is available and updates the
    /// internal status accordingly. Returns `None` if no message is
    /// ready yet or the command has already terminated.
    ///
    /// Call this from the main event loop each frame.
    pub fn poll(&mut self) -> Option<&CommandMessage> {
        // Don't poll if we're already in a terminal state.
        match &self.status {
            RunnerStatus::Done(_) | RunnerStatus::Error(_) => return None,
            RunnerStatus::WaitingForInput => return self.current_message.as_ref(),
            RunnerStatus::WaitingForMessage => {}
        }

        match self.receiver.try_recv() {
            Ok(ReaderEvent::Message(msg)) => {
                // Determine the new status based on the message type.
                self.status = match &msg {
                    CommandMessage::Done { notify } => RunnerStatus::Done(notify.clone()),
                    CommandMessage::Error { message } => RunnerStatus::Error(message.clone()),
                    CommandMessage::Info { .. } => RunnerStatus::WaitingForMessage,
                    // All interactive messages require user input.
                    CommandMessage::Fuzzy { .. }
                    | CommandMessage::Multi { .. }
                    | CommandMessage::Confirm { .. }
                    | CommandMessage::Text { .. } => RunnerStatus::WaitingForInput,
                };
                self.current_message = Some(msg);
                self.current_message.as_ref()
            }
            Ok(ReaderEvent::Eof) => {
                // Child exited without sending Done/Error.
                if matches!(self.status, RunnerStatus::WaitingForMessage) {
                    self.status = RunnerStatus::Done(None);
                }
                None
            }
            Ok(ReaderEvent::ReadError(e)) => {
                self.status = RunnerStatus::Error(e);
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                if matches!(self.status, RunnerStatus::WaitingForMessage) {
                    self.status = RunnerStatus::Done(None);
                }
                None
            }
        }
    }

    /// Send a response to the command script via its stdin.
    ///
    /// This should be called after the user completes the interaction
    /// requested by the current [`CommandMessage`].
    pub fn respond(&mut self, response: CommandResponse) -> Result<(), std::io::Error> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin not available"))?;

        let mut json = serde_json::to_string(&response)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        json.push('\n');
        stdin.write_all(json.as_bytes())?;
        stdin.flush()?;

        self.status = RunnerStatus::WaitingForMessage;

        Ok(())
    }

    /// Cancel the running command by killing the child process.
    pub fn cancel(&mut self) {
        let _ = self.child.kill();
        self.status = RunnerStatus::Error("cancelled".to_string());
    }

    /// Get the current status.
    pub fn status(&self) -> &RunnerStatus {
        &self.status
    }

    /// Get the current message being displayed, if any.
    pub fn current_message(&self) -> Option<&CommandMessage> {
        self.current_message.as_ref()
    }
}

impl Drop for CommandRunner {
    fn drop(&mut self) {
        // Ensure the child process is cleaned up.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Get the default jterm IPC socket path.
fn default_socket_path() -> String {
    jterm_session::daemon::socket_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper to create a temporary command that writes specific JSON to stdout.
    fn make_test_command(tmp: &Path, script_body: &str) -> LoadedCommand {
        use crate::command_loader::CommandMeta;

        let dir = tmp.to_path_buf();
        let run_path = dir.join("run.sh");
        fs::write(
            &run_path,
            format!("#!/bin/sh\n{script_body}"),
        )
        .unwrap();

        // Make executable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&run_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        LoadedCommand {
            meta: CommandMeta {
                name: "test".to_string(),
                description: "test command".to_string(),
                icon: String::new(),
                version: String::new(),
                author: String::new(),
                run: "./run.sh".to_string(),
                tags: vec![],
                signature: None,
            },
            dir,
            run_path,
            verify_result: crate::command_signer::VerifyResult::Unsigned,
        }
    }

    use std::path::Path;

    /// Poll the runner until a message is available, with a timeout.
    /// Returns true if a message was received.
    fn poll_until_message(runner: &mut CommandRunner, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        loop {
            if runner.poll().is_some() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn test_runner_done_message() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            r#"echo '{"type":"done","notify":"All done!"}'"#,
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();

        assert!(poll_until_message(&mut runner, 2000), "expected a message");
        match runner.current_message().unwrap() {
            CommandMessage::Done { notify } => {
                assert_eq!(notify.as_deref(), Some("All done!"));
            }
            other => panic!("expected Done, got: {other:?}"),
        }

        assert_eq!(runner.status(), &RunnerStatus::Done(Some("All done!".to_string())));
    }

    #[test]
    fn test_runner_error_message() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            r#"echo '{"type":"error","message":"something broke"}'"#,
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();

        assert!(poll_until_message(&mut runner, 2000), "expected a message");
        assert_eq!(
            runner.status(),
            &RunnerStatus::Error("something broke".to_string())
        );
    }

    #[test]
    fn test_runner_info_then_done() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            r#"echo '{"type":"info","message":"Loading..."}'
echo '{"type":"done"}'"#,
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();

        // First poll: should get the info message.
        assert!(poll_until_message(&mut runner, 2000), "expected info message");
        match runner.current_message().unwrap() {
            CommandMessage::Info { message } => {
                assert_eq!(message, "Loading...");
            }
            other => panic!("expected Info, got: {other:?}"),
        }
        // Info doesn't require input, status should be WaitingForMessage.
        assert_eq!(runner.status(), &RunnerStatus::WaitingForMessage);

        // Second poll: should get the done message.
        assert!(poll_until_message(&mut runner, 2000), "expected done message");
        match runner.current_message().unwrap() {
            CommandMessage::Done { notify } => {
                assert!(notify.is_none());
            }
            other => panic!("expected Done, got: {other:?}"),
        }
        assert_eq!(runner.status(), &RunnerStatus::Done(None));
    }

    #[test]
    fn test_runner_fuzzy_waits_for_input() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            r#"echo '{"type":"fuzzy","prompt":"Pick","items":[{"value":"a"},{"value":"b"}]}'
# Script would normally read stdin here, but we just exit.
read response
echo '{"type":"done"}'
"#,
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();

        assert!(poll_until_message(&mut runner, 2000), "expected fuzzy message");
        assert_eq!(runner.status(), &RunnerStatus::WaitingForInput);

        // Send a response.
        runner
            .respond(CommandResponse::Selected {
                value: "a".to_string(),
            })
            .unwrap();

        assert_eq!(runner.status(), &RunnerStatus::WaitingForMessage);

        // Wait for the script to process the response and send Done.
        assert!(poll_until_message(&mut runner, 2000), "expected done message");
        assert_eq!(runner.status(), &RunnerStatus::Done(None));
    }

    #[test]
    fn test_runner_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            // Long-running script.
            "sleep 60",
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();
        runner.cancel();
        assert_eq!(
            runner.status(),
            &RunnerStatus::Error("cancelled".to_string())
        );
    }

    #[test]
    fn test_runner_empty_script() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(tmp.path(), "# This script produces no output");

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();

        // Poll until the runner reaches a terminal state or times out.
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(2000);
        loop {
            runner.poll();
            match runner.status() {
                RunnerStatus::Done(None) => break,
                RunnerStatus::WaitingForMessage if start.elapsed() < timeout => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                RunnerStatus::WaitingForMessage => break, // timeout, still acceptable
                other => panic!("expected Done or WaitingForMessage, got: {other:?}"),
            }
        }
    }

    #[test]
    fn test_runner_current_message() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = make_test_command(
            tmp.path(),
            r#"echo '{"type":"info","message":"hello"}'"#,
        );

        let mut runner = CommandRunner::start_with_socket(&cmd, "/tmp/jterm-test.sock").unwrap();
        assert!(runner.current_message().is_none());

        assert!(poll_until_message(&mut runner, 2000), "expected a message");

        assert!(runner.current_message().is_some());
        match runner.current_message().unwrap() {
            CommandMessage::Info { message } => assert_eq!(message, "hello"),
            other => panic!("expected Info, got: {other:?}"),
        }
    }
}
