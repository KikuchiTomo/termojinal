//! `jt` — command-line tool to control the termojinal terminal emulator.
//!
//! Communicates with the `termojinald` daemon over a Unix domain socket using
//! the JSON IPC protocol defined in `termojinal_ipc::protocol`.

use clap::{Parser, Subcommand};
use termojinal_ipc::app_protocol::AppIpcRequest;
use termojinal_ipc::client::IpcClient;
use termojinal_ipc::protocol::IpcRequest;

#[derive(Parser)]
#[command(
    name = "tm",
    about = "Control the termojinal terminal emulator",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all active sessions
    List {
        /// Output raw JSON instead of a table
        #[arg(long)]
        json: bool,
    },

    /// Create a new session
    New {
        /// Shell to use (defaults to $SHELL)
        #[arg(long)]
        shell: Option<String>,

        /// Working directory (defaults to current directory)
        #[arg(long)]
        cwd: Option<String>,
    },

    /// Kill a session by ID (or all sessions with --all)
    Kill {
        /// Session ID to kill (not required with --all)
        id: Option<String>,

        /// Kill all sessions
        #[arg(long)]
        all: bool,
    },

    /// Gracefully exit a session (asks for confirmation if a process is running)
    Exit {
        /// Session ID to exit
        id: String,

        /// Force exit without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Resize a session's PTY
    Resize {
        /// Session ID to resize
        id: String,

        /// Number of columns
        cols: u16,

        /// Number of rows
        rows: u16,
    },

    /// Check if the termojinald daemon is running
    Ping,

    /// One-command setup: config, Claude Code hooks, notification channel
    Setup,

    /// Send a notification to termojinal
    Notify {
        /// Notification title
        #[arg(long)]
        title: Option<String>,

        /// Notification body text
        #[arg(long)]
        body: Option<String>,

        /// Notification subtitle
        #[arg(long)]
        subtitle: Option<String>,

        /// Notification type (e.g. "permission_prompt", "idle_prompt")
        #[arg(long)]
        notification_type: Option<String>,
    },

    /// Handle a Claude Code PermissionRequest hook.
    /// Reads hook JSON from stdin, forwards to termojinal, waits for decision,
    /// and outputs hook decision JSON to stdout.
    AllowRequest,

    /// Report Claude Code status to termojinal (called by hooks).
    ///
    /// Usage:
    ///   tm status running         — Claude Code is actively working
    ///   tm status done            — Claude Code task completed
    ///   tm status subagent-start <agent_id> [--type <type>] [--description <desc>]
    ///   tm status subagent-done <agent_id>
    Status {
        /// State: "running", "done", "subagent-start", "subagent-done"
        state: String,

        /// Agent ID (for subagent-start / subagent-done)
        agent_id: Option<String>,

        /// Subagent type (e.g. "task", "search")
        #[arg(long = "type")]
        agent_type: Option<String>,

        /// Subagent description
        #[arg(long)]
        description: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();

    // Setup runs locally, no daemon needed.
    if matches!(&cli.command, Commands::Setup) {
        run_setup();
        return;
    }

    // The `notify` subcommand connects directly to the app socket, not the daemon.
    if let Commands::Notify {
        title,
        body,
        subtitle,
        notification_type,
    } = &cli.command
    {
        send_notify(
            title.clone(),
            body.clone(),
            subtitle.clone(),
            notification_type.clone(),
        );
        return;
    }

    // The `allow-request` subcommand reads hook stdin, forwards to app, waits for decision.
    if matches!(&cli.command, Commands::AllowRequest) {
        handle_allow_request();
        return;
    }

    // The `exit` subcommand needs special handling for confirmation flow.
    if let Commands::Exit { id, force } = &cli.command {
        handle_exit(id.clone(), *force);
        return;
    }

    // The `status` subcommand sends a Claude Code status update to the daemon.
    if let Commands::Status {
        state,
        agent_id,
        agent_type,
        description,
    } = &cli.command
    {
        send_status(
            state.clone(),
            agent_id.clone(),
            agent_type.clone(),
            description.clone(),
        )
        .await;
        return;
    }

    let client = IpcClient::default_path();

    let request = match &cli.command {
        Commands::Ping => IpcRequest::Ping,
        Commands::List { .. } => IpcRequest::ListSessionDetails,
        Commands::New { shell, cwd } => IpcRequest::CreateSession {
            shell: shell.clone(),
            cwd: cwd.clone(),
        },
        Commands::Kill { id, all } => {
            if *all {
                IpcRequest::KillAll
            } else if let Some(id) = id {
                IpcRequest::KillSession { id: id.clone() }
            } else {
                eprintln!("Error: session ID required (or use --all)");
                std::process::exit(1);
            }
        }
        Commands::Exit { id, force: _ } => IpcRequest::ExitSession { id: id.clone() },
        Commands::Resize { id, cols, rows } => IpcRequest::ResizeSession {
            id: id.clone(),
            cols: *cols,
            rows: *rows,
        },
        Commands::Notify { .. }
        | Commands::Setup
        | Commands::AllowRequest
        | Commands::Status { .. } => {
            unreachable!("handled above")
        }
    };

    let response = client.send(&request).await;

    // If ListSessionDetails failed (e.g. older daemon), fall back to ListSessions.
    let response = match (&cli.command, &response) {
        (Commands::List { .. }, Ok(r)) if !r.success => {
            let r2 = r.error.as_deref().unwrap_or("");
            if r2.contains("unknown request type") {
                client.send(&IpcRequest::ListSessions).await
            } else {
                response
            }
        }
        _ => response,
    };

    match response {
        Ok(response) => {
            if response.success {
                match &cli.command {
                    Commands::Ping => {
                        println!("termojinald is running");
                    }
                    Commands::List { json } => {
                        if let Some(data) = &response.data {
                            if let Some(sessions) = data.get("sessions") {
                                if let Some(arr) = sessions.as_array() {
                                    if *json {
                                        println!(
                                            "{}",
                                            serde_json::to_string_pretty(sessions)
                                                .unwrap_or_default()
                                        );
                                    } else if arr.is_empty() {
                                        println!("No active sessions.");
                                    } else if arr.first().and_then(|v| v.as_str()).is_some() {
                                        // Old format: array of ID strings
                                        for s in arr {
                                            if let Some(id) = s.as_str() {
                                                println!("{id}");
                                            }
                                        }
                                    } else {
                                        print_session_table(arr);
                                    }
                                }
                            }
                        }
                    }
                    Commands::New { .. } => {
                        if let Some(data) = &response.data {
                            let id = data.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let name = data
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            println!("Created session: {name} ({id})");
                        }
                    }
                    Commands::Kill { id, all } => {
                        if *all {
                            if let Some(data) = &response.data {
                                let count =
                                    data.get("killed").and_then(|v| v.as_u64()).unwrap_or(0);
                                println!("Killed {count} session(s).");
                            } else {
                                println!("All sessions killed.");
                            }
                        } else if let Some(id) = id {
                            println!("Killed session: {id}");
                        }
                    }
                    Commands::Resize { id, cols, rows } => {
                        println!("Resized session {id} to {cols}x{rows}");
                    }
                    Commands::Setup
                    | Commands::AllowRequest
                    | Commands::Notify { .. }
                    | Commands::Exit { .. }
                    | Commands::Status { .. } => {
                        unreachable!("handled above")
                    }
                }
            } else {
                let msg = response.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// Send a Claude Code status update to the daemon.
///
/// Reads `CLAUDE_SESSION_ID` from environment and uses PPID to identify
/// which PTY pane this hook was invoked from.
async fn send_status(
    state: String,
    agent_id: Option<String>,
    agent_type: Option<String>,
    description: Option<String>,
) {
    let session_id = std::env::var("CLAUDE_SESSION_ID").ok();
    // Use PPID to trace back to the PTY shell process.
    let ppid = unsafe { libc::getppid() };

    // Normalize subagent states.
    let (final_state, final_agent_id) = match state.as_str() {
        "subagent-start" => {
            if agent_id.is_none() {
                eprintln!("Error: subagent-start requires an agent_id argument");
                std::process::exit(1);
            }
            ("running".to_string(), agent_id)
        }
        "subagent-done" => {
            if agent_id.is_none() {
                eprintln!("Error: subagent-done requires an agent_id argument");
                std::process::exit(1);
            }
            ("done".to_string(), agent_id)
        }
        "running" | "done" => (state, None),
        other => {
            eprintln!("Error: unknown state '{other}'. Expected: running, done, subagent-start, subagent-done");
            std::process::exit(1);
        }
    };

    let request = IpcRequest::ClaudeStatusUpdate {
        session_id,
        state: final_state,
        agent_id: final_agent_id,
        agent_type,
        description,
        pid: Some(ppid),
    };

    let client = IpcClient::default_path();
    match client.send(&request).await {
        Ok(resp) => {
            if !resp.success {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Warning: status update failed: {msg}");
            }
        }
        Err(_) => {
            // Daemon not running — silently ignore.
            // This is expected when hooks are configured but termojinal isn't running.
        }
    }
}

/// Shorten a path by replacing the home directory prefix with `~`.
fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path == home_str.as_ref() {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            if rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    path.to_string()
}

/// Format a duration as a human-readable relative time string.
fn format_relative_time(created_at: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return created_at.to_string();
    };
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(dt);

    let secs = duration.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }

    let mins = duration.num_minutes();
    let hours = duration.num_hours();
    let days = duration.num_days();

    if secs < 60 {
        "just now".to_string()
    } else if mins == 1 {
        "1 min ago".to_string()
    } else if mins < 60 {
        format!("{mins} mins ago")
    } else if hours == 1 {
        "1 hour ago".to_string()
    } else if hours < 24 {
        format!("{hours} hours ago")
    } else if days == 1 {
        "1 day ago".to_string()
    } else {
        format!("{days} days ago")
    }
}

/// Truncate a string to at most `max_len` characters, appending `..` if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 2 {
        s[..max_len].to_string()
    } else {
        format!("{}..", &s[..max_len - 2])
    }
}

/// Print a formatted table of session details.
fn print_session_table(sessions: &[serde_json::Value]) {
    // Extract row data.
    struct Row {
        id_short: String,
        name: String,
        shell: String,
        cwd: String,
        pid: String,
        size: String,
        created: String,
    }

    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| {
            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let id_short = if id.len() > 8 { &id[..8] } else { id };
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let shell_path = s.get("shell").and_then(|v| v.as_str()).unwrap_or("");
            // Show only the shell binary name (e.g. "zsh" instead of "/bin/zsh").
            let shell_name = shell_path.rsplit('/').next().unwrap_or(shell_path);
            let cwd = s.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            let pid = s
                .get("pid")
                .map(|v| {
                    if v.is_null() {
                        "-".to_string()
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            let cols = s.get("cols").and_then(|v| v.as_u64()).unwrap_or(0);
            let rows_val = s.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
            let size = format!("{cols}x{rows_val}");
            let created_at = s.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
            let created = format_relative_time(created_at);

            Row {
                id_short: id_short.to_string(),
                name: name.to_string(),
                shell: shell_name.to_string(),
                cwd: shorten_path(cwd),
                pid,
                size,
                created,
            }
        })
        .collect();

    // Compute column widths (header label is the minimum).
    let w_id = rows
        .iter()
        .map(|r| r.id_short.len())
        .max()
        .unwrap_or(0)
        .max(2);
    let w_name = rows.iter().map(|r| r.name.len()).max().unwrap_or(0).max(4);
    let w_shell = rows.iter().map(|r| r.shell.len()).max().unwrap_or(0).max(5);
    let w_cwd = rows
        .iter()
        .map(|r| r.cwd.len())
        .max()
        .unwrap_or(0)
        .max(3)
        .min(40); // cap CWD column width
    let w_pid = rows.iter().map(|r| r.pid.len()).max().unwrap_or(0).max(3);
    let w_size = rows.iter().map(|r| r.size.len()).max().unwrap_or(0).max(4);
    let w_created = rows
        .iter()
        .map(|r| r.created.len())
        .max()
        .unwrap_or(0)
        .max(7);

    // Print header.
    println!(
        "{:<w_id$}  {:<w_name$}  {:<w_shell$}  {:<w_cwd$}  {:>w_pid$}  {:<w_size$}  {:<w_created$}",
        "ID", "NAME", "SHELL", "CWD", "PID", "SIZE", "CREATED",
    );

    // Print rows.
    for r in &rows {
        let cwd_display = truncate(&r.cwd, w_cwd);
        println!(
            "{:<w_id$}  {:<w_name$}  {:<w_shell$}  {:<w_cwd$}  {:>w_pid$}  {:<w_size$}  {:<w_created$}",
            r.id_short, r.name, r.shell, cwd_display, r.pid, r.size, r.created,
        );
    }

    // Summary.
    let count = rows.len();
    let label = if count == 1 { "session" } else { "sessions" };
    println!("\n{count} {label}");
}

/// Send a notification to the termojinal app via its IPC socket.
///
/// Connects directly to the app socket (`termojinal-app.sock`) using
/// synchronous I/O for minimal latency; fire-and-forget.
fn send_notify(
    title: Option<String>,
    body: Option<String>,
    subtitle: Option<String>,
    notification_type: Option<String>,
) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let sock_path = data_dir.join("termojinal").join("termojinal-app.sock");

    let request = AppIpcRequest::Notify {
        title,
        body,
        subtitle,
        notification_type,
    };

    let mut json = match serde_json::to_string(&request) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error serializing request: {e}");
            std::process::exit(1);
        }
    };
    json.push('\n');

    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Error connecting to termojinal app socket at {}: {e}",
                sock_path.display()
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = stream.write_all(json.as_bytes()) {
        eprintln!("Error writing to app socket: {e}");
        std::process::exit(1);
    }

    // Try to read the response (best-effort, with short timeout).
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let reader = BufReader::new(&stream);
    for line in reader.lines() {
        match line {
            Ok(resp) => {
                let resp = resp.trim().to_string();
                if !resp.is_empty() {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&resp) {
                        if parsed.get("success").and_then(|v| v.as_bool()) == Some(true) {
                            println!("Notification sent.");
                        } else {
                            let msg = parsed
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown error");
                            eprintln!("Error: {msg}");
                            std::process::exit(1);
                        }
                    }
                }
                break;
            }
            Err(_) => break,
        }
    }
}

/// Handle a Claude Code PermissionRequest hook.
///
/// 1. Reads Claude Code hook JSON from stdin.
/// 2. Extracts tool_name + tool_input.
/// 3. Sends `PermissionRequest` to the termojinal app socket.
/// 4. Blocks until the app responds with the user's decision.
/// 5. Outputs the Claude Code hook decision JSON to stdout.
fn handle_allow_request() {
    use std::io::{BufRead, BufReader, Read as _, Write};
    use std::os::unix::net::UnixStream;

    // Read hook input from stdin.
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let hook_input: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing hook input: {e}");
            std::process::exit(1);
        }
    };

    let tool_name = hook_input
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Tools that are not permission requests — auto-allow them so the user can
    // interact with them directly in the terminal (e.g. answering a question).
    const PASSTHROUGH_TOOLS: &[&str] = &["AskUserQuestion", "AskFollowupQuestion"];
    if PASSTHROUGH_TOOLS.contains(&tool_name.as_str()) {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "decision": {
                    "behavior": "allow"
                }
            }
        });
        println!("{}", output);
        return;
    }

    let tool_input = hook_input
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let session_id = hook_input
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Connect to the app socket.
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let sock_path = data_dir.join("termojinal").join("termojinal-app.sock");

    let request = AppIpcRequest::PermissionRequest {
        tool_name,
        tool_input,
        session_id,
    };

    let mut json = serde_json::to_string(&request).expect("serialize");
    json.push('\n');

    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(e) => {
            // App not running — let Claude Code show its own prompt.
            eprintln!("termojinal not running ({e}), falling through");
            std::process::exit(0);
        }
    };

    // Long timeout: wait for user decision (up to 10 minutes, matching hook timeout).
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(600)));

    if let Err(e) = stream.write_all(json.as_bytes()) {
        eprintln!("Error writing to app socket: {e}");
        std::process::exit(0);
    }

    // Block until the app sends the decision response.
    let reader = BufReader::new(&stream);
    for line in reader.lines() {
        match line {
            Ok(resp) => {
                let resp = resp.trim().to_string();
                if resp.is_empty() {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&resp) {
                    let decision = parsed
                        .get("data")
                        .and_then(|d| d.get("decision"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("ask");

                    // Output the Claude Code hook decision JSON.
                    let output = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PermissionRequest",
                            "decision": {
                                "behavior": decision
                            }
                        }
                    });
                    println!("{}", serde_json::to_string(&output).unwrap());
                }
                break;
            }
            Err(_) => break,
        }
    }
}

/// Handle the `exit` subcommand with interactive confirmation.
fn handle_exit(id: String, force: bool) {
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream;

    let sock_path = termojinal_session::daemon::socket_path();
    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: cannot connect to daemon: {e}");
            std::process::exit(1);
        }
    };
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    // First attempt: try to exit gracefully (or with force).
    let req = serde_json::json!({
        "type": "exit_session",
        "id": id,
        "force": force,
    });
    let msg = format!(
        "{}
",
        req
    );
    if stream.write_all(msg.as_bytes()).is_err() {
        eprintln!("Error: failed to send request to daemon");
        std::process::exit(1);
    }

    let mut line = String::new();
    if std::io::BufReader::new(&stream)
        .read_line(&mut line)
        .is_err()
    {
        eprintln!("Error: failed to read response from daemon");
        std::process::exit(1);
    }

    let resp: serde_json::Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: invalid response: {e}");
            std::process::exit(1);
        }
    };

    if resp.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        eprintln!("Error: {err}");
        std::process::exit(1);
    }

    // Check if a running process was detected.
    if let Some(proc_name) = resp
        .get("data")
        .and_then(|d| d.get("running_process"))
        .and_then(|v| v.as_str())
    {
        // Interactive confirmation.
        eprint!(
            "Process '{}' is running in this session. Exit anyway? [y/N] ",
            proc_name
        );
        std::io::stderr().flush().ok();

        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            eprintln!("Error reading input");
            std::process::exit(1);
        }
        let answer = answer.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Cancelled.");
            return;
        }

        // Force exit.
        let mut stream2 = match UnixStream::connect(&sock_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: cannot connect to daemon: {e}");
                std::process::exit(1);
            }
        };
        stream2
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();
        let req2 = serde_json::json!({
            "type": "exit_session",
            "id": id,
            "force": true,
        });
        let msg2 = format!(
            "{}
",
            req2
        );
        if stream2.write_all(msg2.as_bytes()).is_err() {
            eprintln!("Error: failed to send force-exit request");
            std::process::exit(1);
        }
        let mut line2 = String::new();
        let _ = std::io::BufReader::new(&stream2).read_line(&mut line2);
        println!("Session {} exited.", id);
    } else {
        println!("Session {} exited.", id);
    }
}

/// One-command setup: creates config dir, installs hooks, sets notification channel.
fn run_setup() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let home = dirs::home_dir().expect("cannot determine home directory");

    println!("==> termojinal setup");
    println!();

    // 1. Create config directory
    let config_dir = home.join(".config").join("termojinal");
    fs::create_dir_all(&config_dir).ok();
    fs::create_dir_all(config_dir.join("commands")).ok();
    println!("[ok] config directory: {}", config_dir.display());

    // 2. Install bundled commands (symlink from cwd if available)
    let commands_source = ["commands", "../commands", "../../commands"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_dir());
    if let Some(src) = commands_source {
        let dest = config_dir.join("commands");
        for entry in fs::read_dir(&src).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let target = dest.join(&name);
            if !target.exists() && entry.path().is_dir() {
                #[cfg(unix)]
                {
                    let abs = fs::canonicalize(entry.path()).unwrap_or(entry.path());
                    std::os::unix::fs::symlink(&abs, &target).ok();
                }
            }
        }
        println!("[ok] bundled commands linked");
    }

    // 3. Set Claude Code notification channel
    let has_claude = Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();

    if has_claude {
        let status = Command::new("claude")
            .args([
                "config",
                "set",
                "--global",
                "preferredNotifChannel",
                "iterm2",
            ])
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("[ok] Claude Code: preferredNotifChannel = iterm2");
            }
            _ => {
                println!("[!!] failed to set notification channel");
                println!(
                    "     run manually: claude config set --global preferredNotifChannel iterm2"
                );
            }
        }
    } else {
        println!("[--] Claude Code not found, skipping notification channel");
    }

    // 4. Install Claude Code hook scripts
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir).ok();

    // 4a. Notification hook
    let notify_hook_dest = hooks_dir.join("termojinal-notify.sh");
    if !notify_hook_dest.exists() {
        let hook_script = "#!/usr/bin/env bash\n\
# termojinal notification hook for Claude Code\n\
input=$(cat)\n\
hook_event=$(echo \"$input\" | jq -r '.hook_event_name // empty' 2>/dev/null)\n\
message=$(echo \"$input\" | jq -r '.message // empty' 2>/dev/null)\n\
title=$(echo \"$input\" | jq -r '.title // \"Claude Code\"' 2>/dev/null)\n\
notif_type=$(echo \"$input\" | jq -r '.notification_type // empty' 2>/dev/null)\n\
[ \"$hook_event\" != \"Notification\" ] && exit 0\n\
exec tm notify --title \"$title\" --body \"$message\" ${notif_type:+--notification-type \"$notif_type\"}\n";
        fs::write(&notify_hook_dest, hook_script).ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&notify_hook_dest, fs::Permissions::from_mode(0o755)).ok();
        }
        println!("[ok] hook installed: {}", notify_hook_dest.display());
    } else {
        println!("[ok] hook already exists: {}", notify_hook_dest.display());
    }

    // 4b. PermissionRequest hook (Allow Flow)
    let permission_hook_dest = hooks_dir.join("termojinal-permission.sh");
    if !permission_hook_dest.exists() {
        let hook_script = "#!/usr/bin/env bash\n\
# termojinal Allow Flow hook for Claude Code PermissionRequest events.\n\
# Pipes hook input to `tm allow-request`, which blocks until the user decides.\n\
exec tm allow-request\n";
        fs::write(&permission_hook_dest, hook_script).ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&permission_hook_dest, fs::Permissions::from_mode(0o755)).ok();
        }
        println!("[ok] hook installed: {}", permission_hook_dest.display());
    } else {
        println!(
            "[ok] hook already exists: {}",
            permission_hook_dest.display()
        );
    }

    // 5. Register hooks in Claude Code settings.json
    let claude_settings = home.join(".claude").join("settings.json");
    let settings_content = if claude_settings.exists() {
        fs::read_to_string(&claude_settings).unwrap_or_default()
    } else {
        String::new()
    };
    let needs_notify_hook = !settings_content.contains("termojinal-notify.sh");
    let needs_permission_hook = !settings_content.contains("termojinal-permission.sh");
    let needs_status_hooks = !settings_content.contains("tm status");

    if needs_notify_hook || needs_permission_hook || needs_status_hooks {
        let mut settings: serde_json::Value = if !settings_content.is_empty() {
            serde_json::from_str(&settings_content).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let hooks = settings
            .as_object_mut()
            .unwrap()
            .entry("hooks".to_string())
            .or_insert(serde_json::json!({}));

        if needs_notify_hook {
            let hook_entry = serde_json::json!({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": notify_hook_dest.to_string_lossy()
                    }
                ]
            });
            let notif_hooks = hooks
                .as_object_mut()
                .unwrap()
                .entry("Notification".to_string())
                .or_insert(serde_json::json!([]));
            if let Some(arr) = notif_hooks.as_array_mut() {
                arr.push(hook_entry);
            }
        }

        if needs_permission_hook {
            let hook_entry = serde_json::json!({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": permission_hook_dest.to_string_lossy()
                    }
                ]
            });
            let perm_hooks = hooks
                .as_object_mut()
                .unwrap()
                .entry("PermissionRequest".to_string())
                .or_insert(serde_json::json!([]));
            if let Some(arr) = perm_hooks.as_array_mut() {
                arr.push(hook_entry);
            }
        }

        // 5b. Register status hooks for event-driven state detection.
        if needs_status_hooks {
            // PreToolUse -> "running" (Claude is about to use a tool)
            let pre_tool_entry = serde_json::json!({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": "tm status running"
                    }
                ]
            });
            let pre_hooks = hooks
                .as_object_mut()
                .unwrap()
                .entry("PreToolUse".to_string())
                .or_insert(serde_json::json!([]));
            if let Some(arr) = pre_hooks.as_array_mut() {
                arr.push(pre_tool_entry);
            }

            // PostToolUse -> "running" (Claude just finished a tool, still working)
            let post_tool_entry = serde_json::json!({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": "tm status running"
                    }
                ]
            });
            let post_hooks = hooks
                .as_object_mut()
                .unwrap()
                .entry("PostToolUse".to_string())
                .or_insert(serde_json::json!([]));
            if let Some(arr) = post_hooks.as_array_mut() {
                arr.push(post_tool_entry);
            }

            // Stop -> "done" (Claude finished the task)
            let stop_entry = serde_json::json!({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": "tm status done"
                    }
                ]
            });
            let stop_hooks = hooks
                .as_object_mut()
                .unwrap()
                .entry("Stop".to_string())
                .or_insert(serde_json::json!([]));
            if let Some(arr) = stop_hooks.as_array_mut() {
                arr.push(stop_entry);
            }

            println!("[ok] status hooks registered (PreToolUse, PostToolUse, Stop)");
        }

        fs::write(
            &claude_settings,
            serde_json::to_string_pretty(&settings).unwrap(),
        )
        .ok();
        println!("[ok] hooks registered in {}", claude_settings.display());
    } else {
        println!("[ok] hooks already in settings.json");
    }

    println!();
    println!("==> setup complete!");
    println!();
    println!("  Run:      termojinal   (or: make run-dev)");
    println!("  Daemon:   termojinald  (or: make run-daemon)");
    println!("  Hotkey:   Cmd+`        (requires daemon + Input Monitoring)");
}
