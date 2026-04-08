mod channels;
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

    // 5b. Start the Telegram channel adapter if enabled via environment.
    let telegram_shutdown = Arc::new(tokio::sync::Notify::new());
    let telegram_handle = if let Some(config) = channels::telegram::TelegramConfig::from_env() {
        info!(
            allowed_users = config.allowed_users.len(),
            mention_only = config.mention_only,
            stream_mode = ?config.stream_mode,
            "starting Telegram adapter"
        );
        let notify = telegram_shutdown.clone();
        let tg_dispatcher = dispatcher.clone();
        Some(tokio::spawn(async move {
            channels::telegram::serve(tg_dispatcher, config, notify).await;
        }))
    } else {
        info!("Telegram adapter disabled (COMPANION_TELEGRAM_ENABLE != 1)");
        None
    };

    // 5c. Start the XMPP channel adapter if enabled via environment.
    // Phase 2 lands the connect/auth/presence/reconnect path; the dispatcher
    // wiring (DM + MUC handling) lands in Phase 3+ of channel-xmpp.
    let xmpp_shutdown = Arc::new(tokio::sync::Notify::new());
    let xmpp_handle = if let Some(config) = channels::xmpp::XmppConfig::from_env() {
        info!(
            jid = %config.jid,
            server = %config.server,
            port = config.port,
            allowed_jids = config.allowed_jids.len(),
            muc_rooms = config.muc_rooms.len(),
            mention_only = config.mention_only,
            stream_mode = ?config.stream_mode,
            "starting XMPP adapter"
        );
        let notify = xmpp_shutdown.clone();
        let xmpp_dispatcher = dispatcher.clone();
        Some(tokio::spawn(async move {
            channels::xmpp::serve(xmpp_dispatcher, config, notify).await;
        }))
    } else {
        info!("XMPP adapter disabled (COMPANION_XMPP_ENABLE != 1)");
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

    // Graceful shutdown: signal adapters to stop, then wait for them.
    shutdown_notify.notify_one();
    telegram_shutdown.notify_one();
    xmpp_shutdown.notify_one();

    if let Some(handle) = gateway_handle {
        let _ = handle.await;
    }
    if let Some(handle) = telegram_handle {
        let _ = handle.await;
    }
    if let Some(handle) = xmpp_handle {
        let _ = handle.await;
    }

    // D-Bus connection drops when _connection goes out of scope.
    // In-flight turns complete naturally via their tokio tasks.
    info!("companion-core stopped");
}
