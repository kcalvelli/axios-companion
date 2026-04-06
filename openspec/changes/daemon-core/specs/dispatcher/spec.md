# Dispatcher Specification

## Purpose

The dispatcher is the architectural core of `companion-core`. It receives messages from any surface (D-Bus, HTTP, channel adapters), routes them to `claude` via the Tier 0 `companion` wrapper, manages session mapping, and streams responses back. This specification defines the surface abstraction, message flow, concurrency model, and wrapper invocation contract.

The dispatcher is explicitly designed so that `openai-gateway` (the next Tier 1 proposal) can register as a surface without restructuring the core.

## ADDED Requirements

### Requirement: Surface Trait Abstraction

The dispatcher MUST accept messages via a `Surface` trait that any inbound adapter implements. The trait decouples the dispatcher from the transport mechanism — D-Bus, HTTP, Telegram, or any future surface all look the same to the dispatcher.

A surface identifies itself with a `surface_id` (a static string like `"dbus"`, `"openai"`, `"telegram"`) and provides a `conversation_id` for each distinct conversation thread within that surface. The dispatcher uses the `(surface_id, conversation_id)` pair to resolve session state.

The `Surface` trait is an internal Rust abstraction, not a D-Bus or network interface. External adapters (D-Bus server, HTTP server, bot client) implement `Surface` and call the dispatcher's `dispatch()` method.

#### Scenario: D-Bus surface submits a turn

- **Given**: The D-Bus interface receives a `SendMessage("dbus", "conv-1", "hello")` call
- **When**: The D-Bus surface implementation calls `dispatcher.dispatch(turn_request)`
- **Then**: The dispatcher processes the request identically to any other surface
- **And**: The response flows back through the same `TurnResponse` stream

#### Scenario: OpenAI gateway surface submits a turn (future)

- **Given**: An HTTP handler receives a `/v1/chat/completions` request
- **When**: The openai-gateway surface calls `dispatcher.dispatch(turn_request)` with `surface_id = "openai"`
- **Then**: The dispatcher processes it identically to a D-Bus request
- **And**: The gateway maps the `TurnResponse` stream to OpenAI SSE format

**Session policy is the surface's responsibility.** The dispatcher treats `conversation_id` as an opaque string. A surface that wants `per-conversation-id` behavior passes the client's conversation identifier. A surface that wants `single-session` behavior always passes the same ID (e.g., `"gateway-default"`). A surface that wants `ephemeral` behavior passes a unique UUID per request. The dispatcher does not interpret or enforce session policies — it maps `(surface_id, conversation_id)` to a claude session and dispatches.

### Requirement: TurnRequest and TurnResponse Types

The dispatcher MUST define the following types:

**TurnRequest:**

```
surface_id: String       — identifies the surface (e.g., "dbus", "openai", "telegram")
conversation_id: String  — identifies the conversation within the surface
message_text: String     — the user's message
```

**TurnResponse:**

An async stream of `TurnEvent` variants:

```
TextChunk(String)         — a partial response chunk (from stream-json)
Complete(String)           — the full response text (emitted once at the end)
Error(String)              — an error description (emitted once, terminates the stream)
```

The `Complete` event carries the full accumulated response text for surfaces that need it (like `SendMessage` which returns the full string). The `TextChunk` events carry incremental chunks for streaming surfaces. A response stream emits zero or more `TextChunk` events followed by exactly one `Complete` or `Error` event.

#### Scenario: Successful turn produces chunks then completion

- **Given**: A turn request is dispatched
- **When**: The claude subprocess streams output
- **Then**: The dispatcher emits `TextChunk` events as text appears in stream-json
- **And**: After the subprocess exits successfully, emits `Complete` with the full text
- **And**: The stream ends

#### Scenario: Failed turn produces an error event

- **Given**: A turn request is dispatched
- **When**: The claude subprocess exits with non-zero status
- **Then**: The dispatcher emits `Error` with a description
- **And**: The stream ends
- **And**: No `Complete` event is emitted

### Requirement: Wrapper Invocation Via Programmatic Contract

The dispatcher MUST invoke the Tier 0 `companion` wrapper for every turn. It MUST NOT invoke `claude` directly or reimplement persona resolution, workspace injection, or MCP config detection. The wrapper is the primitive.

The dispatcher constructs the invocation based on session state:

**First turn (no existing claude session):**

```
companion -p "<message_text>" --output-format stream-json --verbose
```

**Subsequent turns (existing claude session-id):**

```
companion --resume <claude_session_id> -p "<message_text>" --output-format stream-json --verbose
```

The `--output-format stream-json --verbose` flags are always present — the daemon requires stream-json output with the `init` event to capture session metadata.

The dispatcher MUST find the `companion` binary via `$PATH` at runtime. It MUST NOT hardcode a path or require build-time coupling to the wrapper package.

#### Scenario: Companion binary not found

- **Given**: The `companion` binary is not on `$PATH`
- **When**: The dispatcher attempts to spawn a turn
- **Then**: It emits an `Error` event with a message indicating the binary was not found
- **And**: The daemon continues running (this is a turn-level failure, not a daemon crash)

#### Scenario: First message in a new conversation

- **Given**: No session exists for `("dbus", "conv-42")`
- **When**: The dispatcher handles a `TurnRequest` for that pair
- **Then**: It spawns `companion -p "hello" --output-format stream-json --verbose` (no `--resume`)
- **And**: It captures the `session_id` from the stream-json `init` event
- **And**: It stores the mapping `("dbus", "conv-42") → <session_id>` in the session store

#### Scenario: Follow-up message in an existing conversation

- **Given**: A session exists for `("dbus", "conv-42")` with `claude_session_id = "abc-123"`
- **When**: The dispatcher handles a `TurnRequest` for that pair
- **Then**: It spawns `companion --resume abc-123 -p "follow-up" --output-format stream-json --verbose`
- **And**: The response has conversation context from the prior turn

### Requirement: Session-ID Capture From Stream-JSON Init Event

When the daemon spawns a `companion` subprocess with `--output-format stream-json --verbose`, the first event in the output stream is an `init` event containing a `session_id` field (UUID format).

The dispatcher MUST:

1. Parse the `init` event from the subprocess stdout
2. Extract the `session_id` field
3. Store it in the session store as the `claude_session_id` for the current `(surface_id, conversation_id)` pair

This captured `session_id` is used for `--resume` on subsequent turns in the same conversation.

#### Scenario: Session-ID is captured from init event

- **Given**: The dispatcher spawns a new conversation (no `--resume`)
- **When**: The subprocess emits `{"type":"system","subtype":"init","session_id":"17bcabb7-...","...":"..."}`
- **Then**: The dispatcher stores `"17bcabb7-..."` as the `claude_session_id` for this conversation
- **And**: Subsequent turns for this conversation use `--resume 17bcabb7-...`

#### Scenario: Session-ID is verified on resumed sessions

- **Given**: A session exists with `claude_session_id = "abc-123"`
- **When**: The dispatcher spawns `companion --resume abc-123 -p "msg" --output-format stream-json --verbose`
- **And**: The subprocess emits an `init` event with `session_id = "abc-123"`
- **Then**: The dispatcher confirms the session-id matches and proceeds normally

#### Scenario: Resumed session is not found by claude

- **Given**: A session exists in the store with `claude_session_id = "old-dead-session"`
- **And**: Claude Code has purged that session from `~/.claude/projects/`
- **When**: The dispatcher spawns `companion --resume old-dead-session -p "msg" --output-format stream-json --verbose`
- **And**: The subprocess exits with an error (session not found)
- **Then**: The dispatcher emits an `Error` event to the surface
- **And**: The session mapping remains in the store (the caller may retry with a new conversation_id, or a future version may clear stale mappings)

### Requirement: Stream-JSON Parsing

The dispatcher MUST parse the stream-json output from the `companion` subprocess. Each line of stdout is a JSON object with a `type` field. The dispatcher MUST handle at minimum:

| `type` | `subtype` | Action |
|--------|-----------|--------|
| `system` | `init` | Extract `session_id`, `model`; store metadata |
| `assistant` | — | Extract `message.content[].text` chunks; emit `TextChunk` events |
| `result` | `success` | Extract `result` as final text; emit `Complete` event |
| `result` | `error` | Extract error info; emit `Error` event |

Other event types (`rate_limit_event`, etc.) MUST be logged at debug level and otherwise ignored. The dispatcher MUST NOT fail on unrecognized event types — forward compatibility requires ignoring unknown events.

#### Scenario: Normal response is parsed

- **Given**: A claude subprocess produces init → assistant → result events
- **When**: The dispatcher parses stdout line by line
- **Then**: It emits `TextChunk` events from the assistant message content
- **And**: Emits `Complete` with the `result` field from the result event

#### Scenario: Unknown event type is ignored

- **Given**: A future version of claude emits a `{"type":"telemetry","...":"..."}` event
- **When**: The dispatcher encounters it
- **Then**: It logs the event at debug level
- **And**: Continues processing subsequent events normally

### Requirement: Turn Serialization Per Session

The dispatcher MUST serialize turns within a single session. Only one `companion --resume <session-id>` subprocess may be running at a time for a given `claude_session_id`. Claude Code does not support concurrent writes to the same session.

If a second `TurnRequest` arrives for a session that has an in-flight turn, the request MUST be queued and processed after the in-flight turn completes. The queue depth is unbounded (callers are responsible for not flooding).

Different sessions (different `(surface_id, conversation_id)` pairs with different `claude_session_id` values) MUST be allowed to run concurrently.

#### Scenario: Concurrent requests to the same session

- **Given**: A turn is in-flight for session `("dbus", "conv-1")` with `claude_session_id = "abc"`
- **When**: A second `TurnRequest` arrives for `("dbus", "conv-1")`
- **Then**: The second request is queued
- **And**: It is dispatched after the first turn completes
- **And**: The second response is correct (it sees the first turn's context)

#### Scenario: Concurrent requests to different sessions

- **Given**: A turn is in-flight for session `("dbus", "conv-1")`
- **When**: A `TurnRequest` arrives for `("dbus", "conv-2")`
- **Then**: Both turns run concurrently (separate claude subprocesses)
- **And**: Neither blocks the other

### Requirement: Cancellation On Surface Disconnect

If a surface disconnects or cancels its request while a turn is in-flight, the dispatcher MUST send SIGTERM to the associated `claude` subprocess and drop the response stream. The session remains valid for future turns — cancellation does not invalidate the session mapping.

#### Scenario: D-Bus client disconnects mid-turn

- **Given**: A D-Bus client calls `StreamMessage` and a claude subprocess is running
- **When**: The D-Bus client disconnects before the response completes
- **Then**: The dispatcher sends SIGTERM to the claude subprocess
- **And**: The session mapping remains in the store for future use
- **And**: No response events are emitted after cancellation
