mod dbus;
mod dispatcher;
mod gateway;
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
    let _connection = match dbus::serve(dispatcher.clone()).await {
        Ok(c) => c,
        Err(e) => {
            error!(%e, "failed to start D-Bus interface");
            std::process::exit(1);
        }
    };

    // 5. Start the OpenAI gateway if enabled via environment.
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let gateway_handle = if let Some(config) = gateway::types::GatewayConfig::from_env() {
        info!(
            port = config.port,
            bind = %config.bind_address,
            model = %config.model_name,
            policy = ?config.session_policy,
            "starting OpenAI gateway"
        );
        let notify = shutdown_notify.clone();
        let gw_dispatcher = dispatcher.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = gateway::serve(
                gw_dispatcher,
                config,
                async move { notify.notified().await },
            )
            .await
            {
                error!("OpenAI gateway failed: {e}");
            }
        }))
    } else {
        info!("OpenAI gateway disabled (COMPANION_GATEWAY_ENABLE != 1)");
        None
    };

    // 6. Signal readiness via sd_notify(READY=1).
    if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
        warn!(%e, "sd_notify READY=1 failed (not running under systemd?)");
    } else {
        info!("signaled readiness to systemd");
    }

    // 7. Enter the event loop — wait for shutdown signals.
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

    // Graceful shutdown: signal the gateway to stop, then wait for it.
    shutdown_notify.notify_one();
    if let Some(handle) = gateway_handle {
        let _ = handle.await;
    }

    // D-Bus connection drops when _connection goes out of scope.
    // In-flight turns complete naturally via their tokio tasks.
    info!("companion-core stopped");
}
