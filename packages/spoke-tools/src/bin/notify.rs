//! companion-mcp-notify — desktop notifications via libnotify.
//!
//! Shells out to `notify-send`. Any freedesktop-compliant notification
//! daemon on the user's session (mako, DankMaterialShell, etc.) picks
//! it up — cairn-companion does not care which.

use anyhow::Result;
use companion_spoke::{err_text, ok_text, run, tool_def, ToolHandler};
use serde_json::{json, Value};

struct Notify;

impl ToolHandler for Notify {
    fn server_name(&self) -> &'static str {
        "companion-notify"
    }

    fn tools(&self) -> Vec<Value> {
        vec![tool_def(
            "notify",
            "Show a desktop notification on the user's active Wayland session. \
             The notification is displayed by whichever freedesktop-compliant \
             notification daemon is running (mako, DankMaterialShell, etc.). \
             Fire-and-forget — the tool returns as soon as the notification \
             is enqueued; it does not wait for the user to dismiss it.",
            json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Title of the notification (required, keep short)."
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional body text below the summary."
                    },
                    "urgency": {
                        "type": "string",
                        "enum": ["low", "normal", "critical"],
                        "description": "Urgency level. Defaults to normal."
                    }
                },
                "required": ["summary"]
            }),
        )]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        if name != "notify" {
            return err_text(format!("unknown tool: {name}"));
        }

        let summary = match args.get("summary").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => return err_text("summary is required and must be non-empty"),
        };
        let body = args.get("body").and_then(|v| v.as_str());
        let urgency = args.get("urgency").and_then(|v| v.as_str()).unwrap_or("normal");

        // Validate urgency rather than trusting notify-send to reject it —
        // notify-send happily accepts garbage urgency values and warns to
        // stderr, which the MCP client never sees.
        if !matches!(urgency, "low" | "normal" | "critical") {
            return err_text(format!(
                "urgency must be one of low|normal|critical, got: {urgency}"
            ));
        }

        let mut cmd = tokio::process::Command::new("notify-send");
        cmd.arg("--app-name=sid")
            .arg("--urgency")
            .arg(urgency)
            .arg(summary);
        if let Some(b) = body {
            cmd.arg(b);
        }

        let status = match cmd.status().await {
            Ok(s) => s,
            Err(e) => return err_text(format!("failed to run notify-send: {e}")),
        };

        if !status.success() {
            return err_text(format!(
                "notify-send exited with {}",
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
            ));
        }

        ok_text(format!("Notification \"{summary}\" shown."))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run(Notify).await
}
