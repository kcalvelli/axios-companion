# Spec: D-Bus Proxy — cli-client

## Summary

The CLI uses a zbus 5 proxy trait to call the daemon's `org.axios.Companion1`
interface on the session bus. This is the sole communication channel between
the CLI binary and the daemon.

## Proxy Interface

All methods mirror the daemon's D-Bus interface one-to-one:

| Method | Returns | Used by |
|---|---|---|
| `SendMessage(surface, conversation_id, message)` | `String` | (reserved for non-streaming use) |
| `StreamMessage(surface, conversation_id, message)` | `()` | `companion [prompt]`, `companion chat` |
| `ListSessions()` | `Vec<(String, String, String, String, u32)>` | `companion sessions list` |
| `GetStatus()` | `HashMap<String, OwnedValue>` | `companion status` |
| `GetActiveSurfaces()` | `Vec<String>` | `companion surfaces` |

Signal subscriptions for streaming:

| Signal | Fields | Purpose |
|---|---|---|
| `ResponseChunk` | surface, conversation_id, chunk | Incremental text output |
| `ResponseComplete` | surface, conversation_id, full_text | Marks end of response |
| `ResponseError` | surface, conversation_id, error | Error termination |

## Connection

- Bus name: `org.axios.Companion`
- Object path: `/org/axios/Companion`
- Session bus (not system bus)
- Connection failure prints a diagnostic pointing at `systemctl --user status companion-core`

## Surface Identity

The CLI identifies itself as surface `"cli"` in all D-Bus calls. This
distinguishes CLI-originated sessions from gateway, Telegram, etc.

## Conversation ID

- Read from `COMPANION_CONVERSATION_ID` environment variable if set
- Otherwise: random UUID v4 per invocation
- In chat mode, the same conversation ID persists across all turns in the REPL
