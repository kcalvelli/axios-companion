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

    // Placeholder: wait for shutdown signal.
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("received shutdown signal, exiting"),
        Err(e) => error!(%e, "failed to listen for shutdown signal"),
    }
}
