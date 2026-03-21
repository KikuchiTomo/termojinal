//! `jt` — command-line tool to control the termojinal terminal emulator.
//!
//! Communicates with the `termojinald` daemon over a Unix domain socket using
//! the JSON IPC protocol defined in `termojinal_ipc::protocol`.

use clap::{Parser, Subcommand};
use termojinal_ipc::app_protocol::AppIpcRequest;
use termojinal_ipc::client::IpcClient;
use termojinal_ipc::protocol::IpcRequest;

#[derive(Parser)]
#[command(name = "tm", about = "Control the termojinal terminal emulator", version)]
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

    /// Kill a session by ID
    Kill {
        /// Session ID to kill
        id: String,
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

    let client = IpcClient::default_path();

    let request = match &cli.command {
        Commands::Ping => IpcRequest::Ping,
        Commands::List { .. } => IpcRequest::ListSessionDetails,
        Commands::New { shell, cwd } => IpcRequest::CreateSession {
            shell: shell.clone(),
            cwd: cwd.clone(),
        },
        Commands::Kill { id } => IpcRequest::KillSession { id: id.clone() },
        Commands::Resize { id, cols, rows } => IpcRequest::ResizeSession {
            id: id.clone(),
            cols: *cols,
            rows: *rows,
        },
        Commands::Notify { .. } | Commands::Setup | Commands::AllowRequest => {
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
                                        println!("{}", serde_json::to_string_pretty(sessions).unwrap_or_default());
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
                            let id = data
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let name = data
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            println!("Created session: {name} ({id})");
                        }
                    }
                    Commands::Kill { id } => {
                        println!("Killed session: {id}");
                    }
                    Commands::Resize { id, cols, rows } => {
                        println!("Resized session {id} to {cols}x{rows}");
                    }
                    Commands::Setup | Commands::AllowRequest | Commands::Notify { .. } => {
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
    let w_id = rows.iter().map(|r| r.id_short.len()).max().unwrap_or(0).max(2);
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
    let sock_path = data_dir
        .join("termojinal")
        .join("termojinal-app.sock");

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
    let sock_path = data_dir
        .join("termojinal")
        .join("termojinal-app.sock");

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
            .args(["config", "set", "--global", "preferredNotifChannel", "iterm2"])
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("[ok] Claude Code: preferredNotifChannel = iterm2");
            }
            _ => {
                println!("[!!] failed to set notification channel");
                println!("     run manually: claude config set --global preferredNotifChannel iterm2");
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
        let hook_script = r#"#!/usr/bin/env bash
# termojinal notification hook for Claude Code
input=$(cat)
hook_event=$(echo "$input" | jq -r '.hook_event_name // empty' 2>/dev/null)
message=$(echo "$input" | jq -r '.message // empty' 2>/dev/null)
title=$(echo "$input" | jq -r '.title // "Claude Code"' 2>/dev/null)
notif_type=$(echo "$input" | jq -r '.notification_type // empty' 2>/dev/null)
[ "$hook_event" != "Notification" ] && exit 0
exec tm notify --title "$title" --body "$message" ${notif_type:+--notification-type "$notif_type"}
"#;
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
        let hook_script = r#"#!/usr/bin/env bash
# termojinal Allow Flow hook for Claude Code PermissionRequest events.
# Pipes hook input to `tm allow-request`, which blocks until the user decides.
exec tm allow-request
"#;
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

    if needs_notify_hook || needs_permission_hook {
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
    println!("  Hotkey:   Ctrl+`       (requires daemon + Accessibility)");
}
