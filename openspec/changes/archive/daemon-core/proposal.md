# Proposal: Daemon Core — Tier 1 Foundation

> **Status**: Complete — shipped 2026-04-06. Validated on `edge` with live D-Bus calls.

## Tier

Tier 1 (Single-machine daemon foundation)

## Summary

Introduce a user-level systemd daemon (`companion-core`) that runs continuously, manages persistent sessions across multiple conversation surfaces, and exposes a D-Bus control plane on the user session bus. This is the foundation on which channel adapters, the CLI client, the TUI dashboard, the OpenAI gateway, and the distributed hub all depend.

## Motivation

Tier 0 gives every user a working companion, but it has no memory of the conversations users have with it beyond what Claude Code's own session storage captures, no way to receive messages from Telegram/Discord/email/XMPP, and no surface for other tools to query the companion's state. Tier 1 adds a persistent daemon that turns the wrapper from a one-shot command into a live service with addressable state.

The daemon does NOT replace the claude-code subprocess model. It invokes the Tier 0 `companion` wrapper per turn, exactly as a human would. What it adds is:

- A persistent process that can receive messages from many sources
- A session-to-conversation mapping (so Telegram thread X always resumes claude session Y)
- A D-Bus interface that clients (CLI, TUI, future GUI) talk to instead of spawning claude themselves
- A lifecycle that survives between user invocations, ready to receive the next message from any channel

## Scope

### In scope

- `companion-core` binary — a Rust async daemon (tokio runtime, single binary, no runtime dependencies beyond libc/dbus/sqlite)
- `systemd --user` unit file managed by home-manager (`Type=notify`, `Restart=on-failure`)
- Session store — SQLite database at `$XDG_DATA_HOME/axios-companion/sessions.db` mapping `(surface, conversation_id)` → `claude_session_id` with timestamps and metadata
- D-Bus interface `org.axios.Companion1` on the session bus, exposing methods: `SendMessage`, `StreamMessage`, `ListSessions`, `GetStatus`, `GetActiveSurfaces`
- D-Bus signals: `ResponseChunk`, `ResponseComplete`, `ResponseError` — scoped by surface and conversation_id for client filtering
- Dispatcher — surface-agnostic message routing that accepts `TurnRequest` values from any surface implementation, invokes the `companion` wrapper, parses stream-json output, and streams `TurnEvent` values back
- Claude subprocess lifecycle — spawn `companion -p "<msg>" --output-format stream-json --verbose [--resume <session_id>]`, capture session-id from the `init` event, serialize turns per session, allow concurrent sessions
- Graceful shutdown with in-flight turn draining
- Structured logging via `tracing` to the systemd journal

### Out of scope

- Any channel adapter (Telegram, Discord, email, XMPP — each is its own proposal)
- The OpenAI gateway HTTP endpoint (`openai-gateway` proposal)
- The CLI client (`cli-client` proposal)
- The TUI dashboard (`tui-dashboard` proposal)
- Multi-machine routing (Tier 2)
- Tool servers (Tier 2)

### Non-goals

- Replacing Claude Code's session storage — the daemon maps surfaces to Claude sessions but delegates actual conversation history to `~/.claude/projects/`
- Providing a network-exposed API — the D-Bus interface is session-local; the OpenAI gateway (separate proposal) adds HTTP
- Multi-user support within a single daemon — one daemon per user, enforced by running as a `--user` service
- Persona loading — the daemon invokes the `companion` wrapper, which handles persona/workspace/MCP injection per the Tier 0 wrapper spec's Programmatic Invocation Contract
- Runtime configuration files — all configuration flows through the home-manager module into systemd environment variables

## Architectural decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Invocation primitive | `companion` wrapper via `$PATH` | Wrapper handles persona/workspace/MCP; daemon doesn't reimplement. Build-time coupling avoided. |
| Session-id capture | Parse `session_id` from stream-json `init` event | Verified: `claude -p ... --output-format stream-json --verbose` emits UUID in `init` event. Round-trips via `--resume`. No filesystem scanning. |
| SIGHUP reload | Reserved but unimplemented | Persona is immutable Nix store paths; `home-manager switch` restarts the service. No runtime reload needed. |
| D-Bus streaming model | Signals scoped by surface + conversation_id | Simpler than FD passing. Single-user session bus. Text-only responses. Clients filter by conversation. |
| Configuration | All-from-Nix via systemd `Environment=` | Matches the declarative philosophy. No config file to parse, no config format to design. |
| Database location | `$XDG_DATA_HOME/axios-companion/sessions.db` | Sibling to workspace directory but outside it — not exposed to claude via `--add-dir`. |
| D-Bus interface version | `org.axios.Companion1` | Standard D-Bus convention. Future breaking changes go to `Companion2`. |
| Surface abstraction | Rust trait, not D-Bus/network interface | Dispatcher is transport-agnostic. openai-gateway adds HTTP; channel adapters add bot protocols. All use the same `dispatch()` path. |

## Dependencies

- `bootstrap` must be shipped (the daemon invokes the Tier 0 wrapper and reuses the module option shape)

## Success criteria

1. A user can enable `services.axios-companion.daemon.enable = true` and have a running user-level service after `home-manager switch`
2. `busctl --user introspect org.axios.Companion /org/axios/Companion` shows the `org.axios.Companion1` interface with all documented methods and signals
3. `busctl --user call org.axios.Companion /org/axios/Companion org.axios.Companion1 SendMessage sss "dbus" "test" "hello"` returns a response streamed from Claude
4. The daemon survives Claude subprocess failures and restarts the conversation on the next message
5. Session state persists across daemon restarts (sqlite is on disk, not in memory)
6. Tier 0 (`companion` wrapper) continues to work unchanged — it's a direct Claude invocation and does not go through the daemon
7. The dispatcher's `Surface` trait can be implemented by `openai-gateway` without modifying daemon-core (verified by code review, not by shipping a gateway in this proposal)

## Specs

- [`specs/daemon/spec.md`](specs/daemon/spec.md) — Daemon lifecycle, systemd integration, startup/shutdown, error recovery
- [`specs/dispatcher/spec.md`](specs/dispatcher/spec.md) — Surface trait, message dispatch, wrapper invocation, stream-json parsing, turn serialization
- [`specs/dbus-interface/spec.md`](specs/dbus-interface/spec.md) — D-Bus methods, signals, error replies
- [`specs/session-store/spec.md`](specs/session-store/spec.md) — SQLite schema, CRUD operations, migrations, WAL mode
