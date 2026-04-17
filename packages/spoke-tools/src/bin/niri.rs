//! companion-mcp-niri — control the Niri compositor.
//!
//! Thin wrapper around `niri msg`. Read-side tools return Niri's
//! native JSON output so Claude can reason about it directly; write-
//! side tools shell out to `niri msg action <thing>` and return a
//! terse confirmation.
//!
//! Scoped starter set for Phase 6:
//!   niri_windows             — list all windows
//!   niri_workspaces          — list all workspaces
//!   niri_focused_window      — which window has focus right now
//!   niri_focus_window        — focus a window by numeric id
//!   niri_focus_workspace     — focus a workspace by index or name
//!   niri_close_focused       — close the currently focused window
//!   niri_spawn               — spawn a command in the compositor
//!
//! Out of scope for this phase: the full `niri msg action` catalog
//! (there are ~80 actions). Easy to add by appending to tools() and
//! call() once a real use case shows up.

use anyhow::Result;
use companion_spoke::{err_text, ok_text, run, tool_def, ToolHandler};
use serde_json::{json, Value};

struct Niri;

impl ToolHandler for Niri {
    fn server_name(&self) -> &'static str {
        "companion-niri"
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            tool_def(
                "niri_windows",
                "List all open windows in the running Niri compositor. \
                 Returns Niri's native JSON — an array of objects with \
                 id, app_id, title, workspace_id, is_focused, etc.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool_def(
                "niri_workspaces",
                "List all workspaces in the running Niri compositor. \
                 Returns Niri's native JSON — an array of objects with \
                 id, idx, name, output, is_focused, is_active, etc.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool_def(
                "niri_focused_window",
                "Return information about the currently focused window, \
                 or null if no window has focus. Niri's native JSON.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool_def(
                "niri_focus_window",
                "Focus a window by its numeric `id` (as returned by \
                 `niri_windows`). The window comes to the front of its \
                 column / workspace.",
                json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": "Numeric window id from niri_windows.",
                            "minimum": 0
                        }
                    },
                    "required": ["id"]
                }),
            ),
            tool_def(
                "niri_focus_workspace",
                "Focus a workspace by index (1-based integer) or name \
                 (string). Niri accepts either — pass whichever the \
                 user referred to.",
                json!({
                    "type": "object",
                    "properties": {
                        "reference": {
                            "type": "string",
                            "description": "Workspace index (e.g. \"2\") or name (e.g. \"chat\")."
                        }
                    },
                    "required": ["reference"]
                }),
            ),
            tool_def(
                "niri_close_focused",
                "Close the currently focused window. Niri's close-window \
                 action; whether the app prompts for unsaved changes is \
                 up to the app itself.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool_def(
                "niri_spawn",
                "Spawn a command via the compositor (niri msg action \
                 spawn). Unlike the `apps` tool's open_url / \
                 launch_desktop_entry which shell out to xdg-open / dex, \
                 this runs a command directly — useful for terminal \
                 invocations that don't have a .desktop entry, or for \
                 launching with specific arguments.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "argv list: [\"firefox\", \"--new-window\", \"https://nixos.org\"]",
                            "minItems": 1
                        }
                    },
                    "required": ["command"]
                }),
            ),
        ]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        match name {
            "niri_windows" => niri_json(&["windows"]).await,
            "niri_workspaces" => niri_json(&["workspaces"]).await,
            "niri_focused_window" => niri_json(&["focused-window"]).await,
            "niri_focus_window" => focus_window(args).await,
            "niri_focus_workspace" => focus_workspace(args).await,
            "niri_close_focused" => niri_action(&["close-window"]).await,
            "niri_spawn" => spawn(args).await,
            _ => err_text(format!("unknown tool: {name}")),
        }
    }
}

/// Invoke `niri msg --json <args...>` and return the JSON output as
/// text (Claude can parse it — we don't need to deserialize and
/// re-serialize). Non-zero exit or failed spawn maps to err_text.
async fn niri_json(args: &[&str]) -> Value {
    let mut cmd = tokio::process::Command::new("niri");
    cmd.arg("msg").arg("--json").args(args);

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => return err_text(format!("failed to spawn niri: {e}")),
    };

    if !output.status.success() {
        return err_text(niri_error_text(&output));
    }

    ok_text(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Invoke `niri msg action <args...>` and return a terse confirmation.
/// Used by the write-side tools (focus, close, spawn).
async fn niri_action(args: &[&str]) -> Value {
    let mut cmd = tokio::process::Command::new("niri");
    cmd.arg("msg").arg("action").args(args);

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => return err_text(format!("failed to spawn niri: {e}")),
    };

    if !output.status.success() {
        return err_text(niri_error_text(&output));
    }

    ok_text(format!("OK ({}).", args.join(" ")))
}

async fn focus_window(args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_u64()) {
        Some(n) => n.to_string(),
        None => return err_text("id is required and must be a non-negative integer"),
    };
    niri_action(&["focus-window", "--id", &id]).await
}

async fn focus_workspace(args: &Value) -> Value {
    let reference = match args.get("reference").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return err_text("reference is required and must be non-empty"),
    };
    niri_action(&["focus-workspace", reference]).await
}

async fn spawn(args: &Value) -> Value {
    let command: Vec<String> = match args.get("command").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => return err_text("command is required and must be a non-empty array of strings"),
    };
    if command.is_empty() {
        return err_text("command must contain at least one string");
    }

    // `niri msg action spawn -- <cmd> <args...>`
    let mut full_args = vec!["spawn".to_string(), "--".to_string()];
    full_args.extend(command.iter().cloned());
    let refs: Vec<&str> = full_args.iter().map(String::as_str).collect();
    niri_action(&refs).await
}

fn niri_error_text(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    format!("niri msg exited {}: {}", code, stderr.trim())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run(Niri).await
}
