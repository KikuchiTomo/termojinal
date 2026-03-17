//! jtermd — the jterm session daemon.

use jterm_session::daemon::Daemon;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    log::info!("jtermd starting...");

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

            // Wait for SIGTERM/SIGINT (Ctrl+C) for graceful shutdown.
            tokio::signal::ctrl_c().await.ok();
            log::info!("shutting down, saving sessions...");

            // Save all session states before exiting.
            let mgr = manager.lock().await;
            if let Err(e) = mgr.save_all() {
                log::error!("failed to save sessions on shutdown: {e}");
            } else {
                log::info!("sessions saved successfully");
            }
            drop(mgr);

            // Abort the daemon loop (it runs forever otherwise).
            daemon_handle.abort();

            log::info!("jtermd stopped");
        }
        Err(e) => {
            log::error!("failed to start daemon: {e}");
            std::process::exit(1);
        }
    }
}
