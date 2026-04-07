mod dbus;

use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt;

use clap::{Parser, Subcommand};
use futures_util::StreamExt;

#[derive(Parser)]
#[command(
    name = "companion",
    version,
    about = "Talk to the companion daemon",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    send: SendArgs,
}

#[derive(clap::Args)]
struct SendArgs {
    /// Message to send (omit for interactive mode, use "-" for stdin)
    prompt: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive chat session
    Chat,
    /// Show daemon status
    Status,
    /// Manage sessions
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// List active conversation surfaces
    Surfaces,
}

#[derive(Subcommand)]
enum SessionAction {
    /// List all sessions
    List,
}

const SURFACE: &str = "cli";

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Some(Command::Chat) => cmd_chat().await,
        Some(Command::Status) => cmd_status().await,
        Some(Command::Sessions { action }) => match action {
            SessionAction::List => cmd_sessions_list().await,
        },
        Some(Command::Surfaces) => cmd_surfaces().await,
        None => {
            let prompt = cli.send.prompt;
            if prompt.is_empty() {
                // No args → full Claude Code session via the shell wrapper.
                exec_raw(&[]);
            } else if prompt.len() == 1 && prompt[0] == "-" {
                cmd_stdin().await
            } else {
                cmd_send(&prompt.join(" ")).await
            }
        }
    };

    std::process::exit(exit_code);
}

/// Exec into the shell wrapper (companion-code) for a full Claude Code session.
/// Does not return on success.
fn exec_raw(extra_args: &[&str]) -> ! {
    let err = std::process::Command::new("companion-code")
        .args(extra_args)
        .exec();
    // exec() only returns on error.
    eprintln!("companion: failed to exec companion-code: {err}");
    eprintln!("companion: is companion-code on your PATH?");
    std::process::exit(1);
}

async fn connect_or_die() -> dbus::CompanionProxy<'static> {
    match dbus::connect().await {
        Ok(proxy) => proxy,
        Err(e) => {
            eprintln!("companion: daemon not reachable: {e}");
            eprintln!(
                "companion: is companion-core running? \
                 (systemctl --user status companion-core)"
            );
            std::process::exit(1);
        }
    }
}

/// Stream a single message and print chunks as they arrive. Returns exit code.
async fn stream_one(
    proxy: &dbus::CompanionProxy<'_>,
    conv_id: &str,
    message: &str,
) -> i32 {
    // Subscribe to signals BEFORE sending so we don't miss early chunks.
    let mut chunks = match proxy.receive_response_chunk().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("companion: signal subscription failed: {e}");
            return 1;
        }
    };
    let mut completions = match proxy.receive_response_complete().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("companion: signal subscription failed: {e}");
            return 1;
        }
    };
    let mut errors = match proxy.receive_response_error().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("companion: signal subscription failed: {e}");
            return 1;
        }
    };

    if let Err(e) = proxy.stream_message(SURFACE, conv_id, message).await {
        eprintln!("companion: send failed: {e}");
        return 1;
    }

    let mut stdout = io::stdout().lock();
    loop {
        tokio::select! {
            Some(signal) = chunks.next() => {
                if let Ok(args) = signal.args() {
                    if args.surface == SURFACE && args.conversation_id == conv_id {
                        let _ = write!(stdout, "{}", args.chunk);
                        let _ = stdout.flush();
                    }
                }
            }
            Some(signal) = completions.next() => {
                if let Ok(args) = signal.args() {
                    if args.surface == SURFACE && args.conversation_id == conv_id {
                        let _ = writeln!(stdout);
                        return 0;
                    }
                }
            }
            Some(signal) = errors.next() => {
                if let Ok(args) = signal.args() {
                    if args.surface == SURFACE && args.conversation_id == conv_id {
                        let _ = writeln!(stdout);
                        eprintln!("companion: error: {}", args.error);
                        return 1;
                    }
                }
            }
        }
    }
}

async fn cmd_send(message: &str) -> i32 {
    let proxy = connect_or_die().await;
    let conv_id = conversation_id();
    stream_one(&proxy, &conv_id, message).await
}

async fn cmd_chat() -> i32 {
    let proxy = connect_or_die().await;
    let conv_id = conversation_id();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        let _ = write!(stdout, "you> ");
        let _ = stdout.flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => return 0, // EOF
            Err(e) => {
                eprintln!("companion: read error: {e}");
                return 1;
            }
            _ => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "/quit" || trimmed == "/exit" {
            return 0;
        }

        let _ = write!(stdout, "sid> ");
        let _ = stdout.flush();

        let code = stream_one(&proxy, &conv_id, trimmed).await;
        if code != 0 {
            return code;
        }
    }
}

async fn cmd_stdin() -> i32 {
    let stdin = io::stdin();
    let mut message = String::new();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) => {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(&l);
            }
            Err(e) => {
                eprintln!("companion: stdin read error: {e}");
                return 1;
            }
        }
    }

    if message.trim().is_empty() {
        eprintln!("companion: empty input on stdin");
        return 1;
    }

    cmd_send(&message).await
}

async fn cmd_status() -> i32 {
    let proxy = connect_or_die().await;

    match proxy.get_status().await {
        Ok(status) => {
            let version = status
                .get("version")
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or("unknown");
            let uptime = status
                .get("uptime_seconds")
                .and_then(|v| <u32>::try_from(v).ok())
                .unwrap_or(0);
            let active = status
                .get("active_sessions")
                .and_then(|v| <u32>::try_from(v).ok())
                .unwrap_or(0);
            let in_flight = status
                .get("in_flight_turns")
                .and_then(|v| <u32>::try_from(v).ok())
                .unwrap_or(0);

            println!("companion-core v{version}");
            println!("  uptime:          {}", format_duration(uptime));
            println!("  active sessions: {active}");
            println!("  in-flight turns: {in_flight}");
            0
        }
        Err(e) => {
            eprintln!("companion: failed to get status: {e}");
            1
        }
    }
}

async fn cmd_sessions_list() -> i32 {
    let proxy = connect_or_die().await;

    match proxy.list_sessions().await {
        Ok(sessions) => {
            if sessions.is_empty() {
                println!("No sessions.");
                return 0;
            }
            println!(
                "{:<12} {:<16} {:<24} {:<10} {}",
                "SURFACE", "CONVERSATION", "CLAUDE SESSION", "STATUS", "LAST ACTIVE"
            );
            for (surface, conv_id, claude_id, status, last_active) in sessions {
                let claude_display = if claude_id.is_empty() {
                    "-".into()
                } else {
                    truncate(&claude_id, 22)
                };
                println!(
                    "{:<12} {:<16} {:<24} {:<10} {}",
                    surface,
                    truncate(&conv_id, 14),
                    claude_display,
                    status,
                    format_timestamp(last_active),
                );
            }
            0
        }
        Err(e) => {
            eprintln!("companion: failed to list sessions: {e}");
            1
        }
    }
}

async fn cmd_surfaces() -> i32 {
    let proxy = connect_or_die().await;

    match proxy.get_active_surfaces().await {
        Ok(surfaces) => {
            if surfaces.is_empty() {
                println!("No active surfaces.");
            } else {
                for s in surfaces {
                    println!("{s}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("companion: failed to list surfaces: {e}");
            1
        }
    }
}

/// Stable conversation ID for this terminal session. Reuses an existing
/// ID from the environment so daemon-side sessions survive CLI restarts.
fn conversation_id() -> String {
    std::env::var("COMPANION_CONVERSATION_ID").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
}

fn format_duration(seconds: u32) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn format_timestamp(unix: u32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    if unix == 0 {
        return "-".into();
    }

    let ago = now.saturating_sub(unix);
    if ago < 60 {
        "just now".into()
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else if ago < 86400 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86400)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
