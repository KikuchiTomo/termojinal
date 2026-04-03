//! Daemon IPC client.

use crate::{Pane, UserEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use termojinal_layout::PaneId;
use termojinal_vt::Terminal;
use winit::event_loop::EventLoopProxy;

pub(crate) use termojinal_ipc::daemon_connection::DaemonHandle;

// ---------------------------------------------------------------------------
// Per-pane pending-event flags.
//
// When a daemon reader thread pushes data into the buffer it sets the flag
// to `true` via `swap`.  Only the thread that actually flipped the flag from
// `false → true` sends a `UserEvent::PtyOutput` event, preventing the event
// queue from being flooded when a single pane produces output rapidly.
//
// The event handler in main.rs calls `clear_pty_pending` after draining the
// buffer so that the next chunk of output can trigger a new event.
// ---------------------------------------------------------------------------

static PTY_PENDING: Mutex<Option<HashMap<PaneId, Arc<AtomicBool>>>> = Mutex::new(None);

fn get_or_create_pending(id: PaneId) -> Arc<AtomicBool> {
    let mut guard = PTY_PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    map.entry(id).or_insert_with(|| Arc::new(AtomicBool::new(false))).clone()
}

/// Clear the pending-event flag for a pane so the reader thread can send
/// another `PtyOutput` event once new data arrives.
pub(crate) fn clear_pty_pending(id: PaneId) {
    if let Ok(guard) = PTY_PENDING.lock() {
        if let Some(map) = guard.as_ref() {
            if let Some(flag) = map.get(&id) {
                flag.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// Remove the pending-event flag when a pane is destroyed.
pub(crate) fn remove_pty_pending(id: PaneId) {
    if let Ok(mut guard) = PTY_PENDING.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&id);
        }
    }
}






// ---------------------------------------------------------------------------
// User events from PTY reader threads
// ---------------------------------------------------------------------------


pub(crate) fn spawn_pane(
    id: PaneId,
    cols: u16,
    rows: u16,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    cwd: Option<String>,
    time_travel_config: Option<&crate::config::TimeTravelConfig>,
    cjk_width: bool,
) -> Result<Pane, termojinal_pty::PtyError> {
    let daemon = DaemonHandle::new();
    let shell = termojinal_pty::detect_shell();
    let cwd_str = cwd.as_deref().unwrap_or(".");

    // Create session on the daemon.
    let (session_id, _name, shell_pid) = daemon
        .create_session(&shell, cwd_str, cols, rows)
        .ok_or_else(|| {
            termojinal_pty::PtyError::Open(
                "failed to create session on daemon (is termojinald running?)".to_string(),
            )
        })?;

    log::info!(
        "pane {id}: daemon session={}, shell={}, pid={}",
        session_id,
        shell,
        shell_pid
    );

    let mut terminal = Terminal::new(cols as usize, rows as usize);
    terminal.set_cjk_width(cjk_width);
    if let Some(tt) = time_travel_config {
        terminal.set_command_history_enabled(tt.command_history);
        terminal.set_max_command_history(tt.max_command_history);
    }
    let vt_parser = vte::Parser::new();

    // Insert buffer for this pane.
    buffers.lock().unwrap_or_else(|e| e.into_inner()).insert(id, VecDeque::new());

    // Create write channel for sending key input and resize to the daemon reader thread.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<termojinal_ipc::daemon_connection::WriteCommand>();

    // Register the write channel so daemon_pty_write()/daemon_pty_resize() can find it by session_id.
    termojinal_ipc::daemon_connection::register_write_channel(&session_id, write_tx.clone());

    // Spawn daemon reader thread that connects to the daemon via binary
    // framing, attaches to the session, and reads PTY output.
    let proxy_clone = proxy.clone();
    let buffers_clone = buffers.clone();
    let sid = session_id.clone();
    let sock_path = daemon_socket_path();
    let pending_flag = get_or_create_pending(id);
    std::thread::Builder::new()
        .name(format!("daemon-reader-{id}"))
        .spawn(move || {
            use termojinal_ipc::daemon_connection::{daemon_reader_thread, DaemonReaderEvent};
            let sid_for_cleanup = sid.clone();
            daemon_reader_thread(
                id,
                &sid,
                &sock_path,
                move |event| {
                    match event {
                        DaemonReaderEvent::Output(data) => {
                            if let Ok(mut lock) = buffers_clone.lock() {
                                if let Some(q) = lock.get_mut(&id) {
                                    q.push_back(data);
                                }
                            }
                            // Only send an event if one isn't already pending,
                            // preventing event-queue flooding from busy panes.
                            if !pending_flag.swap(true, Ordering::Relaxed) {
                                let _ = proxy_clone.send_event(UserEvent::PtyOutput(id));
                            }
                        }
                        DaemonReaderEvent::Snapshot(data) => {
                            let _ = proxy_clone.send_event(UserEvent::SnapshotReceived(id, data));
                        }
                        DaemonReaderEvent::Exited => {
                            termojinal_ipc::daemon_connection::unregister_write_channel(&sid_for_cleanup);
                            let _ = proxy_clone.send_event(UserEvent::PtyExited(id));
                        }
                    }
                },
                write_rx,
            );
        })
        .expect("failed to spawn daemon-reader thread");

    Ok(Pane {
        id,
        terminal,
        vt_parser,
        session_id,
        shell,
        shell_pid,
        write_tx,
        selection: None,
        preedit: None,
    })
}

/// Attach to an existing daemon session (re-attach after GUI restart).
///
/// Similar to `spawn_pane` but does not create a new session on the daemon.
/// Instead, it connects to an existing session via `AttachSession` and restores
/// the terminal from the snapshot sent by the daemon.
pub(crate) fn attach_existing_session(
    id: PaneId,
    session_id: String,
    shell: String,
    shell_pid: i32,
    cols: u16,
    rows: u16,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    time_travel_config: Option<&crate::config::TimeTravelConfig>,
    cjk_width: bool,
) -> Option<Pane> {
    log::info!(
        "pane {id}: re-attaching to daemon session={}, shell={}, pid={}",
        session_id,
        shell,
        shell_pid
    );

    let mut terminal = Terminal::new(cols as usize, rows as usize);
    terminal.set_cjk_width(cjk_width);
    if let Some(tt) = time_travel_config {
        terminal.set_command_history_enabled(tt.command_history);
        terminal.set_max_command_history(tt.max_command_history);
    }
    let vt_parser = vte::Parser::new();

    // Insert buffer for this pane.
    if let Ok(mut lock) = buffers.lock() {
        lock.insert(id, VecDeque::new());
    }

    // Create write channel for sending key input and resize to the daemon reader thread.
    let (write_tx, write_rx) =
        std::sync::mpsc::channel::<termojinal_ipc::daemon_connection::WriteCommand>();

    // Register the write channel so daemon_pty_write()/daemon_pty_resize() can find it.
    termojinal_ipc::daemon_connection::register_write_channel(&session_id, write_tx.clone());

    // Spawn daemon reader thread that connects and attaches to the existing session.
    let proxy_clone = proxy.clone();
    let buffers_clone = buffers.clone();
    let sid = session_id.clone();
    let sock_path = daemon_socket_path();
    let pending_flag = get_or_create_pending(id);
    if let Err(e) = std::thread::Builder::new()
        .name(format!("daemon-reader-{id}"))
        .spawn(move || {
            use termojinal_ipc::daemon_connection::{daemon_reader_thread, DaemonReaderEvent};
            let sid_for_cleanup = sid.clone();
            daemon_reader_thread(
                id,
                &sid,
                &sock_path,
                move |event| {
                    match event {
                        DaemonReaderEvent::Output(data) => {
                            if let Ok(mut lock) = buffers_clone.lock() {
                                if let Some(q) = lock.get_mut(&id) {
                                    q.push_back(data);
                                }
                            }
                            // Only send an event if one isn't already pending,
                            // preventing event-queue flooding from busy panes.
                            if !pending_flag.swap(true, Ordering::Relaxed) {
                                let _ = proxy_clone.send_event(UserEvent::PtyOutput(id));
                            }
                        }
                        DaemonReaderEvent::Snapshot(data) => {
                            let _ =
                                proxy_clone.send_event(UserEvent::SnapshotReceived(id, data));
                        }
                        DaemonReaderEvent::Exited => {
                            termojinal_ipc::daemon_connection::unregister_write_channel(
                                &sid_for_cleanup,
                            );
                            let _ = proxy_clone.send_event(UserEvent::PtyExited(id));
                        }
                    }
                },
                write_rx,
            );
        })
    {
        log::error!("failed to spawn daemon-reader thread for reattach: {e}");
        termojinal_ipc::daemon_connection::unregister_write_channel(&session_id);
        return None;
    }

    Some(Pane {
        id,
        terminal,
        vt_parser,
        session_id,
        shell,
        shell_pid,
        write_tx,
        selection: None,
        preedit: None,
    })
}

/// Write data to a pane's PTY via the daemon (fire-and-forget, synchronous).
pub(crate) fn daemon_pty_write(session_id: &str, data: &[u8]) {
    termojinal_ipc::daemon_connection::daemon_pty_write(session_id, data);
}

/// Resize a pane's PTY via the daemon (fire-and-forget, synchronous).
pub(crate) fn daemon_pty_resize(session_id: &str, cols: u16, rows: u16) {
    termojinal_ipc::daemon_connection::daemon_pty_resize(session_id, cols, rows);
}

/// Get the Unix socket path for the termojinald daemon.
/// Mirrors `termojinal_session::daemon::socket_path()`.
pub(crate) fn daemon_socket_path() -> String {
    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir
        .join("termojinal")
        .join("termojinald.sock")
        .to_string_lossy()
        .to_string()
}
