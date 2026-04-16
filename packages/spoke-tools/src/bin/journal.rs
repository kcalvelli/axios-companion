//! companion-mcp-journal — read the user's systemd journal.
//!
//! Thin wrapper around `journalctl --user`. Read-only. Sid can grep
//! logs, pull recent events from a specific unit, or look back to a
//! rough "since" window ("10 minutes ago", "1 hour ago", or any
//! systemd-accepted --since value).
//!
//! Runs on the mcp-gateway host — so `unit=companion-core` reads the
//! gateway host's companion-core journal, not the caller's. Same
//! central-gateway caveat as every other spoke tool at this tier.

use anyhow::Result;
use companion_spoke::{err_text, ok_text, serve, tool_def, ToolHandler};
use serde_json::{json, Value};

const DEFAULT_LINES: u32 = 100;
const MAX_LINES: u32 = 1000;

struct Journal;

impl ToolHandler for Journal {
    fn server_name(&self) -> &'static str {
        "companion-journal"
    }

    fn tools(&self) -> Vec<Value> {
        vec![tool_def(
            "journal_read",
            "Read lines from the user's systemd journal (journalctl --user). \
             Optional filters: `unit` (a user service name, e.g. \
             `companion-core` or `mcp-gateway`), `since` (any value \
             journalctl accepts: `10 minutes ago`, `1 hour ago`, `today`, \
             an ISO timestamp, etc.), and `lines` (default 100, max 1000). \
             Returns newest-first.",
            json!({
                "type": "object",
                "properties": {
                    "unit": {
                        "type": "string",
                        "description": "User service name to filter on, e.g. companion-core."
                    },
                    "since": {
                        "type": "string",
                        "description": "How far back to look. Any journalctl --since value."
                    },
                    "lines": {
                        "type": "integer",
                        "description": "Max lines to return (default 100, max 1000).",
                        "minimum": 1,
                        "maximum": MAX_LINES
                    }
                }
            }),
        )]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        if name != "journal_read" {
            return err_text(format!("unknown tool: {name}"));
        }

        let requested = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LINES as u64) as u32;
        let lines = requested.clamp(1, MAX_LINES);

        let mut cmd = tokio::process::Command::new("journalctl");
        cmd.args(["--user", "--no-pager", "--output=short", "-n", &lines.to_string()]);

        if let Some(unit) = args.get("unit").and_then(|v| v.as_str()) {
            cmd.args(["-u", unit]);
        }
        if let Some(since) = args.get("since").and_then(|v| v.as_str()) {
            cmd.args(["--since", since]);
        }

        // Don't inherit our JSON-RPC stdout/stderr — same class of bug
        // as the clipboard wl-copy fork. journalctl doesn't fork a
        // daemon, but we capture its output via .output() below so
        // the stdio direction is moot either way; explicit is safer.
        let output = match cmd.output().await {
            Ok(o) => o,
            Err(e) => return err_text(format!("failed to spawn journalctl: {e}")),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return err_text(format!(
                "journalctl exited {}: {}",
                output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                stderr.trim()
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        if text.trim().is_empty() {
            ok_text("(no matching journal lines)")
        } else {
            ok_text(text)
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    serve(Journal).await
}
