//! companion-mcp-clipboard — read and write the Wayland clipboard.
//!
//! Shells out to `wl-paste` / `wl-copy` from the wl-clipboard package.
//! Text-only for this phase — clipboard can carry binary blobs
//! (images, files) but every actual Sid request ends up being "grab
//! what I just copied" or "put this on my clipboard."

use anyhow::Result;
use companion_spoke::{err_text, ok_text, run, tool_def, ToolHandler};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

struct Clipboard;

impl ToolHandler for Clipboard {
    fn server_name(&self) -> &'static str {
        "companion-clipboard"
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            tool_def(
                "clipboard_read",
                "Read the current text contents of the Wayland clipboard. \
                 Returns empty text if the clipboard is empty or holds a \
                 non-text payload.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool_def(
                "clipboard_write",
                "Write text to the Wayland clipboard. Replaces whatever \
                 was there.",
                json!({
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "The text to place on the clipboard."
                        }
                    },
                    "required": ["text"]
                }),
            ),
        ]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        match name {
            "clipboard_read" => read().await,
            "clipboard_write" => write(args).await,
            _ => err_text(format!("unknown tool: {name}")),
        }
    }
}

async fn read() -> Value {
    // `-n` strips the trailing newline wl-paste adds by default.
    let output = match tokio::process::Command::new("wl-paste")
        .arg("-n")
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => return err_text(format!("failed to spawn wl-paste: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // wl-paste exits non-zero when the clipboard is empty or holds
        // only a non-text payload. Surface that as an empty read, not
        // an error — "nothing to read" is a legitimate state.
        if stderr.contains("No selection") || stderr.contains("No suitable type") {
            return ok_text("");
        }
        return err_text(format!(
            "wl-paste exited {}: {}",
            output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            stderr.trim()
        ));
    }

    match String::from_utf8(output.stdout) {
        Ok(s) => ok_text(s),
        Err(_) => err_text("clipboard contains non-UTF8 data"),
    }
}

async fn write(args: &Value) -> Value {
    let text = match args.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return err_text("text is required"),
    };

    // wl-copy forks a daemon that holds the selection in the
    // background. By default that child inherits our stdout/stderr,
    // which are the MCP server's JSON-RPC pipe — the daemon keeps
    // the write end open past our exit and any downstream reader
    // (mcp-gateway, or a test harness) never sees EOF. Explicit null
    // redirection cuts the child off from our pipe.
    let mut child = match tokio::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return err_text(format!("failed to spawn wl-copy: {e}")),
    };

    // Pipe via stdin so newlines / binary-ish text pass through unharmed.
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(text.as_bytes()).await {
            return err_text(format!("failed to write to wl-copy stdin: {e}"));
        }
        // Dropping stdin closes the pipe, signaling EOF to wl-copy.
        drop(stdin);
    } else {
        return err_text("wl-copy did not expose stdin");
    }

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => return err_text(format!("wl-copy wait failed: {e}")),
    };

    if !status.success() {
        return err_text(format!(
            "wl-copy exited {}",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        ));
    }

    ok_text(format!("Copied {} bytes to the clipboard.", text.len()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run(Clipboard).await
}
