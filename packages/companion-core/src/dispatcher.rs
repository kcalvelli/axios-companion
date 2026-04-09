//! Dispatcher — routes messages from any surface through the companion wrapper,
//! manages session mapping, and streams responses back.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, error, info, warn};

use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-surface trust tier. Decides which Claude Code tools the spawned
/// companion subprocess is allowed to call. Set at the channel adapter
/// boundary, after that surface's existing identity check (allowed_jids,
/// allowed_users, etc.) passes.
///
/// Two tiers, deliberately. There is no in-between — every surface is
/// either "Keith is the verified speaker" or "anyone could be on the wire".
/// If you ever need a third tier, add a `trusted_<surface>` config to the
/// channel adapter and decide there, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    /// The surface has verified the speaker is the owner. The companion
    /// subprocess gets the curated MCP allowlist (mail/dav/sentinel) on
    /// top of whatever Keith's user-level `~/.claude/settings.local.json`
    /// already permits. Owner == Keith, so inheriting his interactive
    /// allow list is correct, not a leak.
    Owner,
    /// The surface cannot vouch for the speaker (XMPP MUC, openai-gateway
    /// over HTTP). Strip every dangerous tool from the model's view via
    /// `permissions.deny`, so it never even sees them as options. Replies
    /// in this tier are pure conversation — no tool calls, no file
    /// access, no MCP, no shell.
    Anonymous,
}

/// A request to process a single turn.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub surface_id: String,
    pub conversation_id: String,
    pub message_text: String,
    /// The trust tier the originating surface vouches for. The dispatcher
    /// translates this into per-tier `--permission-mode dontAsk --settings`
    /// flags on the companion subprocess. Required, not defaulted —
    /// channel adapters MUST decide explicitly.
    pub trust: TrustLevel,
}

// ---------------------------------------------------------------------------
// Per-trust-tier Claude Code settings JSON
// ---------------------------------------------------------------------------
//
// Both literals are passed inline via `--settings '<json>'` on the
// companion subprocess argv (see run_turn). The shape matches
// `~/.claude/settings.json`'s `permissions.{allow,deny}`. Inline settings
// are MERGED with the user-level settings file, NOT replaced — verified
// 2026-04-09 against claude-code 2.1.92.
//
// Owner uses an `allow` list because the merge is desirable: Keith's
// own interactive allow rules + the curated MCP set = full owner
// toolkit, no Keith intervention needed.
//
// Anonymous uses a `deny` list, NOT `allow`-with-empty, because the
// merge would cause Keith's interactive allow rules to leak through.
// `permissions.deny` strips tools from the model's `tools[]` array
// entirely (verified live: the model can't even ToolSearch for a
// denied tool), so anonymous turns never produce hallucinated
// tool-result text.
//
// Wildcards (`*`, `mcp__*`) do NOT work in either list — verified
// against claude-code 2.1.92. Every tool name must be enumerated.
// New built-in or MCP tools added by claude-code or by the daemon's
// `--mcp-config` need to be added to ANONYMOUS_DENY below or they
// will silently start being allowed in MUC / openai-gateway turns.

const OWNER_SETTINGS_JSON: &str = r#"{"permissions":{"allow":["mcp__axios-ai-mail__*","mcp__mcp-dav__*","mcp__sentinel__*"]}}"#;

// Enumerated deny list for the Anonymous tier. Includes:
//   - all built-in claude-code tools (Bash, Edit, Read, ...) observed
//     in the init event of a non-bare claude invocation
//   - all deferred tools that could create state, spawn agents, or
//     cost money (Cron*, Task, Worktree*, RemoteTrigger, Skill)
//   - every MCP tool the daemon's --mcp-config could plausibly load
//     (axios-ai-mail, mcp-dav, sentinel) plus the always-loaded
//     claude_ai_Gmail / claude_ai_Google_Calendar OAuth flows
//
// Tools intentionally NOT denied (because they're UI/state-only and
// can't affect the daemon or the outside world): AskUserQuestion,
// EnterPlanMode, ExitPlanMode, TodoWrite, TaskOutput, TaskStop,
// ToolSearch (denying ToolSearch breaks the model's ability to
// discover tools, but since everything else is denied that's moot).
// ToolSearch is left in only because the model uses it harmlessly
// when it can't find a tool — see the live pre-flight transcript.
const ANONYMOUS_SETTINGS_JSON: &str = r#"{"permissions":{"allow":[],"deny":["Bash","Edit","Write","Read","Glob","Grep","WebFetch","WebSearch","NotebookEdit","Task","Skill","RemoteTrigger","CronCreate","CronDelete","CronList","EnterWorktree","ExitWorktree","mcp__axios-ai-mail__bulk_update_tags","mcp__axios-ai-mail__compose_email","mcp__axios-ai-mail__delete_by_filter","mcp__axios-ai-mail__delete_email","mcp__axios-ai-mail__get_unread_count","mcp__axios-ai-mail__list_accounts","mcp__axios-ai-mail__list_tags","mcp__axios-ai-mail__mark_read","mcp__axios-ai-mail__read_email","mcp__axios-ai-mail__reply_to_email","mcp__axios-ai-mail__restore_email","mcp__axios-ai-mail__search_emails","mcp__axios-ai-mail__send_email","mcp__axios-ai-mail__update_tags","mcp__mcp-dav__create_contact","mcp__mcp-dav__create_event","mcp__mcp-dav__delete_contact","mcp__mcp-dav__get_contact","mcp__mcp-dav__get_free_busy","mcp__mcp-dav__list_contacts","mcp__mcp-dav__list_events","mcp__mcp-dav__search_contacts","mcp__mcp-dav__search_events","mcp__mcp-dav__update_contact","mcp__sentinel__check_fleet_health","mcp__sentinel__host_disk","mcp__sentinel__host_gpu","mcp__sentinel__host_temperatures","mcp__sentinel__list_hosts","mcp__sentinel__query_host","mcp__sentinel__reboot_host","mcp__sentinel__restart_service","mcp__sentinel__system_status","mcp__sentinel__view_logs","mcp__claude_ai_Gmail__authenticate","mcp__claude_ai_Google_Calendar__authenticate"]}}"#;

fn settings_json_for(trust: TrustLevel) -> &'static str {
    match trust {
        TrustLevel::Owner => OWNER_SETTINGS_JSON,
        TrustLevel::Anonymous => ANONYMOUS_SETTINGS_JSON,
    }
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

/// A TurnEvent tagged with the surface and conversation it belongs to.
/// Sent on the broadcast channel so observers (D-Bus signal emitter, etc.)
/// can see all traffic regardless of which surface originated it.
#[derive(Debug, Clone)]
pub struct BroadcastEvent {
    pub surface: String,
    pub conversation_id: String,
    pub event: TurnEvent,
}

// ---------------------------------------------------------------------------
// Stream-json event parsing
// ---------------------------------------------------------------------------

/// Minimally parsed stream-json event from the companion subprocess.
///
/// Claude's `--output-format stream-json --verbose --include-partial-messages`
/// produces a few different event shapes; we only care about a handful and
/// let the rest fall through to a debug log:
///
/// - `system/init` — carries the claude session id we persist for resume
/// - `stream_event` wrapping a `content_block_delta` with `text_delta` —
///   token-level streaming. The actual delta lives at `event.delta.text`,
///   so we keep the inner blob as a raw `serde_json::Value` and navigate
///   it in the handler. (Defining a typed schema for every inner event
///   shape would be a lot of code for one read site.)
/// - `assistant` — the legacy aggregated message, used as a fallback when
///   partial deltas are unavailable (e.g. mock fixtures in tests).
/// - `result/success` — canonical final text and turn complete
/// - `result/error` — turn failed
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
    event: Option<serde_json::Value>,
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
    /// Broadcast channel for all turn events across all surfaces.
    broadcast_tx: broadcast::Sender<BroadcastEvent>,
}

impl Dispatcher {
    pub fn new(store: SessionStore) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            store: Arc::new(Mutex::new(store)),
            session_locks: Mutex::new(HashMap::new()),
            companion_cmd: "companion".into(),
            subprocess_env: HashMap::new(),
            broadcast_tx,
        }
    }

    /// Subscribe to the broadcast channel for all turn events.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.broadcast_tx.subscribe()
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
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            store: Arc::new(Mutex::new(store)),
            session_locks: Mutex::new(HashMap::new()),
            companion_cmd: cmd.into(),
            subprocess_env: env,
            broadcast_tx,
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
        let broadcast_tx = self.broadcast_tx.clone();

        tokio::spawn(async move {
            // Serialize turns within a session.
            let _guard = lock.lock().await;
            Self::run_turn(store, req, tx, broadcast_tx, &cmd, &env).await;
        });

        rx
    }

    async fn run_turn(
        store: Arc<Mutex<SessionStore>>,
        req: TurnRequest,
        tx: mpsc::Sender<TurnEvent>,
        broadcast_tx: broadcast::Sender<BroadcastEvent>,
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
                        let err = TurnEvent::Error(format!("session store error: {e}"));
                        let _ = broadcast_tx.send(BroadcastEvent { surface: req.surface_id.clone(), conversation_id: req.conversation_id.clone(), event: err.clone() });
                        let _ = tx.send(err).await;
                        return;
                    }
                },
                Err(e) => {
                    let err = TurnEvent::Error(format!("session store error: {e}"));
                    let _ = broadcast_tx.send(BroadcastEvent { surface: req.surface_id.clone(), conversation_id: req.conversation_id.clone(), event: err.clone() });
                    let _ = tx.send(err).await;
                    return;
                }
            }
        };

        // Build the companion invocation. The argv order is load-bearing:
        // `-p -- <text>` MUST come last so the `--` flag-stop only blocks
        // claude's parser from interpreting the prompt body as a flag,
        // without also eating downstream arguments. Without this, a prompt
        // body that begins with `-` (a common case in MUC after mention
        // stripping — "Sid - hi" → "- hi") trips claude's CLI parser with
        // `error: unknown option '- hi'` and the subprocess exits with
        // status 1. Verified live against mini's claude-code 2.1.92 in
        // 2026-04-08 — see channel-xmpp Phase 5 live MUC test for context.
        //
        // The `--permission-mode dontAsk` + `--settings` pair MUST sit
        // between `--include-partial-messages` and `--resume` so that
        // `-p --` stays terminal. dontAsk + per-tier settings is what
        // lets channel-delivered turns use MCP tools (Owner) or refuse
        // them visibly (Anonymous) instead of hanging on a permission
        // prompt that nobody can answer. See `TrustLevel` and
        // `settings_json_for` above for the trust model.
        //
        // Note: `companion` resolves via the daemon's systemd PATH
        // override to a writeShellApplication wrapper (see
        // packages/companion/default.nix) which builds its own args[]
        // array (system prompt, workspace dir, --mcp-config) and ends
        // with `exec claude "${args[@]}" "$@"`. Everything we append
        // here lands in `"$@"` and reaches claude unchanged.
        let mut cmd = Command::new(companion_cmd);
        cmd.arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            // --include-partial-messages turns claude's stream into
            // token-level deltas (`stream_event` envelopes wrapping
            // `content_block_delta` / `text_delta`). Without this flag,
            // stream-json only emits one `assistant` event per complete
            // model message — fine for tool-use turns, useless for pure
            // text turns where the user sees nothing until generation
            // ends. The XEP-0308 streaming corrections in
            // `channels::xmpp::stream_single_message` are designed
            // around this delta stream; without this flag, every
            // pure-text turn produces exactly one chunk and zero
            // visible streaming. See dispatcher's stream_event handler
            // below for how the deltas are unwrapped.
            .arg("--include-partial-messages")
            // --permission-mode dontAsk: never prompt the user for tool
            // approval (there is no user at this subprocess); instead
            // either silently allow per the merged settings or deny with
            // a tool_result error. --settings: per-tier inline JSON that
            // either adds the curated MCP allow list (Owner) or denies
            // every dangerous tool by name (Anonymous). See the constants
            // and `settings_json_for` near the top of this file.
            .arg("--permission-mode")
            .arg("dontAsk")
            .arg("--settings")
            .arg(settings_json_for(req.trust));
        if let Some(ref resume_id) = claude_session_id {
            cmd.arg("--resume").arg(resume_id);
        }
        cmd.arg("-p").arg("--").arg(&req.message_text);

        cmd.envs(extra_env);
        cmd.stdout(std::process::Stdio::piped());
        // stderr is inherited (goes to journald via the parent) so claude's
        // own error messages are visible without us having to scrape them.
        // The previous `Stdio::piped()` was a debugging dead-end — we never
        // read from it, which silently dropped every claude error.
        cmd.stderr(std::process::Stdio::inherit());

        info!(
            surface = %req.surface_id,
            conversation = %req.conversation_id,
            resume = ?claude_session_id,
            "spawning companion"
        );

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err = TurnEvent::Error(format!("failed to spawn companion: {e}"));
                let _ = broadcast_tx.send(BroadcastEvent { surface: req.surface_id.clone(), conversation_id: req.conversation_id.clone(), event: err.clone() });
                let _ = tx.send(err).await;
                return;
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let mut reader = tokio::io::BufReader::new(stdout).lines();

        let mut full_response = String::new();
        let mut captured_session_id = false;
        // Set true once we've emitted a TextChunk from a `content_block_delta`
        // (i.e. partial-message streaming is working). Used to suppress the
        // legacy `assistant` event's text emission so the same response
        // doesn't get streamed AND re-emitted as one big chunk at message
        // end. Stays false in tests using the mock fixture (which never
        // emits stream_events) so the legacy path keeps working there.
        let mut seen_partial_text = false;
        let start = std::time::Instant::now();

        // Helper: send event to the caller's channel and the broadcast.
        // Returns false if the caller dropped the receiver (cancellation).
        let emit = |tx: &mpsc::Sender<TurnEvent>,
                    broadcast_tx: &broadcast::Sender<BroadcastEvent>,
                    surface: &str,
                    conversation_id: &str,
                    event: TurnEvent| {
            let _ = broadcast_tx.send(BroadcastEvent {
                surface: surface.to_string(),
                conversation_id: conversation_id.to_string(),
                event: event.clone(),
            });
            tx.try_send(event)
        };

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
                ("stream_event", _) => {
                    // Token-level partial-message stream. We unwrap one
                    // shape: content_block_delta carrying a text_delta.
                    // Everything else (message_start, content_block_start,
                    // message_delta, message_stop, ...) is ignored — we
                    // only need the text deltas to drive XEP-0308
                    // streaming corrections downstream.
                    let inner = match event.event.as_ref() {
                        Some(v) => v,
                        None => continue,
                    };
                    if inner.get("type").and_then(|t| t.as_str()) != Some("content_block_delta") {
                        continue;
                    }
                    let delta = match inner.get("delta") {
                        Some(d) => d,
                        None => continue,
                    };
                    if delta.get("type").and_then(|t| t.as_str()) != Some("text_delta") {
                        continue;
                    }
                    let text = match delta.get("text").and_then(|t| t.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    if text.is_empty() {
                        continue;
                    }
                    seen_partial_text = true;
                    full_response.push_str(&text);
                    if emit(&tx, &broadcast_tx, &req.surface_id, &req.conversation_id, TurnEvent::TextChunk(text)).is_err() {
                        info!("turn cancelled by surface, killing subprocess");
                        let _ = child.kill().await;
                        return;
                    }
                }
                ("assistant", _) => {
                    // Legacy aggregated-message path. With
                    // --include-partial-messages enabled in production,
                    // every text response we'd emit here has already
                    // been streamed via content_block_delta events
                    // above — re-emitting it would duplicate the body.
                    // Skip text emission once we've seen any partial
                    // delta. The mock fixture in tests doesn't emit
                    // stream_events, so seen_partial_text stays false
                    // and the legacy emission keeps the test suite
                    // working unchanged.
                    if seen_partial_text {
                        continue;
                    }
                    if let Some(msg) = event.message {
                        for block in msg.content {
                            if let Some(text) = block.text {
                                full_response.push_str(&text);
                                if emit(&tx, &broadcast_tx, &req.surface_id, &req.conversation_id, TurnEvent::TextChunk(text)).is_err() {
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
                    let _ = emit(&tx, &broadcast_tx, &req.surface_id, &req.conversation_id, TurnEvent::Complete(result_text));
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
                    let _ = emit(&tx, &broadcast_tx, &req.surface_id, &req.conversation_id, TurnEvent::Error(err_msg));
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
                let _ = emit(
                    &tx, &broadcast_tx,
                    &req.surface_id, &req.conversation_id,
                    TurnEvent::Error(format!("companion exited with status {code}")),
                );
            }
            Err(e) => {
                let _ = emit(
                    &tx, &broadcast_tx,
                    &req.surface_id, &req.conversation_id,
                    TurnEvent::Error(format!("failed to wait on companion: {e}")),
                );
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

    fn make_request(surface: &str, conv: &str, msg: &str, trust: TrustLevel) -> TurnRequest {
        TurnRequest {
            surface_id: surface.into(),
            conversation_id: conv.into(),
            message_text: msg.into(),
            trust,
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
        let rx = dispatcher.dispatch(make_request("dbus", "conv-1", "hello", TrustLevel::Anonymous)).await;
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
        let rx = dispatcher.dispatch(make_request("dbus", "conv-2", "fail", TrustLevel::Anonymous)).await;
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
        let rx = dispatcher.dispatch(make_request("dbus", "conv-3", "crash", TrustLevel::Anonymous)).await;
        let events = collect_events(rx).await;

        let has_error = events.iter().any(|e| matches!(e, TurnEvent::Error(_)));
        assert!(has_error, "expected an Error event on crash");
    }

    #[tokio::test]
    async fn cancellation_kills_subprocess() {
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("slow");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-4", "slow", TrustLevel::Anonymous)).await;

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

        let rx1 = dispatcher.dispatch(make_request("dbus", "conv-a", "one", TrustLevel::Anonymous)).await;
        let rx2 = dispatcher.dispatch(make_request("dbus", "conv-b", "two", TrustLevel::Anonymous)).await;

        let (events1, events2) = tokio::join!(collect_events(rx1), collect_events(rx2));

        assert!(events1.iter().any(|e| matches!(e, TurnEvent::Complete(_))));
        assert!(events2.iter().any(|e| matches!(e, TurnEvent::Complete(_))));
    }

    /// Spin up a dispatcher in `argv` mock mode and run one turn at the
    /// given trust level. Returns the captured argv (one entry per line)
    /// the dispatcher actually passed to the companion subprocess.
    ///
    /// This is the regression guard for the channel-permissions fix —
    /// it asserts that `--permission-mode dontAsk` and the right
    /// per-tier `--settings` JSON literal land on the spawned process,
    /// so a future refactor can't silently drop them.
    async fn capture_dispatch_argv(trust: TrustLevel) -> Vec<String> {
        let argv_path = std::env::temp_dir().join(format!(
            "companion-argv-test-{}.txt",
            uuid::Uuid::new_v4()
        ));
        let mut env = HashMap::new();
        env.insert("MOCK_MODE".into(), "argv".into());
        env.insert("MOCK_SESSION_ID".into(), "argv-test-session".into());
        env.insert(
            "MOCK_ARGV_FILE".into(),
            argv_path.to_string_lossy().into_owned(),
        );
        let store = SessionStore::open_in_memory().unwrap();
        let dispatcher = Dispatcher::with_command(store, mock_script(), env);
        let rx = dispatcher
            .dispatch(make_request("test", "argv-conv", "hi", trust))
            .await;
        // Drain to make sure the subprocess has actually run.
        let _ = collect_events(rx).await;
        let captured = std::fs::read_to_string(&argv_path)
            .expect("argv file should exist after argv-mode mock ran");
        let _ = std::fs::remove_file(&argv_path);
        captured.lines().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn passes_permission_mode_and_settings_owner() {
        if !mock_available() { return; }
        let argv = capture_dispatch_argv(TrustLevel::Owner).await;

        // --permission-mode dontAsk must be present, in that order.
        let mode_idx = argv
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("--permission-mode must be in argv");
        assert_eq!(
            argv.get(mode_idx + 1).map(String::as_str),
            Some("dontAsk"),
            "--permission-mode must be followed by dontAsk"
        );

        // --settings must be present and followed by the OWNER literal.
        let settings_idx = argv
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings must be in argv");
        assert_eq!(
            argv.get(settings_idx + 1).map(String::as_str),
            Some(OWNER_SETTINGS_JSON),
            "--settings must carry the owner JSON literal verbatim"
        );

        // The owner literal must allow the curated MCP namespaces.
        assert!(
            OWNER_SETTINGS_JSON.contains("mcp__axios-ai-mail__*"),
            "owner settings must allow axios-ai-mail MCP tools"
        );
        assert!(
            OWNER_SETTINGS_JSON.contains("mcp__mcp-dav__*"),
            "owner settings must allow mcp-dav MCP tools"
        );
        assert!(
            OWNER_SETTINGS_JSON.contains("mcp__sentinel__*"),
            "owner settings must allow sentinel MCP tools"
        );

        // -p MUST stay terminal — no flag may follow it before --.
        let p_idx = argv
            .iter()
            .position(|a| a == "-p")
            .expect("-p must be in argv");
        assert!(
            settings_idx < p_idx,
            "--settings must precede -p so the prompt body stays terminal"
        );
        assert!(
            mode_idx < p_idx,
            "--permission-mode must precede -p so the prompt body stays terminal"
        );
    }

    #[tokio::test]
    async fn passes_permission_mode_and_settings_anonymous() {
        if !mock_available() { return; }
        let argv = capture_dispatch_argv(TrustLevel::Anonymous).await;

        let mode_idx = argv
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("--permission-mode must be in argv");
        assert_eq!(
            argv.get(mode_idx + 1).map(String::as_str),
            Some("dontAsk"),
            "--permission-mode must be followed by dontAsk"
        );

        let settings_idx = argv
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings must be in argv");
        assert_eq!(
            argv.get(settings_idx + 1).map(String::as_str),
            Some(ANONYMOUS_SETTINGS_JSON),
            "--settings must carry the anonymous JSON literal verbatim"
        );

        // Anonymous must DENY (not allow) the dangerous tool set.
        assert!(
            ANONYMOUS_SETTINGS_JSON.contains(r#""deny":["#),
            "anonymous settings must use a deny list"
        );
        for tool in &[
            "Bash", "Edit", "Write", "Read", "WebFetch", "WebSearch",
            "mcp__axios-ai-mail__send_email",
            "mcp__sentinel__reboot_host",
            "mcp__mcp-dav__delete_contact",
        ] {
            assert!(
                ANONYMOUS_SETTINGS_JSON.contains(tool),
                "anonymous deny list must include {}",
                tool
            );
        }
    }

    #[tokio::test]
    async fn partial_messages_emit_token_chunks_and_dedupe_legacy_assistant() {
        // Drives the `partial` mock mode, which produces three text deltas
        // wrapped in stream_event/content_block_delta envelopes followed by
        // the legacy `assistant` aggregate. The dispatcher must:
        //   1. Emit one TextChunk per delta (in order)
        //   2. SUPPRESS the legacy assistant event's text emission since
        //      `seen_partial_text` is now true (otherwise the response
        //      would arrive twice — once streamed, once aggregated)
        //   3. Emit the canonical Complete from result/success
        //
        // This is the regression guard for the dispatcher fix that finally
        // makes channel-xmpp Phase 4.2 streaming actually visible to users.
        if !mock_available() { return; }
        let dispatcher = mock_dispatcher("partial");
        let rx = dispatcher.dispatch(make_request("dbus", "conv-partial", "stream please", TrustLevel::Anonymous)).await;
        let events = collect_events(rx).await;

        let chunks: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::TextChunk(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec!["Hello ", "streaming ", "world"],
            "expected token-level chunks from content_block_delta deltas \
             with no legacy-assistant duplication"
        );

        let complete = events.iter().find_map(|e| match e {
            TurnEvent::Complete(t) => Some(t.as_str()),
            _ => None,
        });
        assert_eq!(complete, Some("Hello streaming world"));
    }
}
