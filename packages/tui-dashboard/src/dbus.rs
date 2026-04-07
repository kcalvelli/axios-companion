//! D-Bus proxy for org.axios.Companion1.
//!
//! Client-side proxy — identical to the one in cli-client.

use zbus::Connection;

#[zbus::proxy(
    interface = "org.axios.Companion1",
    default_service = "org.axios.Companion",
    default_path = "/org/axios/Companion"
)]
pub trait Companion {
    /// Send a message and block until the full response arrives.
    async fn send_message(
        &self,
        surface: &str,
        conversation_id: &str,
        message: &str,
    ) -> zbus::Result<String>;

    /// Send a message and return immediately. Response arrives via signals.
    async fn stream_message(
        &self,
        surface: &str,
        conversation_id: &str,
        message: &str,
    ) -> zbus::Result<()>;

    /// List all sessions: (surface, conversation_id, claude_session_id, status, last_active_at).
    async fn list_sessions(&self) -> zbus::Result<Vec<(String, String, String, String, u32)>>;

    /// Daemon status as a string-keyed property map.
    async fn get_status(
        &self,
    ) -> zbus::Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>>;

    /// Surfaces with at least one active session.
    async fn get_active_surfaces(&self) -> zbus::Result<Vec<String>>;

    #[zbus(signal)]
    fn response_chunk(surface: &str, conversation_id: &str, chunk: &str);

    #[zbus(signal)]
    fn response_complete(surface: &str, conversation_id: &str, full_text: &str);

    #[zbus(signal)]
    fn response_error(surface: &str, conversation_id: &str, error: &str);
}

pub async fn connect() -> Result<CompanionProxy<'static>, zbus::Error> {
    let connection = Connection::session().await?;
    CompanionProxy::new(&connection).await
}
