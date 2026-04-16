//! Shared JSON-RPC MCP shell for cairn-companion's spoke tool binaries.
//!
//! Every tool binary under `src/bin/` calls [`serve`] with its own
//! `ToolHandler` implementation. The shell reads line-delimited
//! JSON-RPC messages from stdin, dispatches `initialize` / `tools/list`
//! / `tools/call` to the handler, and writes responses to stdout.
//!
//! No MCP SDK crate — patterned after sentinel-mcp's hand-rolled
//! implementation. Every tool binary is ~30 lines on top of this.

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What every tool binary must provide. Stateless by design — the
/// binary is respawned per-call by mcp-gateway.
pub trait ToolHandler: Send + Sync {
    /// `serverInfo.name` returned from `initialize`.
    fn server_name(&self) -> &'static str;

    /// `serverInfo.version` returned from `initialize`.
    fn server_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Tool descriptors returned from `tools/list`. Build entries with
    /// [`tool_def`].
    fn tools(&self) -> Vec<Value>;

    /// Dispatch one `tools/call` invocation. Return the full result
    /// body (`{ content: [...], isError?: bool }`) — build it with
    /// [`ok_text`], [`ok_image`], or [`err_text`].
    ///
    /// Desugared from `async fn` so the returned future is explicitly
    /// `Send`. The shell loop is single-threaded today but this keeps
    /// us free to spawn into a multi-threaded runtime later without
    /// breaking every existing tool binary.
    fn call(
        &self,
        name: &str,
        arguments: &Value,
    ) -> impl std::future::Future<Output = Value> + Send;
}

/// Run the stdio MCP server loop until stdin closes.
pub async fn serve<H: ToolHandler>(handler: H) -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut lines = stdin.lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = jsonrpc_error(Value::Null, -32700, &format!("parse error: {e}"));
                write_response(&mut stdout, &err).await?;
                continue;
            }
        };

        let response = handle_request(&handler, &request).await;
        if !response.is_null() {
            write_response(&mut stdout, &response).await?;
        }
    }

    Ok(())
}

async fn handle_request<H: ToolHandler>(handler: &H, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    match method {
        "initialize" => jsonrpc_result(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": handler.server_name(),
                    "version": handler.server_version(),
                },
            }),
        ),
        // Notifications have no response.
        "notifications/initialized" | "notifications/cancelled" => Value::Null,
        "ping" => jsonrpc_result(id, json!({})),
        "tools/list" => jsonrpc_result(id, json!({ "tools": handler.tools() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(json!({}));
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let body = handler.call(&name, &args).await;
            jsonrpc_result(id, body)
        }
        _ => jsonrpc_error(id, -32601, &format!("method not found: {method}")),
    }
}

async fn write_response<W: AsyncWriteExt + Unpin>(w: &mut W, value: &Value) -> Result<()> {
    let mut s = serde_json::to_string(value)?;
    s.push('\n');
    w.write_all(s.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

pub fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

pub fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

// ---------------------------------------------------------------------------
// Tool-descriptor and response-body helpers
// ---------------------------------------------------------------------------

/// Build one entry for [`ToolHandler::tools`].
pub fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

/// Successful text response body for [`ToolHandler::call`].
pub fn ok_text(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
    })
}

/// Successful image response body for [`ToolHandler::call`].
pub fn ok_image(base64_data: impl Into<String>, mime_type: &str) -> Value {
    json!({
        "content": [{
            "type": "image",
            "data": base64_data.into(),
            "mimeType": mime_type,
        }],
    })
}

/// Tool-level error response. MCP convention: protocol errors use the
/// JSON-RPC `error` channel, tool-level failures (bad args, command
/// not found, allowlist rejection) use `isError: true` on the result.
pub fn err_text(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHandler;

    impl ToolHandler for FakeHandler {
        fn server_name(&self) -> &'static str {
            "fake"
        }
        fn tools(&self) -> Vec<Value> {
            vec![tool_def(
                "echo",
                "Echo the input",
                json!({ "type": "object", "properties": { "msg": { "type": "string" } } }),
            )]
        }
        async fn call(&self, name: &str, args: &Value) -> Value {
            if name == "echo" {
                ok_text(args.get("msg").and_then(|v| v.as_str()).unwrap_or(""))
            } else {
                err_text(format!("unknown tool: {name}"))
            }
        }
    }

    #[tokio::test]
    async fn initialize_reports_server_info() {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let resp = handle_request(&FakeHandler, &req).await;
        assert_eq!(resp["result"]["serverInfo"]["name"], "fake");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn tools_list_returns_handler_tools() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle_request(&FakeHandler, &req).await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
    }

    #[tokio::test]
    async fn tools_call_dispatches_to_handler() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "echo", "arguments": { "msg": "hi" } }
        });
        let resp = handle_request(&FakeHandler, &req).await;
        let content = resp["result"]["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], "hi");
        // Success → no isError.
        assert!(resp["result"].get("isError").is_none());
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_is_error() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "nope", "arguments": {} }
        });
        let resp = handle_request(&FakeHandler, &req).await;
        assert_eq!(resp["result"]["isError"], true);
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let req = json!({ "jsonrpc": "2.0", "id": 4, "method": "nope" });
        let resp = handle_request(&FakeHandler, &req).await;
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn initialized_notification_has_no_response() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let resp = handle_request(&FakeHandler, &req).await;
        assert!(resp.is_null());
    }
}
