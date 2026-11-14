//! muzeec-cli-daemon: playback engine + IPC + desktop integration.

mod daemon;
mod mpris;
mod notify;

use std::sync::Arc;

use muzeec_cli_core::config::Config;
use muzeec_cli_core::ipc::IpcServer;
use muzeec_cli_core::paths::Paths;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("muzeec-cli-daemon {VERSION}");
        return Ok(());
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,muzeec_cli_core=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let paths = Paths::resolve();
    paths.ensure_dirs()?;
    let config = Config::load(&paths)?;

    tracing::info!(
        socket = %paths.socket_path().display(),
        db = %paths.db_file().display(),
        "starting muzeec-cli-daemon"
    );

    let daemon = daemon::start(paths.clone(), config.clone(), 512 * 1024 * 1024)?;

    // Persist state and exit cleanly on SIGINT/SIGTERM.
    let shutdown_daemon = daemon.clone();
    ctrlc::set_handler(move || {
        tracing::info!("shutting down");
        shutdown_daemon.shutdown();
        std::process::exit(0);
    })?;

    daemon::spawn_watcher(&daemon);

    if config.integrations.mpris {
        mpris::spawn(daemon.clone());
    }
    if config.integrations.notifications {
        notify::spawn(daemon.clone());
    }

    let socket = paths.socket_path();
    let server = match IpcServer::bind(&socket) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "cannot bind IPC socket");
            return Err(e.into());
        }
    };
    tracing::info!("listening on {}", socket.display());
    let handler: Arc<dyn muzeec_cli_core::ipc::Handler> = daemon.clone();
    server.serve(handler)?;
    Ok(())
}
