//! companion-mcp-apps — launch applications and URLs on the desktop.
//!
//! Two thin shell-outs:
//!   `open_url`              → xdg-open <url>
//!   `launch_desktop_entry`  → find <name>.desktop in XDG dirs, then
//!                             dex <path>
//!
//! Using `dex` rather than `gtk-launch` because gtk-launch only ships
//! inside the full gtk3 package (≈30 MB runtime closure for one
//! binary). `dex` is a tiny freedesktop launcher — but it only accepts
//! positional .desktop file paths; its `-a` flag is `--autostart`,
//! not "look up by name." So this tool walks the XDG applications
//! dirs itself to resolve the name to a path before handing it to
//! dex.
//!
//! Both tools are fire-and-forget. The tool returns as soon as the
//! launcher spawns the child; it does not wait for the application
//! to exit, to show a window, or anything else user-visible. The
//! child is detached from our stdio (stdout + stderr → null) so its
//! lifetime can't hold our JSON-RPC pipe open past MCP-server exit —
//! same lesson as wl-copy in the clipboard tool.

use anyhow::Result;
use companion_spoke::{err_text, ok_text, run, tool_def, ToolHandler};
use serde_json::{json, Value};

struct Apps;

impl ToolHandler for Apps {
    fn server_name(&self) -> &'static str {
        "companion-apps"
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            tool_def(
                "open_url",
                "Open a URL in the user's default browser (xdg-open). \
                 Fire-and-forget — returns as soon as the browser is \
                 spawned. Runs on the mcp-gateway host, so the URL opens \
                 on THAT host's display, not the caller's.",
                json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL or file:// path to open."
                        }
                    },
                    "required": ["url"]
                }),
            ),
            tool_def(
                "launch_desktop_entry",
                "Launch a `.desktop` application entry by name. The tool \
                 walks the XDG applications dirs (~/.local/share/applications, \
                 ~/.nix-profile/share/applications, \
                 /etc/profiles/per-user/$USER/share/applications, \
                 /run/current-system/sw/share/applications) to find \
                 `<name>.desktop`, then hands it to dex to execute. \
                 Fire-and-forget. Runs on the mcp-gateway host. On NixOS, \
                 entry names are usually the reverse-DNS app ID \
                 (com.mitchellh.ghostty, org.mozilla.firefox) — not the \
                 bare binary name.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The .desktop entry name, e.g. \"firefox\" or \"com.mitchellh.ghostty\". No .desktop suffix, no path."
                        }
                    },
                    "required": ["name"]
                }),
            ),
        ]
    }

    async fn call(&self, name: &str, args: &Value) -> Value {
        match name {
            "open_url" => open_url(args).await,
            "launch_desktop_entry" => launch_desktop_entry(args).await,
            _ => err_text(format!("unknown tool: {name}")),
        }
    }
}

async fn open_url(args: &Value) -> Value {
    let url = match args.get("url").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return err_text("url is required and must be non-empty"),
    };

    // xdg-open forks and detaches the launched process; inheriting our
    // JSON-RPC stdio would keep the MCP pipe open past our exit. Same
    // bug that bit wl-copy in Phase 3.
    let status = match tokio::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
    {
        Ok(s) => s,
        Err(e) => return err_text(format!("failed to spawn xdg-open: {e}")),
    };

    if !status.success() {
        return err_text(format!(
            "xdg-open exited {}",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        ));
    }

    ok_text(format!("Opened {url}."))
}

async fn launch_desktop_entry(args: &Value) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return err_text("name is required and must be non-empty"),
    };
    let entry = name.strip_suffix(".desktop").unwrap_or(name);

    // dex is a launcher, not a finder. Its `-a` flag means --autostart
    // (run every entry in ~/.config/autostart/), NOT "look up by name"
    // — learned that the hard way when the first live test fired Solaar
    // five times and never touched Ghostty. So: resolve the .desktop
    // path ourselves by walking the XDG applications dirs, then hand
    // the full path to dex as a positional arg.
    let desktop_path = match find_desktop_file(entry) {
        Some(p) => p,
        None => {
            return err_text(format!(
                "no .desktop entry found for \"{entry}\". Searched: {}. \
                 Did you mean a different entry name? Entries on NixOS \
                 are usually the reverse-DNS app ID (com.mitchellh.ghostty, \
                 org.mozilla.firefox) — tab-complete in a file manager \
                 on ~/.local/share/applications/ or check \
                 /etc/profiles/per-user/$USER/share/applications/.",
                application_dirs().join(", ")
            ));
        }
    };

    let status = match tokio::process::Command::new("dex")
        .arg(&desktop_path)
        .env("XDG_DATA_DIRS", nixos_xdg_data_dirs())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
    {
        Ok(s) => s,
        Err(e) => return err_text(format!("failed to spawn dex: {e}")),
    };

    if !status.success() {
        return err_text(format!(
            "dex could not launch \"{entry}\" from {desktop_path} (exit {}). \
             The .desktop file resolved but execution failed.",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        ));
    }

    ok_text(format!("Launched {entry}."))
}

/// Build the list of applications directories to search — same set
/// as [`nixos_xdg_data_dirs`] but each with `/applications` appended,
/// which is where freedesktop says desktop entries live.
fn application_dirs() -> Vec<String> {
    nixos_xdg_data_dirs()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|d| format!("{d}/applications"))
        .collect()
}

/// Look up `<name>.desktop` in the standard applications directories
/// and return the first hit's full path. Mirrors how gtk-launch does
/// its XDG lookup, without needing the 30 MB gtk3 closure.
fn find_desktop_file(name: &str) -> Option<String> {
    let filename = format!("{name}.desktop");
    for dir in application_dirs() {
        let candidate = format!("{dir}/{filename}");
        if std::path::Path::new(&candidate).is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Build an `XDG_DATA_DIRS` value that covers the NixOS canonical
/// locations for desktop entries:
///   - `$HOME/.local/share`              — user-level (XDG spec)
///   - `$HOME/.nix-profile/share`        — per-user nix profile
///   - `/etc/profiles/per-user/$USER/share` — NixOS per-user profile
///                                          (where user-installed
///                                          home-manager packages land)
///   - `/run/current-system/sw/share`    — NixOS system profile
///   - `/usr/local/share:/usr/share`     — freedesktop fallback
///
/// mcp-gateway's systemd unit does not set XDG_DATA_DIRS at all, so
/// dex would otherwise fall back to just `/usr/share:/usr/local/share`
/// — which on NixOS does not exist, and user-installed apps (ghostty,
/// firefox, whatever you `home.packages` into your profile) are
/// invisible. This function is the reason `launch_desktop_entry
/// ghostty` works on a NixOS box at all.
///
/// If the existing env already has a `XDG_DATA_DIRS` set (e.g.,
/// someone runs the tool interactively from a login shell), we prefer
/// that — their session probably knows better than we do. Otherwise
/// we construct the NixOS-style default.
fn nixos_xdg_data_dirs() -> String {
    if let Ok(existing) = std::env::var("XDG_DATA_DIRS") {
        if !existing.is_empty() {
            return existing;
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let user = resolve_username(&home);

    [
        format!("{home}/.local/share"),
        format!("{home}/.nix-profile/share"),
        format!("/etc/profiles/per-user/{user}/share"),
        "/run/current-system/sw/share".to_string(),
        "/usr/local/share".to_string(),
        "/usr/share".to_string(),
    ]
    .join(":")
}

/// Resolve the current user's name without any new crate dependencies.
///
/// `$USER` is reliable inside login shells and user-scope systemd
/// units, but systemd does NOT set `$USER` in Environment= for a
/// system-scope service running with `User=keith` — which is exactly
/// how mcp-gateway runs. Without this fallback, [`nixos_xdg_data_dirs`]
/// would build `/etc/profiles/per-user/root/share`, which doesn't
/// exist, and dex would never see user-installed desktop entries.
///
/// Fallback strategy: derive from `$HOME`'s basename. Linux layout
/// convention is `/home/<user>` for normal accounts and `/root` for
/// root, which covers every case we care about on NixOS. If `$HOME`
/// doesn't look like either, fall back to `"root"` — a bad guess is
/// better than a panic, and the resulting path just harmlessly
/// misses in the applications-dir search.
fn resolve_username(home: &str) -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return user;
        }
    }

    std::path::Path::new(home)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "root".into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run(Apps).await
}
