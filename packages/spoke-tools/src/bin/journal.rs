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
        vec![
            tool_def(
                "journal_read",
                "Read lines from the user's systemd journal (journalctl --user). \
                 Optional filters: `unit` (a user service name, e.g. \
                 `companion-core` or `mcp-gateway` — call `journal_list_units` \
                 first if you don't know the exact name), `since` (any value \
                 journalctl accepts: `10 minutes ago`, `1 hour ago`, `today`, \
                 an ISO timestamp, etc.), and `lines` (default 100, max 1000). \
                 Returns newest-first.",
                json!({
                    "type": "object",
                    "properties": {
                        "unit": {
                            "type": "string",
                            "description": "Exact user service name to filter on. \
                                            No .service suffix. Call journal_list_units \
                                            to discover names."
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
            ),
            tool_def(
                "journal_list_units",
                "List the user's systemd services (names only). Use this to \
                 discover the exact unit name to pass to `journal_read`.",
                json!({ "type": "object", "properties": {} }),
            ),
        ]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        match name {
            "journal_read" => read_journal(args).await,
            "journal_list_units" => list_units().await,
            _ => err_text(format!("unknown tool: {name}")),
        }
    }
}

async fn read_journal(args: &Value) -> Value {
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
    // journalctl emits the literal string "-- No entries --" to stdout
    // when the filter matches nothing. That string isn't empty so our
    // plain is_empty check doesn't catch it and Claude gets "-- No
    // entries --" verbatim. Normalize both cases to one actionable
    // message that hints at the common cause (wrong unit name).
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "-- No entries --" {
        ok_text(
            "(no matching journal lines — if filtering by unit, \
             call journal_list_units to check the exact name)",
        )
    } else {
        ok_text(text)
    }
}

async fn list_units() -> Value {
    // Names only — keeps the response compact and avoids surfacing
    // enable/active state that Sid probably doesn't need for the
    // common "what's this unit called?" lookup.
    let output = match tokio::process::Command::new("systemctl")
        .args([
            "--user",
            "list-unit-files",
            "--type=service",
            "--no-pager",
            "--no-legend",
            "--state=enabled,static,linked",
        ])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => return err_text(format!("failed to spawn systemctl: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return err_text(format!("systemctl exited non-zero: {}", stderr.trim()));
    }

    // Output is "<unit>.service <state> <preset>" per line. Strip to
    // the bare unit name so the result plugs straight into
    // journal_read's unit argument.
    let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(|name| name.strip_suffix(".service").unwrap_or(name).to_string())
        .collect();

    if names.is_empty() {
        ok_text("(no user services)")
    } else {
        ok_text(names.join("\n"))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    serve(Journal).await
}
