//! OpenAI-compatible HTTP gateway for the companion daemon.
//!
//! Exposes `/v1/chat/completions`, `/v1/models`, and `/health` endpoints.
//! Routes requests through the shared dispatcher, mapping between OpenAI
//! format and the daemon's TurnRequest/TurnEvent types.

pub mod types;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tracing::{error, info, warn};

use crate::dispatcher::{Dispatcher, TurnEvent, TurnRequest};
use types::*;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct AppState {
    dispatcher: Arc<Dispatcher>,
    config: GatewayConfig,
    start_time: u64,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the gateway HTTP server. Returns a `JoinHandle` that resolves when
/// the server shuts down. Pass a `shutdown` future to trigger graceful stop.
pub async fn serve(
    dispatcher: Arc<Dispatcher>,
    config: GatewayConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let start_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let app = build_router(dispatcher, config.clone(), start_time);

    let addr = format!("{}:{}", config.bind_address, config.port);
    let listener = TcpListener::bind(&addr).await?;
    info!(addr = %addr, "OpenAI gateway listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(ModelsResponse::new(
        state.config.model_name.clone(),
        state.start_time,
    ))
}

async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    // Parse request body.
    let req: ChatCompletionRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                OpenAIErrorEnvelope::invalid_json(format!("Invalid JSON: {e}")),
            );
        }
    };

    // Validate messages.
    if req.messages.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            OpenAIErrorEnvelope::invalid_messages("messages array is empty"),
        );
    }

    // Extract last user message.
    let user_message = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .map(|s| s.to_string());

    let user_message = match user_message {
        Some(m) if !m.is_empty() => m,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                OpenAIErrorEnvelope::no_user_message(),
            );
        }
    };

    // Collect system messages and prepend them as context to the user message.
    // This preserves instructions from callers like sid-assistant (HA) that use
    // the system role to provide action context or behavioral guidance.
    let system_context: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .filter_map(|m| m.content.as_deref())
        .filter(|s| !s.is_empty())
        .collect();

    let message_text = if system_context.is_empty() {
        user_message
    } else {
        format!("{}\n\n{}", system_context.join("\n\n"), user_message)
    };

    // Resolve conversation ID per session policy.
    let conversation_id = resolve_conversation_id(&state.config, &headers);

    let turn_req = TurnRequest {
        surface_id: "openai".into(),
        conversation_id,
        message_text,
    };

    let streaming = req.stream.unwrap_or(false);
    let request_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = state.start_time;
    let model = state.config.model_name.clone();

    if streaming {
        handle_streaming(state, turn_req, request_id, model, created).await
    } else {
        handle_non_streaming(state, turn_req, request_id, model, created).await
    }
}

async fn fallback() -> Response {
    error_response(StatusCode::NOT_FOUND, OpenAIErrorEnvelope::not_found())
}

// ---------------------------------------------------------------------------
// Non-streaming path
// ---------------------------------------------------------------------------

async fn handle_non_streaming(
    state: Arc<AppState>,
    req: TurnRequest,
    request_id: String,
    model: String,
    created: u64,
) -> Response {
    let mut rx = state.dispatcher.dispatch(req).await;
    let mut full_text = String::new();
    let mut had_error = false;
    let mut error_detail = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => full_text.push_str(&chunk),
            TurnEvent::Complete(text) => {
                full_text = text;
                break;
            }
            TurnEvent::Error(e) => {
                error!(%e, "dispatcher error during non-streaming completion");
                had_error = true;
                error_detail = e;
                break;
            }
        }
    }

    if had_error {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            OpenAIErrorEnvelope::companion_error(error_detail),
        );
    }

    let completion = ChatCompletion::new(request_id, model, full_text, created);
    (StatusCode::OK, Json(completion)).into_response()
}

// ---------------------------------------------------------------------------
// Streaming path
// ---------------------------------------------------------------------------

async fn handle_streaming(
    state: Arc<AppState>,
    req: TurnRequest,
    request_id: String,
    model: String,
    created: u64,
) -> Response {
    let rx = state.dispatcher.dispatch(req).await;
    let stream = ReceiverStream::new(rx);

    let mut is_first = true;
    let id = request_id;
    let mdl = model;

    let sse_stream = stream.map(move |event| {
        match event {
            TurnEvent::TextChunk(chunk) => {
                let chunk_obj = if is_first {
                    is_first = false;
                    ChatCompletionChunk::first(&id, &mdl, chunk, created)
                } else {
                    ChatCompletionChunk::content(&id, &mdl, chunk, created)
                };
                let json = serde_json::to_string(&chunk_obj).unwrap_or_default();
                Ok(Event::default().data(json))
            }
            TurnEvent::Complete(_) => {
                let stop = ChatCompletionChunk::stop(&id, &mdl, created);
                let json = serde_json::to_string(&stop).unwrap_or_default();
                // We'll send the stop chunk — the [DONE] sentinel comes after.
                Ok(Event::default().data(json))
            }
            TurnEvent::Error(e) => {
                warn!(%e, "dispatcher error during streaming completion");
                let err = OpenAIErrorEnvelope::companion_error(&e);
                let json = serde_json::to_string(&err).unwrap_or_default();
                Ok(Event::default().data(json))
            }
        }
    });

    // Append [DONE] after the dispatcher stream ends.
    let done_stream = tokio_stream::once(Ok::<_, std::convert::Infallible>(
        Event::default().data("[DONE]"),
    ));
    let full_stream = sse_stream.chain(done_stream);

    Sse::new(full_stream).into_response()
}

// ---------------------------------------------------------------------------
// Session policy
// ---------------------------------------------------------------------------

fn resolve_conversation_id(config: &GatewayConfig, headers: &HeaderMap) -> String {
    match config.session_policy {
        SessionPolicy::Ephemeral => uuid::Uuid::new_v4().to_string(),
        SessionPolicy::SingleSession => "openai-default".into(),
        SessionPolicy::PerConversationId => {
            headers
                .get("x-conversation-id")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "openai-default".into())
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_response(status: StatusCode, envelope: OpenAIErrorEnvelope) -> Response {
    (status, Json(envelope)).into_response()
}

/// Build the router with shared state. Exposed for testing.
fn build_router(
    dispatcher: Arc<Dispatcher>,
    config: GatewayConfig,
    start_time: u64,
) -> Router {
    let state = Arc::new(AppState {
        dispatcher,
        config,
        start_time,
    });

    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .fallback(fallback)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Dispatcher;
    use crate::store::SessionStore;
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tower::util::ServiceExt;

    fn mock_available() -> bool {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("mock_companion.sh");
        std::process::Command::new(&script)
            .env("MOCK_MODE", "crash")
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

    fn test_config() -> GatewayConfig {
        GatewayConfig {
            port: 0,
            bind_address: "127.0.0.1".into(),
            model_name: "test-model".into(),
            session_policy: SessionPolicy::PerConversationId,
        }
    }

    fn test_router(mode: &str) -> Router {
        let store = SessionStore::open_in_memory().unwrap();
        let mut env = HashMap::new();
        env.insert("MOCK_MODE".into(), mode.into());
        env.insert("MOCK_SESSION_ID".into(), "gw-test-session".into());
        let dispatcher = Arc::new(Dispatcher::with_command(store, mock_script(), env));
        build_router(dispatcher, test_config(), 1700000000)
    }

    async fn body_to_string(body: Body) -> String {
        let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_router("normal");
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn models_returns_configured_model() {
        let app = test_router("normal");
        let req = Request::get("/v1/models").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"][0]["id"], "test-model");
        assert_eq!(v["data"][0]["owned_by"], "axios-companion");
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let app = test_router("normal");
        let req = Request::get("/v1/nope").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn empty_messages_returns_400() {
        let app = test_router("normal");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"messages":[]}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 400);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "invalid_messages");
    }

    #[tokio::test]
    async fn no_user_message_returns_400() {
        let app = test_router("normal");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"messages":[{"role":"system","content":"be nice"}]}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 400);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "no_user_message");
    }

    #[tokio::test]
    async fn invalid_json_returns_400() {
        let app = test_router("normal");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 400);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "invalid_json");
    }

    #[tokio::test]
    async fn non_streaming_completion() {
        if !mock_available() { return; }
        let app = test_router("normal");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"messages":[{"role":"user","content":"hello"}],"stream":false}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["model"], "test-model");
        assert_eq!(
            v["choices"][0]["message"]["content"],
            "Hello from mock companion."
        );
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert!(v["id"].as_str().unwrap().starts_with("chatcmpl-"));
    }

    #[tokio::test]
    async fn streaming_completion() {
        if !mock_available() { return; }
        let app = test_router("normal");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"messages":[{"role":"user","content":"hello"}],"stream":true}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let body = body_to_string(resp.into_body()).await;
        // SSE format: "data: <json>\n\n" lines
        let data_lines: Vec<&str> = body
            .lines()
            .filter(|l| l.starts_with("data: "))
            .collect();

        // Should have: chunk1 ("Hello from "), chunk2 ("mock companion."),
        // stop chunk, and [DONE]
        assert!(
            data_lines.len() >= 3,
            "expected at least 3 data lines, got {}: {:?}",
            data_lines.len(),
            data_lines
        );

        // Last data line is [DONE]
        assert_eq!(*data_lines.last().unwrap(), "data: [DONE]");

        // First chunk should have role
        let first_json: serde_json::Value =
            serde_json::from_str(data_lines[0].strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(first_json["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(
            first_json["choices"][0]["delta"]["content"],
            "Hello from "
        );

        // Second chunk should be content-only
        let second_json: serde_json::Value =
            serde_json::from_str(data_lines[1].strip_prefix("data: ").unwrap()).unwrap();
        assert!(second_json["choices"][0]["delta"].get("role").is_none());
        assert_eq!(
            second_json["choices"][0]["delta"]["content"],
            "mock companion."
        );
    }

    #[tokio::test]
    async fn error_turn_returns_500() {
        if !mock_available() { return; }
        let app = test_router("error");
        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"messages":[{"role":"user","content":"fail"}]}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 500);
        let body = body_to_string(resp.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "companion_error");
    }

    #[tokio::test]
    async fn conversation_id_from_header() {
        // This test verifies session creation with a custom conversation ID.
        if !mock_available() { return; }

        let store = SessionStore::open_in_memory().unwrap();
        let mut env = HashMap::new();
        env.insert("MOCK_MODE".into(), "normal".into());
        env.insert("MOCK_SESSION_ID".into(), "header-test".into());
        let dispatcher = Arc::new(Dispatcher::with_command(store, mock_script(), env));
        let app = build_router(dispatcher.clone(), test_config(), 1700000000);

        let req = Request::post("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("x-conversation-id", "kitchen")
            .body(Body::from(
                r#"{"messages":[{"role":"user","content":"hello"}]}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        // Verify the session was created with the right conversation ID.
        let store = dispatcher.store().await;
        let session = store.lookup_session("openai", "kitchen").unwrap();
        assert!(session.is_some(), "session should exist for conversation 'kitchen'");
    }
}
