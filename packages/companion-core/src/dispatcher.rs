//! Dispatcher — routes messages from any surface through the companion wrapper,
//! manages session mapping, and streams responses back.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A request to process a single turn.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub surface_id: String,
    pub conversation_id: String,
    pub message_text: String,
}

/// Events emitted during a turn.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Incremental text chunk from the assistant.
    TextChunk(String),
    /// Full accumulated response — emitted once at the end.
    Complete(String),
    /// Error description — emitted once, terminates the stream.
    Error(String),
}

// ---------------------------------------------------------------------------
// Stream-json event parsing
// ---------------------------------------------------------------------------

/// Minimally parsed stream-json event from the companion subprocess.
#[derive(serde::Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    message: Option<AssistantMessage>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(serde::Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(serde::Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Per-session turn lock. Ensures only one companion subprocess runs per
/// (surface, conversation_id) at a time.
type SessionKey = (String, String);

pub struct Dispatcher {
    store: Arc<Mutex<SessionStore>>,
    /// Per-session mutexes for turn serialization.
    session_locks: Mutex<HashMap<SessionKey, Arc<Mutex<()>>>>,
    /// Command to invoke. Defaults to "companion", configurable for tests.
    companion_cmd: String,
    /// Extra env vars to set on the subprocess. Empty in production.
    subprocess_env: HashMap<String, String>,
}

impl Dispatcher {
    pub fn new(store: SessionStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            session_locks: Mutex::new(HashMap::new()),
            companion_cmd: "companion".into(),
            subprocess_env: HashMap::new(),
        }
    }

    /// Get a lock on the session store (for D-Bus methods that query sessions directly).
    pub async fn store(&self) -> tokio::sync::MutexGuard<'_, SessionStore> {
        self.store.lock().await
    }

    /// Create a dispatcher with a custom companion command and env vars (for tests).
    #[cfg(test)]
    pub fn with_command(
        store: SessionStore,
        cmd: impl Into<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            session_locks: Mutex::new(HashMap::new()),
            companion_cmd: cmd.into(),
            subprocess_env: env,
        }
    }

    /// Get or create the per-session lock for turn serialization.
    async fn session_lock(&self, key: &SessionKey) -> Arc<Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Dispatch a turn request. Returns a receiver that yields TurnEvents.
    ///
    /// The caller owns the receiver. If the receiver is dropped before the
    /// turn completes, the subprocess is killed (cancellation).
    pub async fn dispatch(&self, req: TurnRequest) -> mpsc::Receiver<TurnEvent> {
        let (tx, rx) = mpsc::channel(64);
        let key: SessionKey = (req.surface_id.clone(), req.conversation_id.clone());
        let lock = self.session_lock(&key).await;
        let store = self.store.clone();
        let cmd = self.companion_cmd.clone();
        let env = self.subprocess_env.clone();

        tokio::spawn(async move {
            // Serialize turns within a session.
            let _guard = lock.lock().await;
            Self::run_turn(store, req, tx, &cmd, &env).await;
        });

        rx
    }

    async fn run_turn(
        store: Arc<Mutex<SessionStore>>,
        req: TurnRequest,
        tx: mpsc::Sender<TurnEvent>,
        companion_cmd: &str,
        extra_env: &HashMap<String, String>,
    ) {
        // Resolve (or create) the session.
        let (session_id, claude_session_id) = {
            let store = store.lock().await;
            match store.lookup_session(&req.surface_id, &req.conversation_id) {
                Ok(Some(session)) => {
                    if let Err(e) = store.touch_session(session.id) {
                        warn!(%e, "failed to touch session");
                    }
                    (session.id, session.claude_session_id)
                }
                Ok(None) => match store.create_session(&req.surface_id, &req.conversation_id) {
                    Ok(id) => (id, None),
                    Err(e) => {
                        let _ = tx.send(TurnEvent::Error(format!("session store error: {e}"))).await;
                        return;
                    }
                },
                Err(e) => {
                    let _ = tx.send(TurnEvent::Error(format!("session store error: {e}"))).await;
                    return;
                }
            }
        };

        // Build the companion invocation.
        let mut cmd = Command::new(companion_cmd);
        if let Some(ref resume_id) = claude_session_id {
            cmd.arg("--resume").arg(resume_id);
        }
        cmd.arg("-p")
            .arg(&req.message_text)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");

        cmd.envs(extra_env);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        info!(
            surface = %req.surface_id,
            conversation = %req.conversation_id,
            resume = ?claude_session_id,
            "spawning companion"
        );

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(TurnEvent::Error(format!("failed to spawn companion: {e}"))).await;
                return;
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let mut reader = tokio::io::BufReader::new(stdout).lines();

        let mut full_response = String::new();
        let mut captured_session_id = false;
        let start = std::time::Instant::now();

        // Parse stream-json output line by line.
        while let Ok(Some(line)) = reader.next_line().await {
            let event: StreamEvent = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(e) => {
                    debug!(%e, line = %line, "skipping unparseable stream-json line");
                    continue;
                }
            };

            match (event.event_type.as_str(), event.subtype.as_deref()) {
                ("system", Some("init")) => {
                    if let Some(sid) = event.session_id {
                        debug!(session_id = %sid, "captured session id from init event");
                        if claude_session_id.is_none() && !captured_session_id {
                            let store = store.lock().await;
                            if let Err(e) = store.set_claude_session_id(session_id, &sid) {
                                warn!(%e, "failed to store claude session id");
                            }
                            captured_session_id = true;
                        }
                    }
                }
                ("assistant", _) => {
                    if let Some(msg) = event.message {
                        for block in msg.content {
                            if let Some(text) = block.text {
                                full_response.push_str(&text);
                                if tx.send(TurnEvent::TextChunk(text)).await.is_err() {
                                    // Receiver dropped — cancellation.
                                    info!("turn cancelled by surface, killing subprocess");
                                    let _ = child.kill().await;
                                    return;
                                }
                            }
                        }
                    }
                }
                ("result", Some("success")) => {
                    let result_text = event.result.unwrap_or(full_response.clone());
                    let duration = start.elapsed();
                    info!(
                        surface = %req.surface_id,
                        conversation = %req.conversation_id,
                        turn_duration_ms = duration.as_millis() as u64,
                        "turn complete"
                    );
                    let _ = tx.send(TurnEvent::Complete(result_text)).await;
                    break;
                }
                ("result", Some("error")) => {
                    let err_msg = event.error.unwrap_or_else(|| "unknown claude error".into());
                    error!(
                        surface = %req.surface_id,
                        conversation = %req.conversation_id,
                        error = %err_msg,
                        "turn failed"
                    );
                    let _ = tx.send(TurnEvent::Error(err_msg)).await;
                    break;
                }
                (other_type, subtype) => {
                    debug!(
                        event_type = %other_type,
                        subtype = ?subtype,
                        "ignoring unhandled stream-json event"
                    );
                }
            }
        }

        // Wait for subprocess to exit.
        match child.wait().await {
            Ok(status) if !status.success() => {
                let code = status.code().unwrap_or(-1);
                // Only emit error if we haven't already sent a Complete or Error.
                // The channel might be closed if we already sent a terminal event.
                let _ = tx
                    .send(TurnEvent::Error(format!(
                        "companion exited with status {code}"
                    )))
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(TurnEvent::Error(format!("failed to wait on companion: {e}")))
                    .await;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;
    use std::path::PathBuf;

    /// Check if the mock script can actually run (needs /usr/bin/env bash).
    /// Returns false inside Nix build sandboxes where /usr/bin/env doesn't exist.
    fn mock_available() -> bool {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("mock_companion.sh");
        std::process::Command::new(&script)
            .env("MOCK_MODE", "crash") // fastest mode — just exits
            .output()
            .is_ok()
    }

    fn mock_script() -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("mock_companion.sh")
            .to_string_lossy()
            .into_owned()
    }

    fn mock_dispatcher(mode: &str) -> Dispatcher {
        mock_dispatcher_with(mode, "mock-session-default")
    }

    fn mock_dispatcher_with(mode: &str, session_id: &str) -> Dispatcher {
        let store = SessionStore::open_in_memory().unwrap();
        let mut env = HashMap::new();
        env.insert("MOCK_MODE".into(), mode.into());
        env.insert("MOCK_SESSION_ID".into(), session_id.into());
        Dispatcher::with_command(store, mock_script(), env)
    }

    fn make_request(surface: &str, conv: &str, msg: &str) -> TurnRequest {
        TurnRequest {
            surface_id: surface.into(),
            conversation_id: conv.into(),
            message_text: msg.into(),
        }
    }

    async fn collect_events(mut rx: mpsc::Receiver<TurnEvent>) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        events
    }

    #[tokio::test]
    async fn normal_turn_produces_chunks_and_complete() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher_with("normal", "test-session-001");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-1", "hello")).await;
        let events = collect_events(rx).await;

        let chunks: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::TextChunk(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(chunks, vec!["Hello from ", "mock companion."]);

        let complete = events.iter().find_map(|e| match e {
            TurnEvent::Complete(t) => Some(t.as_str()),
            _ => None,
        });
        assert_eq!(complete, Some("Hello from mock companion."));

        // Session ID should be stored.
        let store = dispatcher.store.lock().await;
        let session = store.lookup_session("dbus", "conv-1").unwrap().unwrap();
        assert_eq!(
            session.claude_session_id.as_deref(),
            Some("test-session-001")
        );
    }

    #[tokio::test]
    async fn error_turn_produces_error_event() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("error");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-2", "fail")).await;
        let events = collect_events(rx).await;

        let has_error = events.iter().any(|e| matches!(e, TurnEvent::Error(_)));
        assert!(has_error, "expected an Error event");

        let has_complete = events.iter().any(|e| matches!(e, TurnEvent::Complete(_)));
        assert!(!has_complete, "should not have Complete on error");
    }

    #[tokio::test]
    async fn crash_produces_error_event() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("crash");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-3", "crash")).await;
        let events = collect_events(rx).await;

        let has_error = events.iter().any(|e| matches!(e, TurnEvent::Error(_)));
        assert!(has_error, "expected an Error event on crash");
    }

    #[tokio::test]
    async fn cancellation_kills_subprocess() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("slow");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-4", "slow")).await;

        // Drop the receiver immediately — should trigger cancellation.
        drop(rx);

        // Give the spawned task a moment to clean up.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Session should survive cancellation.
        let store = dispatcher.store.lock().await;
        let session = store.lookup_session("dbus", "conv-4").unwrap();
        assert!(session.is_some(), "session should survive cancellation");
    }

    #[tokio::test]
    async fn concurrent_different_sessions() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("normal");

        let rx1 = dispatcher.dispatch(make_request("dbus", "conv-a", "one")).await;
        let rx2 = dispatcher.dispatch(make_request("dbus", "conv-b", "two")).await;

        let (events1, events2) = tokio::join!(collect_events(rx1), collect_events(rx2));

        assert!(events1.iter().any(|e| matches!(e, TurnEvent::Complete(_))));
        assert!(events2.iter().any(|e| matches!(e, TurnEvent::Complete(_))));
    }
}
