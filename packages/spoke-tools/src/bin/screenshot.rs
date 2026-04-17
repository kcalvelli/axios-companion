//! companion-mcp-screenshot — capture the Wayland display.
//!
//! Shells out to `grim` to grab pixels, base64-encodes the PNG, and
//! returns it as MCP `ImageContent`. Any Claude Code client that
//! supports multimodal input can then see what's on the screen.
//!
//! Phase 2 scope: `screenshot_full` only. Region and window variants
//! (requiring `slurp` for interactive selection, and `niri msg` for
//! focused-window geometry) are deferred to a follow-up once the
//! multimodal return path is proven.
//!
//! Runs on whichever host the gateway is registered against — with
//! Keith's centralized-mcp-gateway architecture, that's always edge.
//! Screenshots are of edge's display regardless of which host the
//! caller sat in front of. That's a distributed-routing concern, not
//! a spoke-tools concern.

use anyhow::Result;
use base64::Engine;
use companion_spoke::{err_text, ok_image, run, tool_def, ToolHandler};
use serde_json::{json, Value};

struct Screenshot;

impl ToolHandler for Screenshot {
    fn server_name(&self) -> &'static str {
        "companion-screenshot"
    }

    fn tools(&self) -> Vec<Value> {
        vec![tool_def(
            "screenshot_full",
            "Capture the entire Wayland display (all outputs) and return \
             it as a PNG image. Runs on the mcp-gateway host — this is \
             always that host's screen, not the caller's.",
            json!({ "type": "object", "properties": {} }),
        )]
    }

    async fn call(&self, name: &str, _args: &Value) -> Value {
        if name != "screenshot_full" {
            return err_text(format!("unknown tool: {name}"));
        }

        // `grim -` writes PNG to stdout. Capturing there is cleaner
        // than tempfile juggling.
        let output = match tokio::process::Command::new("grim")
            .arg("-")
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => return err_text(format!("failed to spawn grim: {e}")),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return err_text(format!(
                "grim exited {}: {}",
                output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                stderr.trim()
            ));
        }

        if output.stdout.is_empty() {
            return err_text("grim produced no output (Wayland session unavailable?)");
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(&output.stdout);
        ok_image(encoded, "image/png")
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run(Screenshot).await
}
