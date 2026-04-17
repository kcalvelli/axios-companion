//! companion-mcp-shell — allowlisted shell execution.
//!
//! **This is the highest-blast-radius spoke tool.** Every invocation
//! is audit-logged to the user journal. The allowlist is an explicit
//! operator decision, not a default — an empty allowlist means deny
//! everything, which is the right default for a tool that could, if
//! misused, do arbitrary damage.
//!
//! Guarantees:
//!   - argv is passed directly to tokio::process::Command. No shell
//!     wrapping, no string interpolation, no injection vector via
//!     command arguments.
//!   - argv[0] is checked against the allowlist *by basename*. A
//!     caller can't sneak `/usr/bin/evil-rm-rf` past an allowlist of
//!     `["rm"]` by giving the full path — and can't sneak
//!     `rm` past an allowlist of `["/sbin/rm"]` either. Basename
//!     matching is the only sane contract.
//!   - stdin is passed through a pipe as bytes. No shell.
//!   - `timeout_secs` is enforced in-process via
//!     `tokio::time::timeout`. A hung subprocess can't starve the
//!     MCP server.
//!   - Every invocation logs one structured event at INFO level:
//!     argv, allow/deny, exit code, duration. journalctl --user -t
//!     companion-mcp-shell shows the full audit trail.
//!
//! Configuration, via env (set by home-manager):
//!   COMPANION_SHELL_ALLOWLIST="git,ls,cat,grep"   comma-separated
//!   COMPANION_SHELL_ALLOWLIST="*"                 allow-all (LOUD)
//!   (unset or empty)                              deny-all
//!
//! The home-manager option takes a Nix list and marshals it into
//! the env string at build time. See
//! modules/home-manager/default.nix.

use anyhow::Result;
use companion_spoke::{err_text, ok_text, run as serve, tool_def, ToolHandler};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;
use tracing::{info, warn};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;

struct Shell {
    allowlist: Allowlist,
}

/// Parsed allowlist state. Three modes explicit in the type so every
/// call site has to acknowledge them.
#[derive(Debug, Clone)]
enum Allowlist {
    /// Deny every command. This is what you get when
    /// COMPANION_SHELL_ALLOWLIST is unset or empty — the safe default.
    DenyAll,
    /// Allow every command. Triggered by setting the allowlist to
    /// just `*`. Every call is audit-logged at WARN.
    AllowAll,
    /// Allow only commands whose basename is in this set.
    Specific(HashSet<String>),
}

impl Allowlist {
    fn from_env() -> Self {
        let raw = std::env::var("COMPANION_SHELL_ALLOWLIST").unwrap_or_default();
        let items: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        if items.is_empty() {
            return Self::DenyAll;
        }
        if items == ["*"] {
            return Self::AllowAll;
        }
        Self::Specific(items.into_iter().map(String::from).collect())
    }

    fn permits(&self, command_name: &str) -> bool {
        match self {
            Self::DenyAll => false,
            Self::AllowAll => true,
            Self::Specific(set) => set.contains(command_name),
        }
    }
}

impl ToolHandler for Shell {
    fn server_name(&self) -> &'static str {
        "companion-shell"
    }

    fn tools(&self) -> Vec<Value> {
        vec![tool_def(
            "run",
            "Run a shell command on the mcp-gateway host. The command is \
             passed as an argv array — argv[0] is the program, the rest \
             are its arguments. No shell interpretation: pipes, globs, \
             redirects, and $variables are NOT expanded. \
             \
             argv[0] is checked against the operator-configured allowlist \
             by basename. Commands not on the allowlist are rejected \
             before execution with a clear error. \
             \
             Every invocation is audit-logged to the user journal — \
             journalctl --user -t companion-mcp-shell shows the full \
             history. Honor that: if something would feel bad to have \
             audit-logged, don't run it. \
             \
             Returns combined stdout/stderr plus exit code. A non-zero \
             exit code is reported but NOT treated as a tool error — \
             the tool succeeded in running the command; what the command \
             did is the caller's concern.",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "argv list. First element is the program; rest are its arguments.",
                        "minItems": 1
                    },
                    "stdin": {
                        "type": "string",
                        "description": "Optional text to pipe to the command's stdin."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Kill the command after this many seconds (default 30, max 300).",
                        "minimum": 1,
                        "maximum": MAX_TIMEOUT_SECS
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the command. Defaults to the daemon's CWD."
                    }
                },
                "required": ["command"]
            }),
        )]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        if name != "run" {
            return err_text(format!("unknown tool: {name}"));
        }
        run(&self.allowlist, args).await
    }
}

async fn run(allowlist: &Allowlist, args: &Value) -> Value {
    let command: Vec<String> = match args.get("command").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => return err_text("command is required and must be a non-empty array of strings"),
    };
    if command.is_empty() {
        return err_text("command must contain at least one string");
    }

    let program = &command[0];
    let basename = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
        .to_string();

    // Allowlist check. This is the load-bearing safety line — every
    // change here is a security-review change.
    if !allowlist.permits(&basename) {
        warn!(
            target: "companion-mcp-shell",
            argv = ?command,
            basename = %basename,
            allowlist = ?allowlist,
            "shell command REJECTED by allowlist"
        );
        return err_text(format!(
            "command \"{basename}\" is not on the allowlist. Allowlist is \
             configured by the operator via \
             services.cairn-companion.spoke.tools.shell.allowlist in \
             home-manager; nothing the tool caller does at runtime can \
             change it."
        ));
    }

    if matches!(allowlist, Allowlist::AllowAll) {
        warn!(
            target: "companion-mcp-shell",
            argv = ?command,
            "shell allowlist is AllowAll (`*`) — every call audit-logged"
        );
    }

    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let stdin_text = args.get("stdin").and_then(|v| v.as_str()).map(String::from);
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(String::from);

    let start = Instant::now();

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&command[1..]);
    if let Some(ref d) = cwd {
        cmd.current_dir(d);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            info!(
                target: "companion-mcp-shell",
                argv = ?command,
                "spawn failed: {e}"
            );
            // ENOENT is the common failure mode — binary isn't on the
            // mcp-gateway process's PATH. Raw error text is "No such
            // file or directory", which gets confused with "command
            // not allowlisted" by callers who aren't reading carefully.
            // Surface the distinction explicitly.
            let msg = if e.raw_os_error() == Some(2) {
                format!(
                    "binary \"{basename}\" is on the allowlist but not \
                     resolvable on the mcp-gateway process's PATH. \
                     Either pass the full path (e.g. \
                     `[\"/run/current-system/sw/bin/{basename}\", ...]`) \
                     or ask Keith to add the binary's containing \
                     package to the shell tool's wrapped PATH in \
                     spoke-tools/default.nix. This is NOT an allowlist \
                     rejection — the allowlist permits it."
                )
            } else {
                format!("failed to spawn {basename}: {e}")
            };
            return err_text(msg);
        }
    };

    if let Some(text) = &stdin_text {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(text.as_bytes()).await {
                let _ = child.kill().await;
                return err_text(format!("failed to write stdin: {e}"));
            }
            drop(stdin);
        }
    }

    let wait_fut = child.wait_with_output();
    let output = match timeout(Duration::from_secs(timeout_secs), wait_fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            info!(
                target: "companion-mcp-shell",
                argv = ?command,
                "wait failed: {e}"
            );
            return err_text(format!("waiting for {basename} failed: {e}"));
        }
        Err(_) => {
            info!(
                target: "companion-mcp-shell",
                argv = ?command,
                timeout_secs = timeout_secs,
                "command TIMED OUT"
            );
            return err_text(format!(
                "{basename} did not exit within {timeout_secs}s and was killed"
            ));
        }
    };

    let elapsed = start.elapsed();
    let exit_code = output.status.code();

    info!(
        target: "companion-mcp-shell",
        argv = ?command,
        exit = ?exit_code,
        duration_ms = elapsed.as_millis() as u64,
        "shell command completed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_display = exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());

    let mut body = format!("exit: {exit_display}\n");
    if !stdout.is_empty() {
        body.push_str("--- stdout ---\n");
        body.push_str(&stdout);
        if !stdout.ends_with('\n') {
            body.push('\n');
        }
    }
    if !stderr.is_empty() {
        body.push_str("--- stderr ---\n");
        body.push_str(&stderr);
        if !stderr.ends_with('\n') {
            body.push('\n');
        }
    }
    if stdout.is_empty() && stderr.is_empty() {
        body.push_str("(no output)\n");
    }

    ok_text(body)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Hook tracing-journald so audit events land in the user journal
    // under the "companion-mcp-shell" identifier. journalctl --user
    // -t companion-mcp-shell is the operator's audit log.
    //
    // If journald isn't reachable (running outside systemd), fall
    // back silently — the tool still works, just without audit.
    if let Ok(layer) = tracing_journald::layer() {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry()
            .with(layer)
            .with(
                tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    }

    let allowlist = Allowlist::from_env();
    info!(
        target: "companion-mcp-shell",
        allowlist = ?allowlist,
        "shell tool starting"
    );

    serve(Shell { allowlist }).await
}
