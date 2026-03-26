//! termojinald — the termojinal session daemon.

use termojinal_session::daemon::Daemon;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("termojinald starting...");

    match Daemon::new() {
        Ok(daemon) => {
            let manager = daemon.manager().clone();

            // Spawn the main daemon loop.
            let daemon_handle = tokio::spawn(async move {
                if let Err(e) = daemon.run().await {
                    log::error!("daemon error: {e}");
                    std::process::exit(1);
                }
            });

            // Wait for SIGINT (Ctrl+C) or SIGTERM for graceful shutdown.
            let shutdown = async {
                let ctrl_c = tokio::signal::ctrl_c();

                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut sigterm =
                        signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
                    tokio::select! {
                        _ = ctrl_c => log::info!("received SIGINT"),
                        _ = sigterm.recv() => log::info!("received SIGTERM"),
                    }
                }

                #[cfg(not(unix))]
                {
                    ctrl_c.await.ok();
                    log::info!("received SIGINT");
                }
            };
            shutdown.await;

            log::info!("shutting down, sending SIGHUP to sessions...");

            // Send SIGHUP to all sessions, save state, then clean up.
            {
                let mut mgr = manager.lock().await;

                // Send SIGHUP to all session shells and wait briefly for them to exit.
                mgr.graceful_shutdown();

                // Save all session states before exiting.
                if let Err(e) = mgr.save_all() {
                    log::error!("failed to save sessions on shutdown: {e}");
                } else {
                    log::info!("sessions saved successfully");
                }
            }

            // Abort the daemon loop (it runs forever otherwise).
            daemon_handle.abort();

            log::info!("termojinald stopped");
        }
        Err(e) => {
            log::error!("failed to start daemon: {e}");
            std::process::exit(1);
        }
    }
}
