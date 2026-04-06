mod dbus;
mod dispatcher;
mod store;

use std::sync::Arc;

use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    // 1. Initialize structured logging via tracing to the systemd journal.
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

    // 2. Open (or create) the SQLite session store and run pending migrations.
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

    // 3. Initialize the dispatcher.
    let dispatcher = Arc::new(dispatcher::Dispatcher::new(store));
    info!("dispatcher ready");

    // 4. Acquire the D-Bus well-known name on the session bus.
    let _connection = match dbus::serve(dispatcher).await {
        Ok(c) => c,
        Err(e) => {
            error!(%e, "failed to start D-Bus interface");
            std::process::exit(1);
        }
    };

    // 5. Signal readiness via sd_notify(READY=1).
    if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
        // Not fatal — we might not be running under systemd.
        warn!(%e, "sd_notify READY=1 failed (not running under systemd?)");
    } else {
        info!("signaled readiness to systemd");
    }

    // 6. Enter the event loop — wait for shutdown signals.
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sighup = signal(SignalKind::hangup()).expect("failed to register SIGHUP handler");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received SIGINT, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM, shutting down");
                break;
            }
            _ = sighup.recv() => {
                info!("SIGHUP received, no reload action defined");
            }
        }
    }

    // Graceful shutdown: the D-Bus connection drops when _connection goes
    // out of scope, which stops accepting new calls. In-flight turns will
    // complete naturally as their tokio tasks finish. The dispatcher's
    // session locks ensure no new turns start on sessions that are draining.
    //
    // TODO(Phase 5.3): Add explicit drain with 120s timeout for in-flight
    // turns once we track active turn handles in the dispatcher.
    info!("companion-core stopped");
}
