# Spec: CLI Subcommands — cli-client

## Summary

The CLI is built on `clap` (derive mode) with subcommands and a default
positional-argument mode for backward compatibility with Tier 0.

## Command Structure

```
companion [PROMPT...]           → send message (interactive if no prompt)
companion -                     → read prompt from stdin
companion chat                  → explicit interactive REPL
companion status                → daemon health info
companion sessions list         → tabular session list
companion surfaces              → list active surfaces
```

## Default Behavior (No Subcommand)

When no subcommand is given, the CLI looks at positional arguments:

1. **No args** → enters interactive REPL (equivalent to `companion chat`)
2. **`-` as sole arg** → reads full prompt from stdin, sends, prints response
3. **Any other args** → joins them with spaces, sends as single message

## Interactive REPL (`chat`)

- Prompt: `you> ` for input, `sid> ` for response
- `/quit` and `/exit` terminate the session
- EOF (Ctrl-D) terminates the session
- Empty lines are skipped
- Conversation ID persists across all turns in the REPL
- Responses stream via D-Bus signals (chunks print as they arrive)

## `status`

Calls `GetStatus()` and formats:

```
companion-core v0.1.0
  uptime:          2h 15m 30s
  active sessions: 3
  in-flight turns: 1
```

## `sessions list`

Calls `ListSessions()` and formats as a table:

```
SURFACE      CONVERSATION     CLAUDE SESSION           STATUS     LAST ACTIVE
cli          abc123           37bf3e64-b483-4e04-...   active     5m ago
openai       openai-default   0452d7a8-e978-46c0-...   active     2h ago
```

- Empty claude_session_id shown as `-`
- Timestamps shown as relative ("just now", "5m ago", "2h ago", "3d ago")
- Long IDs truncated with `...`

## `surfaces`

Calls `GetActiveSurfaces()` and prints one surface per line.

## Exit Codes

- `0` — success
- `1` — any error (daemon unreachable, D-Bus error, empty stdin)

## Deferred Subcommands

The following subcommands from the proposal skeleton are deferred to a
follow-up, as they need new daemon-side D-Bus methods:

- `companion logs [-f] [--surface <name>]` — needs daemon log streaming
- `companion sessions show|resume|delete` — needs per-session D-Bus ops
- `companion memory list|show|edit` — needs workspace filesystem access
