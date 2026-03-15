//! `jt` — command-line tool to control the jterm terminal emulator.
//!
//! Communicates with the `jtermd` daemon over a Unix domain socket using
//! the JSON IPC protocol defined in `jterm_ipc::protocol`.

use clap::{Parser, Subcommand};
use jterm_ipc::client::IpcClient;
use jterm_ipc::protocol::IpcRequest;

#[derive(Parser)]
#[command(name = "jt", about = "Control the jterm terminal emulator")]
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

    /// Check if the jtermd daemon is running
    Ping,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();
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
    };

    match client.send(&request).await {
        Ok(response) => {
            if response.success {
                match &cli.command {
                    Commands::Ping => {
                        println!("jtermd is running");
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
