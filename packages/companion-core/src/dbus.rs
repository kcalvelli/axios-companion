//! D-Bus interface — org.axios.Companion1 on the session bus.
//!
//! Translates D-Bus method calls into dispatcher TurnRequests and maps
//! TurnEvents back to D-Bus replies or signals.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use zbus::object_server::SignalEmitter;
use zbus::{interface, Connection};
use tracing::{info, warn};

use crate::dispatcher::{Dispatcher, TurnEvent, TurnRequest};

/// Shared daemon state accessible from the D-Bus interface.
pub struct CompanionInterface {
    dispatcher: Arc<Dispatcher>,
    start_time: Instant,
    in_flight: Arc<AtomicU32>,
}

impl CompanionInterface {
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        Self {
            dispatcher,
            start_time: Instant::now(),
            in_flight: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[interface(name = "org.axios.Companion1")]
impl CompanionInterface {
    /// Submit a message and block until the full response is available.
    async fn send_message(
        &self,
        surface: &str,
        conversation_id: &str,
        message: &str,
    ) -> zbus::fdo::Result<String> {
        if message.is_empty() {
            return Err(zbus::fdo::Error::InvalidArgs(
                "message must not be empty".into(),
            ));
        }

        let req = TurnRequest {
            surface_id: surface.to_string(),
            conversation_id: conversation_id.to_string(),
            message_text: message.to_string(),
        };

        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let mut rx = self.dispatcher.dispatch(req).await;
        let mut result = String::new();
        let mut error_msg: Option<String> = None;

        while let Some(event) = rx.recv().await {
            match event {
                TurnEvent::TextChunk(_) => {
                    // SendMessage doesn't stream — just wait for Complete.
                }
                TurnEvent::Complete(text) => {
                    result = text;
                    break;
                }
                TurnEvent::Error(e) => {
                    error_msg = Some(e);
                    break;
                }
            }
        }

        self.in_flight.fetch_sub(1, Ordering::Relaxed);

        match error_msg {
            Some(e) => Err(zbus::fdo::Error::Failed(e)),
            None => Ok(result),
        }
    }

    /// Submit a message and return immediately. Response chunks arrive via signals.
    async fn stream_message(
        &self,
        surface: &str,
        conversation_id: &str,
        message: &str,
    ) -> zbus::fdo::Result<()> {
        if message.is_empty() {
            return Err(zbus::fdo::Error::InvalidArgs(
                "message must not be empty".into(),
            ));
        }

        let req = TurnRequest {
            surface_id: surface.to_string(),
            conversation_id: conversation_id.to_string(),
            message_text: message.to_string(),
        };

        let mut rx = self.dispatcher.dispatch(req).await;
        let in_flight = self.in_flight.clone();
        in_flight.fetch_add(1, Ordering::Relaxed);

        // The caller's receiver keeps the turn alive. Signals are emitted
        // by the broadcast subscriber (started in serve()), not here.
        tokio::spawn(async move {
            while let Some(_event) = rx.recv().await {
                // Just drain the channel to keep the turn alive.
                // The broadcast subscriber handles D-Bus signal emission.
            }
            in_flight.fetch_sub(1, Ordering::Relaxed);
        });

        Ok(())
    }

    /// List all sessions in the store.
    async fn list_sessions(&self) -> zbus::fdo::Result<Vec<(String, String, String, String, u32)>> {
        let store = self.dispatcher.store().await;
        let sessions = store.list_sessions().map_err(|e| {
            zbus::fdo::Error::Failed(format!("session store error: {e}"))
        })?;

        Ok(sessions
            .into_iter()
            .map(|s| {
                (
                    s.surface,
                    s.conversation_id,
                    s.claude_session_id.unwrap_or_default(),
                    s.status,
                    s.last_active_at as u32,
                )
            })
            .collect())
    }

    /// Return daemon status information.
    async fn get_status(&self) -> zbus::fdo::Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>> {
        use zbus::zvariant::OwnedValue;

        let store = self.dispatcher.store().await;
        let active_sessions = store
            .list_sessions()
            .map(|s| s.iter().filter(|s| s.status == "active").count() as u32)
            .unwrap_or(0);

        let mut status = std::collections::HashMap::new();
        status.insert(
            "uptime_seconds".into(),
            OwnedValue::from(self.start_time.elapsed().as_secs() as u32),
        );
        status.insert(
            "active_sessions".into(),
            OwnedValue::from(active_sessions),
        );
        status.insert(
            "in_flight_turns".into(),
            OwnedValue::from(self.in_flight.load(Ordering::Relaxed)),
        );
        status.insert(
            "version".into(),
            OwnedValue::try_from(zbus::zvariant::Value::from(env!("CARGO_PKG_VERSION")))
                .expect("string to OwnedValue"),
        );

        Ok(status)
    }

    /// Return list of unique surface IDs that have active sessions.
    async fn get_active_surfaces(&self) -> zbus::fdo::Result<Vec<String>> {
        let store = self.dispatcher.store().await;
        let sessions = store.list_sessions().map_err(|e| {
            zbus::fdo::Error::Failed(format!("session store error: {e}"))
        })?;

        let mut surfaces: Vec<String> = sessions
            .into_iter()
            .filter(|s| s.status == "active")
            .map(|s| s.surface)
            .collect();
        surfaces.sort();
        surfaces.dedup();
        Ok(surfaces)
    }

    // -- Signals --

    #[zbus(signal)]
    async fn response_chunk(
        emitter: &SignalEmitter<'_>,
        surface: &str,
        conversation_id: &str,
        chunk: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn response_complete(
        emitter: &SignalEmitter<'_>,
        surface: &str,
        conversation_id: &str,
        full_text: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn response_error(
        emitter: &SignalEmitter<'_>,
        surface: &str,
        conversation_id: &str,
        error: &str,
    ) -> zbus::Result<()>;
}

/// Start the D-Bus server: acquire the bus name, serve the interface, and
/// spawn a background task that emits D-Bus signals for all turn events
/// across all surfaces (not just D-Bus-originated ones).
pub async fn serve(dispatcher: Arc<Dispatcher>) -> zbus::Result<Connection> {
    let mut broadcast_rx = dispatcher.subscribe();
    let iface = CompanionInterface::new(dispatcher);

    let connection = Connection::session().await?;

    connection
        .object_server()
        .at("/org/axios/Companion", iface)
        .await?;

    connection
        .request_name("org.axios.Companion")
        .await?;

    // Spawn a task that subscribes to the dispatcher's broadcast channel
    // and emits D-Bus signals for every turn event, regardless of surface.
    let signal_conn = connection.clone();
    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(ev) => {
                    let iface_ref = match signal_conn
                        .object_server()
                        .interface::<_, CompanionInterface>("/org/axios/Companion")
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(%e, "failed to get interface ref for signal emission");
                            continue;
                        }
                    };
                    let ctxt = iface_ref.signal_emitter();

                    match ev.event {
                        TurnEvent::TextChunk(chunk) => {
                            let _ = CompanionInterface::response_chunk(
                                &ctxt,
                                &ev.surface,
                                &ev.conversation_id,
                                &chunk,
                            )
                            .await;
                        }
                        TurnEvent::Complete(text) => {
                            let _ = CompanionInterface::response_complete(
                                &ctxt,
                                &ev.surface,
                                &ev.conversation_id,
                                &text,
                            )
                            .await;
                        }
                        TurnEvent::Error(error) => {
                            let _ = CompanionInterface::response_error(
                                &ctxt,
                                &ev.surface,
                                &ev.conversation_id,
                                &error,
                            )
                            .await;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "broadcast subscriber lagged, some signals dropped");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("broadcast channel closed, signal emitter stopping");
                    break;
                }
            }
        }
    });

    info!("D-Bus interface ready on org.axios.Companion");
    Ok(connection)
}
