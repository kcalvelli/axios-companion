mod dbus;
mod dispatcher;
mod store;

use std::sync::Arc;

use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize tracing. Prefer journald when available (running under systemd),
    // fall back to stderr for interactive use / development.
    let journald = tracing_journald::layer().ok();
    let fallback = if journald.is_none() {
        Some(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .compact(),
        )
    } else {
        None
    };

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(journald)
        .with(fallback)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "companion-core starting"
    );

    // Open the session store.
    let data_dir = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").expect("HOME not set");
        format!("{home}/.local/share")
    });
    let db_path = std::path::PathBuf::from(data_dir)
        .join("axios-companion")
        .join("sessions.db");

    let store = match store::SessionStore::open(&db_path) {
        Ok(s) => {
            info!(path = %db_path.display(), "session store ready");
            s
        }
        Err(e) => {
            error!(%e, path = %db_path.display(), "failed to open session store");
            std::process::exit(1);
        }
    };

    let dispatcher = Arc::new(dispatcher::Dispatcher::new(store));
    info!("dispatcher ready");

    // Acquire D-Bus name and serve the interface.
    let _connection = match dbus::serve(dispatcher).await {
        Ok(c) => c,
        Err(e) => {
            error!(%e, "failed to start D-Bus interface");
            std::process::exit(1);
        }
    };

    // Wait for shutdown signal.
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("received shutdown signal, exiting"),
        Err(e) => error!(%e, "failed to listen for shutdown signal"),
    }
}
