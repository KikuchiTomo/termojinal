//! `jt` — command-line tool to control the termojinal terminal emulator.
//!
//! Communicates with the `termojinald` daemon over a Unix domain socket using
//! the JSON IPC protocol defined in `termojinal_ipc::protocol`.

use clap::{Parser, Subcommand};
use termojinal_ipc::app_protocol::AppIpcRequest;
use termojinal_ipc::client::IpcClient;
use termojinal_ipc::protocol::IpcRequest;

#[derive(Parser)]
#[command(name = "tm", about = "Control the termojinal terminal emulator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all active sessions
    List,

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
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();

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

    let client = IpcClient::default_path();

    let request = match &cli.command {
        Commands::Ping => IpcRequest::Ping,
        Commands::List => IpcRequest::ListSessions,
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
        Commands::Notify { .. } => unreachable!("handled above"),
    };

    match client.send(&request).await {
        Ok(response) => {
            if response.success {
                match &cli.command {
                    Commands::Ping => {
                        println!("termojinald is running");
                    }
                    Commands::List => {
                        if let Some(data) = &response.data {
                            if let Some(sessions) = data.get("sessions") {
                                if let Some(arr) = sessions.as_array() {
                                    if arr.is_empty() {
                                        println!("No active sessions.");
                                    } else {
                                        for s in arr {
                                            if let Some(id) = s.as_str() {
                                                println!("{id}");
                                            }
                                        }
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
                    Commands::Notify { .. } => unreachable!("handled above"),
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
