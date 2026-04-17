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
    let companion_dir = std::path::PathBuf::from(&data_dir).join("cairn-companion");
    let db_path = companion_dir.join("sessions.db");
    let workspace_dir = companion_dir.join("workspace");

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

    // 3. Build the Anonymous --settings deny list by querying the live
    //    mcp-gateway tool registry. This is the dispatcher's only
    //    runtime knowledge of the gateway tool set — adding a new MCP
    //    server picks up automatically on the next daemon restart.
    //
    //    Fallback policy: if the gateway is unreachable or returns
    //    something unparseable, REFUSE TO START. Serving anonymous
    //    channel turns with a stale or missing deny list could leak
    //    dangerous tools, and a quiet partial degradation is worse
    //    than a loud restart loop. Systemd will retry per the unit's
    //    Restart=on-failure.
    let anonymous_settings = match dispatcher::build_anonymous_settings_json().await {
        Ok(json) => {
            info!("anonymous deny list built from live gateway");
            json
        }
        Err(e) => {
            error!(
                error = %e,
                "failed to build anonymous deny list — refusing to start"
            );
            std::process::exit(1);
        }
    };

    // 4. Initialize the dispatcher.
    let dispatcher = Arc::new(dispatcher::Dispatcher::new(store, anonymous_settings, workspace_dir));
    info!("dispatcher ready");

    // 5. Acquire the D-Bus well-known name on the session bus.
    let _connection = match dbus::serve(dispatcher.clone()).await {
        Ok(c) => c,
        Err(e) => {
            error!(%e, "failed to start D-Bus interface");
            std::process::exit(1);
        }
    };

    // 5b. Periodic memory index regeneration (catches Syncthing arrivals).
    dbus::spawn_memory_index_task(dispatcher.clone());

    // 6. Start the OpenAI gateway if enabled via environment.
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

    // 6b. Start the Telegram channel adapter if enabled via environment.
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

    // 6c. Start the XMPP channel adapter if enabled via environment.
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

    // 6d. Start the email channel adapter if enabled via environment.
    // Polls IMAP for unseen mail in Sid's own inbox (genxbot@calvelli.us
    // on the production deploy), parses, dispatches, and replies via SMTP.
    // Conversation key is the RFC 5322 thread root Message-ID, so each
    // mail thread gets its own Claude session.
    let email_shutdown = Arc::new(tokio::sync::Notify::new());
    let email_handle = if let Some(config) = channels::email::EmailConfig::from_env() {
        info!(
            address = %config.address,
            imap_host = %config.imap_host,
            imap_port = config.imap_port,
            smtp_host = %config.smtp_host,
            smtp_port = config.smtp_port,
            poll_secs = config.poll_interval.as_secs(),
            allowed_senders = config.allowed_senders.len(),
            "starting email adapter"
        );
        let notify = email_shutdown.clone();
        let email_dispatcher = dispatcher.clone();
        Some(tokio::spawn(async move {
            channels::email::serve(email_dispatcher, config, notify).await;
        }))
    } else {
        info!("email adapter disabled (COMPANION_EMAIL_ENABLE != 1)");
        None
    };

    // 6e. Start the Discord channel adapter if enabled via environment.
    let discord_shutdown = Arc::new(tokio::sync::Notify::new());
    let discord_handle = if let Some(config) = channels::discord::DiscordConfig::from_env() {
        info!(
            allowed_users = config.allowed_user_ids.len(),
            mention_only = config.mention_only,
            stream_mode = ?config.stream_mode,
            "starting Discord adapter"
        );
        let notify = discord_shutdown.clone();
        let discord_dispatcher = dispatcher.clone();
        Some(tokio::spawn(async move {
            channels::discord::serve(discord_dispatcher, config, notify).await;
        }))
    } else {
        info!("Discord adapter disabled (COMPANION_DISCORD_ENABLE != 1)");
        None
    };

    // 7. Signal readiness via sd_notify(READY=1).
    if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
        warn!(%e, "sd_notify READY=1 failed (not running under systemd?)");
    } else {
        info!("signaled readiness to systemd");
    }

    // 8. Enter the event loop — wait for shutdown signals.
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
    email_shutdown.notify_one();
    discord_shutdown.notify_one();

    if let Some(handle) = gateway_handle {
        let _ = handle.await;
    }
    if let Some(handle) = telegram_handle {
        let _ = handle.await;
    }
    if let Some(handle) = xmpp_handle {
        let _ = handle.await;
    }
    if let Some(handle) = email_handle {
        let _ = handle.await;
    }
    if let Some(handle) = discord_handle {
        let _ = handle.await;
    }

    // D-Bus connection drops when _connection goes out of scope.
    // In-flight turns complete naturally via their tokio tasks.
    info!("companion-core stopped");
}
