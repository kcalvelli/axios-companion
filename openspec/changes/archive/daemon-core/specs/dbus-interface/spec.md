# D-Bus Interface Specification

## Purpose

The daemon exposes a D-Bus interface on the user session bus as the primary control plane for Tier 1. CLI clients, the TUI dashboard, and any local tool can interact with the companion via this interface. This specification defines the bus name, object path, interface version, methods, signals, and error handling.

The D-Bus surface is one implementation of the dispatcher's `Surface` trait — it translates D-Bus method calls into `TurnRequest` values, routes them through the dispatcher, and maps `TurnResponse` events back to D-Bus replies or signals.

## ADDED Requirements

### Requirement: Bus Name and Object Path

The daemon MUST register the well-known name `org.axios.Companion` on the user session bus. All methods and signals MUST be exposed on:

- **Object path:** `/org/axios/Companion`
- **Interface:** `org.axios.Companion1`

The interface name is versioned (`Companion1`) to allow API evolution without breaking existing clients. Future incompatible changes would introduce `org.axios.Companion2`.

#### Scenario: Interface is discoverable

- **Given**: The daemon is running
- **When**: A user runs `busctl --user introspect org.axios.Companion /org/axios/Companion`
- **Then**: The output lists the `org.axios.Companion1` interface
- **And**: All methods and signals defined in this spec are visible

### Requirement: SendMessage Method (Synchronous)

```
SendMessage(surface: s, conversation_id: s, message: s) → (response: s)
```

Submits a message and blocks until the full response is available. Returns the complete response text as a single string. This is the simplest integration point — suitable for scripts, one-off queries, and clients that do not need streaming.

The method MUST:

- Create a `TurnRequest` with the provided `surface`, `conversation_id`, and `message`
- Route it through the dispatcher
- Accumulate the full response from `TurnEvent::Complete`
- Return the response string

If the turn fails, the method MUST return a D-Bus error reply (see error handling requirement).

#### Scenario: Simple message and response

- **Given**: The daemon is running
- **When**: A client calls `SendMessage("dbus", "test-conv", "what is 2+2")`
- **Then**: The method blocks until claude responds
- **And**: Returns a string containing the response

#### Scenario: Conversation continuity

- **Given**: A client has called `SendMessage("dbus", "conv-1", "my name is Keith")` and received a response
- **When**: The same client calls `SendMessage("dbus", "conv-1", "what is my name?")`
- **Then**: The response references "Keith" (the session was resumed)

### Requirement: StreamMessage Method (Asynchronous)

```
StreamMessage(surface: s, conversation_id: s, message: s) → ()
```

Submits a message and returns immediately. Response chunks are delivered via `ResponseChunk` signals, with `ResponseComplete` or `ResponseError` at the end. This is the streaming integration point for the TUI dashboard and CLI client.

The method MUST:

- Create a `TurnRequest` with the provided arguments
- Route it through the dispatcher
- Return immediately (void reply)
- Emit `ResponseChunk` signals as `TurnEvent::TextChunk` events arrive
- Emit `ResponseComplete` when `TurnEvent::Complete` arrives
- Emit `ResponseError` if `TurnEvent::Error` arrives

#### Scenario: Streaming response via signals

- **Given**: The daemon is running
- **When**: A client calls `StreamMessage("dbus", "conv-1", "write a haiku")`
- **Then**: The method returns immediately (no error = accepted)
- **And**: `ResponseChunk` signals are emitted as claude generates text
- **And**: `ResponseComplete` is emitted with the full response when done

#### Scenario: Client listens for signals before calling

- **Given**: A client subscribes to `ResponseChunk` and `ResponseComplete` signals filtered by `surface="dbus"` and `conversation_id="conv-1"`
- **When**: The client calls `StreamMessage("dbus", "conv-1", "hello")`
- **Then**: The client receives only signals matching its filter
- **And**: Does not receive signals from other conversations

### Requirement: ListSessions Method

```
ListSessions() → a(ssssu)
```

Returns an array of tuples, one per session in the store:

```
(surface: s, conversation_id: s, claude_session_id: s, status: s, last_active: u)
```

Where:

- `surface` — the surface_id that owns this session
- `conversation_id` — the conversation identifier within that surface
- `claude_session_id` — the claude session UUID (empty string if first turn hasn't completed)
- `status` — `"active"` or `"archived"`
- `last_active` — Unix timestamp of the last turn

#### Scenario: Sessions are listed

- **Given**: The daemon has processed messages on two conversations
- **When**: A client calls `ListSessions()`
- **Then**: The result contains two entries with the correct surface, conversation_id, and timestamps

#### Scenario: No sessions exist

- **Given**: The daemon just started with an empty session store
- **When**: A client calls `ListSessions()`
- **Then**: The result is an empty array

### Requirement: GetStatus Method

```
GetStatus() → a{sv}
```

Returns a dict of status information:

| Key | Type | Description |
|-----|------|-------------|
| `uptime_seconds` | `u` | Seconds since daemon started |
| `active_sessions` | `u` | Number of sessions with status `"active"` |
| `in_flight_turns` | `u` | Number of claude subprocesses currently running |
| `version` | `s` | Daemon version string |

#### Scenario: Status is reported

- **Given**: The daemon has been running for 300 seconds with 2 active sessions and 1 in-flight turn
- **When**: A client calls `GetStatus()`
- **Then**: The result contains `uptime_seconds=300`, `active_sessions=2`, `in_flight_turns=1`

### Requirement: GetActiveSurfaces Method

```
GetActiveSurfaces() → as
```

Returns a list of unique `surface_id` strings that have at least one active session.

#### Scenario: Multiple surfaces active

- **Given**: The daemon has sessions for surfaces `"dbus"` and `"openai"`
- **When**: A client calls `GetActiveSurfaces()`
- **Then**: The result is `["dbus", "openai"]`

### Requirement: Response Signals

The daemon MUST emit the following signals on the `org.axios.Companion1` interface, scoped by `surface` and `conversation_id` so clients can filter:

**ResponseChunk:**

```
ResponseChunk(surface: s, conversation_id: s, chunk: s)
```

Emitted for each text chunk during a streaming response. Only emitted when the turn was initiated via `StreamMessage`.

**ResponseComplete:**

```
ResponseComplete(surface: s, conversation_id: s, full_text: s)
```

Emitted once when the turn completes. Emitted for both `StreamMessage` and `SendMessage` turns, allowing a monitoring client to observe all completions.

**ResponseError:**

```
ResponseError(surface: s, conversation_id: s, error: s)
```

Emitted when a turn fails. The `error` string contains a human-readable description.

#### Scenario: Signal filtering

- **Given**: Client A subscribes to signals with `conversation_id="conv-1"` and Client B subscribes with `conversation_id="conv-2"`
- **When**: Both conversations have active streaming turns
- **Then**: Client A receives only `conv-1` signals
- **And**: Client B receives only `conv-2` signals

### Requirement: D-Bus Error Replies

When a method call fails, the daemon MUST return a D-Bus error reply using standard error naming conventions. The following error names are defined:

| Error name | When |
|------------|------|
| `org.axios.Companion1.Error.TurnFailed` | Claude subprocess exited non-zero or produced unparseable output |
| `org.axios.Companion1.Error.SessionNotFound` | A `--resume` was attempted but claude could not find the session |
| `org.axios.Companion1.Error.InvalidArgument` | A required argument was empty or malformed |
| `org.axios.Companion1.Error.DaemonShuttingDown` | The daemon is in shutdown and not accepting new requests |

Error replies MUST include a human-readable description string.

#### Scenario: Turn failure returns D-Bus error

- **Given**: A client calls `SendMessage("dbus", "conv-1", "hello")`
- **And**: The claude subprocess crashes
- **When**: The method returns
- **Then**: The return is a D-Bus error reply with name `org.axios.Companion1.Error.TurnFailed`
- **And**: The error message describes what happened

#### Scenario: Empty message is rejected

- **Given**: A client calls `SendMessage("dbus", "conv-1", "")`
- **When**: The method is invoked
- **Then**: The return is a D-Bus error reply with name `org.axios.Companion1.Error.InvalidArgument`
- **And**: No claude subprocess is spawned
