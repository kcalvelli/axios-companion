# Proposal: Spoke Tools — Machine-Local MCP Tool Servers

## Tier

Tier 2

## Summary

Ship a set of MCP tool servers that expose the local machine's
capabilities — desktop notifications, screenshot capture, clipboard
access, journal reading, app launching, Niri compositor control, and
allowlisted shell execution — as MCP tools consumable by Claude Code
via mcp-gateway. Each tool is a binary inside a single cargo package
registered with mcp-gateway through the cairn-companion home-manager
module.

Together, these tools turn every cairn-companion-enabled machine into
a tool surface the future Tier 2 hub can route actions to. They are
also immediately useful at Tier 0 — any Claude Code session on the
local machine picks them up through mcp-gateway automatically, with no
daemon-core dependency.

## Motivation

Every existing cairn-companion surface *talks*: Telegram, Discord,
email, XMPP, the CLI, the TUI, the OpenAI gateway. None of them *act*
on the machine they run on. The core Tier 2 promise is "the companion
lives on the desktop you're currently using," which requires the
companion to actually do things on that desktop — show a notification,
take a screenshot, open a URL, focus a window, run a command.

MCP is the right boundary for this. mcp-gateway already aggregates
stdio MCP servers into a single HTTP endpoint with a tool registry,
and it has a declarative home-manager module for server registration.
Cairn-companion just needs to ship the tool binaries and wire them up.

This proposal does NOT build a distributed routing system (that is
`distributed-routing`, Tier 2 phase 2). It only builds the *tools* and
registers them with the local mcp-gateway. The tools are useful
standalone — any Claude Code invocation on the machine picks them up.

## Scope

### In scope

Seven MCP tool binaries, shipped as one cargo package
`companion-spoke-tools` with multiple `[[bin]]` entries (one crate, N
binaries, shared JSON-RPC MCP shell in the library root):

- `companion-mcp-notify` — desktop notifications via `notify-send`
  (libnotify; picked up by DankMaterialShell / mako / any
  freedesktop-compliant daemon).
- `companion-mcp-screenshot` — capture full screen / window / region
  via `grim` + `slurp` on Wayland. Returns base64-encoded PNG as MCP
  `ImageContent`.
- `companion-mcp-clipboard` — read and write the primary clipboard via
  `wl-clipboard` (`wl-copy`, `wl-paste`).
- `companion-mcp-journal` — read the user journal via
  `journalctl --user`. Read-only.
- `companion-mcp-apps` — launch applications via `xdg-open` or
  `gtk-launch`. Fire-and-forget.
- `companion-mcp-niri` — control the Niri compositor via `niri msg`
  (focus, spawn, workspace, windows, event subscription).
- `companion-mcp-shell` — run shell commands in the user's
  environment, gated by an allowlist.

Home-manager module additions in `modules/home-manager/default.nix`:

- `services.cairn-companion.spoke.enable` — master toggle.
- `services.cairn-companion.spoke.package` — override the tool
  package (defaults to `companion-spoke-tools`).
- `services.cairn-companion.spoke.tools.<tool>.enable` — per-tool
  toggle.
- `services.cairn-companion.spoke.tools.shell.allowlist` — list of
  command names; `[]` denies everything, `["*"]` allows everything.
- Per-enabled-tool emission of `services.mcp-gateway.servers.companion-<tool>`
  so enabling a spoke tool automatically registers it with a local
  mcp-gateway (if one is present).

### Out of scope

- Hub-to-spoke routing over Tailscale (`distributed-routing`).
- Active-spoke presence tracking (`distributed-routing`).
- Cross-machine conversation state (`distributed-routing`).
- X11 variants of graphical tools — Wayland-only (grim/slurp/wl-clipboard).
  All cairn machines run Niri.
- Browser extension integration — deferred until a real need emerges.
- Pulling in an MCP SDK crate. Follows sentinel-mcp's precedent: pure
  hand-rolled JSON-RPC over stdio, one `handle_request` function per
  tool binary, shared helpers in the library root of this package.

### Non-goals

- Application-level auth on the tool endpoints. Tailscale provides
  network trust per the mcp-gateway design.
- Shipping a new daemon — these tools are short-lived stdio processes
  spawned per-call by mcp-gateway.

## Dependencies

- `bootstrap` — for the home-manager module structure this hooks into.
- `mcp-gateway` — external, Keith's project at `kcalvelli/mcp-gateway`.
  Imported by the consuming NixOS config (e.g. `hosts/edge.nix`), not
  by this flake directly. The home-manager module emits
  `services.mcp-gateway.servers.*` values only — it does not declare
  the option.

This proposal does NOT depend on `daemon-core` or any Tier 1
proposal. Spoke tools are standalone-useful.

## Phasing

Each phase is one shippable commit. The phase ordering is by blast
radius: lowest-risk tool first to prove the pattern, highest-risk
tool last once the pattern is solid.

- **Phase 0 + 1** (one commit): scaffolding + `notify`. The cargo
  package, the shared JSON-RPC shell, the first tool binary, the
  home-manager wiring for `spoke.enable` + `spoke.tools.notify.enable`,
  the mcp-gateway auto-registration. Proves the pattern end-to-end.
- **Phase 2**: `screenshot`. Proves the `ImageContent` return path and
  delivers the canonical multimodal demo.
- **Phase 3**: `clipboard`.
- **Phase 4**: `journal`.
- **Phase 5**: `apps`.
- **Phase 6**: `niri`.
- **Phase 7**: `shell`. Last because its allowlist design deserves
  focused attention only after the binary-and-wiring pattern is
  boring.

## Success criteria

1. `packages.<system>.companion-spoke-tools` is an independently-buildable
   Nix package producing one binary per enabled tool.
2. Enabling `services.cairn-companion.spoke.enable = true` plus any
   `spoke.tools.<tool>.enable = true` registers those tools with
   mcp-gateway via `services.mcp-gateway.servers.*`.
3. After `home-manager switch`, `mcp-gw --json list` shows the enabled
   `companion-<tool>` servers alongside existing mcp-gateway servers.
4. A `companion "send me a desktop notification that says hello"`
   invocation at Tier 0 successfully calls `companion-mcp-notify` and
   the notification appears on the running desktop.
5. A `companion "take a screenshot and tell me what's on screen"`
   invocation successfully calls `companion-mcp-screenshot`, returns
   the PNG as MCP `ImageContent`, and Claude describes it (Phase 2).
6. The shell allowlist is enforced — any command not on the allowlist
   is rejected before execution, with a clear error back through MCP
   (Phase 7).
7. All tools work on a standard cairn Niri + DMS environment with no
   X11 fallbacks.
