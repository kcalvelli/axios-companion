# Proposal: Daemon Core — Tier 1 Foundation

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1 (Single-machine daemon foundation)

## Summary

Introduce a user-level systemd daemon (`companion-core`) that runs continuously, manages persistent sessions across multiple conversation surfaces, and exposes a D-Bus control plane on the user session bus. This is the foundation on which channel adapters, the CLI client, the TUI dashboard, and the distributed hub all depend.

## Motivation

Tier 0 gives every user a working companion, but it has no memory of the conversations users have with it beyond what Claude Code's own session storage captures, no way to receive messages from Telegram/Discord/email/XMPP, and no surface for other tools to query the companion's state. Tier 1 adds a persistent daemon that turns the wrapper from a one-shot command into a live service with addressable state.

The daemon does NOT replace the claude-code subprocess model. It spawns `claude -p` per turn, exactly as Tier 0 does. What it adds is:

- A persistent process that can receive messages from many sources
- A session-to-conversation mapping (so Telegram thread X always resumes claude session Y)
- A D-Bus interface that clients (CLI, TUI, future GUI) talk to instead of spawning claude themselves
- A lifecycle that survives between user invocations, ready to receive the next message from any channel

## Scope

### In scope

- `companion-core` binary — a Rust async daemon (reasons: review capability, lean ecosystem, matches existing project language decisions)
- `systemd --user` unit file managed by home-manager
- Session store — sqlite schema mapping `(surface, conversation_id)` → `claude_session_id` with timestamps and metadata
- D-Bus interface `org.axios.Companion` exposing methods: `SendMessage`, `GetStatus`, `ListSessions`, `StreamResponse`, `GetActiveSurfaces`
- Claude subprocess lifecycle management — spawn `claude -p --output-format stream-json --resume <session>`, parse stream events, route output to the requesting surface
- Persona loading — same resolution logic as Tier 0, but loaded once at daemon startup and reused across turns
- Graceful shutdown, reload on config change, error recovery on subprocess failure

### Out of scope

- Any channel adapter (Telegram, Discord, email, XMPP — each is its own proposal)
- The CLI client (`cli-client` proposal)
- The TUI dashboard (`tui-dashboard` proposal)
- Multi-machine routing (Tier 2)
- Tool servers (Tier 2)

### Non-goals

- Replacing Claude Code's session storage — the daemon maps surfaces to Claude sessions but delegates actual conversation history to `~/.claude/projects/`
- Providing a network-exposed API — the D-Bus interface is session-local; remote access is a Tier 2 concern via mcp-gateway
- Multi-user support within a single daemon — one daemon per user, enforced by running as a `--user` service

## Dependencies

- `bootstrap` must be shipped (the daemon reuses the persona resolution logic and module option shape)

## Success criteria

1. A user can enable `services.axios-companion.daemon.enable = true` and have a running user-level service after `home-manager switch`
2. `busctl --user introspect org.axios.Companion /` shows the documented interface
3. `busctl --user call org.axios.Companion / org.axios.Companion1 SendMessage s "hello"` returns a response streamed from Claude
4. The daemon survives Claude subprocess failures and restarts the conversation on the next message
5. Session state persists across daemon restarts (sqlite is on disk, not in memory)
6. Tier 0 (`companion` wrapper) continues to work unchanged — it's a direct Claude invocation and does not go through the daemon
