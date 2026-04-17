//! D-Bus interface — org.cairn.Companion1 on the session bus.
//!
//! Translates D-Bus method calls into dispatcher TurnRequests and maps
//! TurnEvents back to D-Bus replies or signals.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use zbus::object_server::SignalEmitter;
use zbus::{interface, Connection};
use tracing::{info, warn};

use crate::dispatcher::{Dispatcher, TrustLevel, TurnEvent, TurnRequest};

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

    /// Resolve the Claude Code project memory directory for the workspace.
    /// Claude Code slugifies the absolute workspace path: replace both `/`
    /// and `.` with `-` (so `/.local/` becomes `--local-`).
    /// The memory lives at `~/.claude/projects/<slug>/memory/`.
    fn memory_dir(&self) -> PathBuf {
        let workspace = self.dispatcher.workspace_dir();
        let slug = workspace.to_string_lossy().replace('/', "-").replace('.', "-");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        PathBuf::from(home)
            .join(".claude")
            .join("projects")
            .join(slug)
            .join("memory")
    }

    /// Ensure the Syncthing .stignore file exists in the memory dir.
    /// Excludes MEMORY.md (regenerated locally) and conflict artifacts.
    pub fn ensure_stignore(&self) {
        let dir = self.memory_dir();
        if !dir.is_dir() {
            return;
        }
        let stignore = dir.join(".stignore");
        if !stignore.exists() {
            let content = "// Managed by companion-core — do not edit.\n\
                           // MEMORY.md is regenerated locally from frontmatter\n\
                           // to avoid Syncthing conflicts across machines.\n\
                           MEMORY.md\n\
                           *.sync-conflict-*\n";
            if let Err(e) = std::fs::write(&stignore, content) {
                warn!(%e, "failed to create .stignore");
            } else {
                info!("created .stignore in memory dir");
            }
        }
    }

    /// Regenerate MEMORY.md from the frontmatter of all memory files.
    /// Each file's YAML frontmatter must contain `name` and `description`.
    /// Produces one index line per file: `- [Name](filename.md) — description`
    pub fn regenerate_index(&self) {
        let dir = self.memory_dir();
        if !dir.is_dir() {
            return;
        }

        let mut entries: Vec<(String, String, String)> = Vec::new(); // (name, filename, description)

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                warn!(%e, "failed to read memory dir for index regeneration");
                return;
            }
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            let filename = entry.file_name().to_string_lossy().into_owned();

            // Skip non-markdown, the index itself, stignore, and conflict files.
            if !filename.ends_with(".md")
                || filename == "MEMORY.md"
                || filename.contains(".sync-conflict-")
            {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some((name, description)) = parse_frontmatter(&content) {
                    entries.push((name, filename, description));
                }
            }
        }

        entries.sort_by(|a, b| a.1.cmp(&b.1));

        let mut index = String::new();
        for (name, filename, description) in &entries {
            index.push_str(&format!("- [{}]({}) — {}\n", name, filename, description));
        }

        let index_path = dir.join("MEMORY.md");

        // Only write if content changed — avoid unnecessary mtime bumps
        // that would trigger more Syncthing churn.
        let current = std::fs::read_to_string(&index_path).unwrap_or_default();
        if current != index {
            if let Err(e) = std::fs::write(&index_path, &index) {
                warn!(%e, "failed to write MEMORY.md");
            } else {
                info!(entries = entries.len(), "regenerated MEMORY.md");
            }
        }
    }
}

/// Extract `name` and `description` from YAML frontmatter.
/// Expects `---` delimited block at the start of the file.
fn parse_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let after_open = &content[3..];
    let close = after_open.find("---")?;
    let block = &after_open[..close];

    let mut name = None;
    let mut description = None;

    for line in block.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        }
    }

    match (name, description) {
        (Some(n), Some(d)) => Some((n, d)),
        _ => None,
    }
}

#[interface(name = "org.cairn.Companion1")]
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

        // D-Bus trust = Owner. The org.cairn.Companion1 interface lives
        // on the session bus, which is UID-guarded — only processes
        // running as Keith can reach it. Anything able to call this is
        // already running with Keith's trust. (Pre-existing concern:
        // any process Keith runs gets Owner trust here, including a
        // compromised npm install or browser tab reaching localhost
        // through dbus-broker. Tracked separately, not blocking this
        // change.)
        let req = TurnRequest {
            surface_id: surface.to_string(),
            conversation_id: conversation_id.to_string(),
            message_text: message.to_string(),
            trust: TrustLevel::Owner,
            model: None,
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

        // Same Owner-trust rationale as send_message above — the
        // session bus is UID-guarded.
        let req = TurnRequest {
            surface_id: surface.to_string(),
            conversation_id: conversation_id.to_string(),
            message_text: message.to_string(),
            trust: TrustLevel::Owner,
            model: None,
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

    /// Fetch one session's full details.
    ///
    /// Returns the full session row as a tuple — same shape as list_sessions'
    /// element type, extended with created_at and metadata. Empty strings
    /// stand in for NULL claude_session_id / metadata.
    async fn get_session(
        &self,
        surface: &str,
        conversation_id: &str,
    ) -> zbus::fdo::Result<(String, String, String, String, u32, u32, String)> {
        let store = self.dispatcher.store().await;
        let session = store
            .lookup_session(surface, conversation_id)
            .map_err(|e| zbus::fdo::Error::Failed(format!("session store error: {e}")))?
            .ok_or_else(|| {
                zbus::fdo::Error::FileNotFound(format!(
                    "no session for surface={surface} conversation_id={conversation_id}"
                ))
            })?;

        Ok((
            session.surface,
            session.conversation_id,
            session.claude_session_id.unwrap_or_default(),
            session.status,
            session.created_at as u32,
            session.last_active_at as u32,
            session.metadata.unwrap_or_default(),
        ))
    }

    /// Delete a session by (surface, conversation_id). Returns true if a row
    /// was removed, false if no such session existed.
    async fn delete_session(
        &self,
        surface: &str,
        conversation_id: &str,
    ) -> zbus::fdo::Result<bool> {
        let store = self.dispatcher.store().await;
        store
            .delete_session(surface, conversation_id)
            .map_err(|e| zbus::fdo::Error::Failed(format!("session store error: {e}")))
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

    // -- Memory --

    /// Return the resolved path to Claude Code's project memory directory
    /// for the workspace. Returns empty string if the directory doesn't
    /// exist yet (no session has written memory).
    async fn get_memory_path(&self) -> zbus::fdo::Result<String> {
        let path = self.memory_dir();
        Ok(if path.is_dir() {
            path.to_string_lossy().into_owned()
        } else {
            String::new()
        })
    }

    /// List memory files: Vec<(filename, size_bytes, mtime_epoch)>.
    /// Returns empty if the memory directory doesn't exist.
    async fn list_memory_files(&self) -> zbus::fdo::Result<Vec<(String, u64, i64)>> {
        let dir = self.memory_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&dir).map_err(|e| {
            zbus::fdo::Error::Failed(format!("failed to read memory dir: {e}"))
        })?;

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let meta = entry.metadata().unwrap_or_else(|_| {
                    std::fs::metadata(&path).expect("metadata")
                });
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                entries.push((name, size, mtime));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    /// Read a single memory file's contents. Returns FileNotFound if
    /// the file doesn't exist (or the memory dir itself doesn't exist).
    async fn read_memory_file(&self, name: &str) -> zbus::fdo::Result<String> {
        // Reject path traversal
        if name.contains('/') || name.contains('\\') || name == ".." || name == "." {
            return Err(zbus::fdo::Error::InvalidArgs(
                "filename must not contain path separators".into(),
            ));
        }
        let path = self.memory_dir().join(name);
        std::fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                zbus::fdo::Error::FileNotFound(format!("memory file not found: {name}"))
            }
            _ => zbus::fdo::Error::Failed(format!("failed to read {name}: {e}")),
        })
    }

    /// Return the MEMORY.md index contents. Returns empty string if
    /// the index doesn't exist yet.
    async fn get_memory_index(&self) -> zbus::fdo::Result<String> {
        let path = self.memory_dir().join("MEMORY.md");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(zbus::fdo::Error::Failed(format!(
                "failed to read MEMORY.md: {e}"
            ))),
        }
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

    // Seed .stignore and regenerate index before accepting D-Bus calls.
    iface.ensure_stignore();
    iface.regenerate_index();

    let connection = Connection::session().await?;

    connection
        .object_server()
        .at("/org/cairn/Companion", iface)
        .await?;

    connection
        .request_name("org.cairn.Companion")
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
                        .interface::<_, CompanionInterface>("/org/cairn/Companion")
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

    info!("D-Bus interface ready on org.cairn.Companion");
    Ok(connection)
}

/// Spawn a background task that periodically regenerates MEMORY.md
/// from frontmatter. Catches Syncthing-propagated files that arrive
/// after startup. Runs every 30 seconds — cheap (stat + maybe one write).
pub fn spawn_memory_index_task(dispatcher: Arc<Dispatcher>) {
    let iface = CompanionInterface::new(dispatcher);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            iface.ensure_stignore();
            iface.regenerate_index();
        }
    });
}
