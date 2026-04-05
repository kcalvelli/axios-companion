# Proposal: Spoke Tools — Machine-Local MCP Tool Servers

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 2

## Summary

Ship a set of MCP tool servers that expose the local machine's capabilities — shell execution, app launching, screenshot capture, clipboard access, desktop notifications, journal reading, and Niri compositor control — as MCP tools consumable by Claude Code via mcp-gateway. Each tool server is a small Rust binary registered with mcp-gateway through a home-manager module. Together, these servers turn every axios-companion-enabled machine into a tool surface that the Tier 2 hub can route actions to.

## Motivation

For the companion to "live on the desktop you're currently using" (the core Tier 2 promise), the hub daemon needs to be able to execute actions on whichever machine the user is at — open their browser, launch apps, take screenshots, read their user journal, manipulate windows in their compositor. The cleanest way to do this is via MCP: every machine exposes its local capabilities as MCP servers registered in mcp-gateway, and the hub connects to each machine's gateway over Tailscale (which handles network trust, per the `mcp-gateway` project's existing design).

This proposal does NOT build a distributed routing system (that is `distributed-routing`). It only builds the *tools themselves* and registers them with mcp-gateway. The tools are immediately useful even without the hub — any Claude Code session on the local machine picks them up through mcp-gateway automatically.

## Scope

### In scope

New MCP tool servers, each as a small Rust binary:

- `companion-mcp-shell` — run shell commands in the user's environment (with allowlist support)
- `companion-mcp-apps` — launch applications via `xdg-open` / `gtk-launch`
- `companion-mcp-screenshot` — capture screen/window/region via `grim` + `slurp` (Wayland)
- `companion-mcp-clipboard` — read/write the clipboard via `wl-clipboard`
- `companion-mcp-notify` — desktop notifications via libnotify (picked up by DankMaterialShell / any freedesktop-compliant daemon)
- `companion-mcp-journal` — read the user journal via `journalctl --user`
- `companion-mcp-niri` — control Niri compositor via `niri msg` (focus, spawn, workspace, windows, event stream)

Home-manager module additions:

- `services.axios-companion.spoke.enable` — master enable for spoke tools
- `services.axios-companion.spoke.tools.<tool>.enable` — per-tool toggle
- `services.axios-companion.spoke.tools.shell.allowlist` — command allowlist (`["*"]` for fully open)
- Automatic registration of enabled tool servers with `services.mcp-gateway.servers`

### Out of scope

- The hub daemon changes to route tool calls to remote spokes (see `distributed-routing`)
- Active-spoke presence detection (see `distributed-routing`)
- Cross-machine conversation state (see `distributed-routing`)
- Browser extension integration (deferred — might be v2+ if useful)

### Non-goals

- Building a new daemon parallel to mcp-gateway — these tools register WITH mcp-gateway, which already exists
- Application-level auth on the tool endpoints — Tailscale provides network trust per the `mcp-gateway` design
- X11 tool variants — Wayland-only (grim/slurp/wl-clipboard/wtype); axios users run Niri

## Dependencies

- `bootstrap` (for the home-manager module structure)
- `mcp-gateway` (external — Keith's existing project at kcalvelli/mcp-gateway)

This proposal does NOT depend on `daemon-core` or any Tier 1 proposal. Spoke tools are useful standalone — any Claude Code session on the local machine can invoke them via mcp-gateway, even without the companion daemon.

## Success criteria

1. Each tool server is an independently-buildable Nix package exposing a stdio MCP server
2. Enabling `services.axios-companion.spoke.enable = true` registers all enabled tools with mcp-gateway via `services.mcp-gateway.servers.*`
3. After `home-manager switch`, `mcp-gw tools list` shows all enabled companion tools alongside existing mcp-gateway tools
4. A `companion "take a screenshot and tell me what's on screen"` invocation (at Tier 0 or Tier 1) successfully calls `companion-mcp-screenshot`, returns the image, and Claude describes it
5. The shell allowlist is enforced — commands not in the allowlist are rejected before execution
6. Tool invocations are audit-logged to the user journal via `journalctl --user -u mcp-gateway`
7. All tools work on a standard axios Niri + DMS environment with no X11 fallbacks
