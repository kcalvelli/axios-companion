# Proposal: CLI Client — Tier 1 Rich Command-Line Interface

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

Upgrade the Tier 0 `companion` shell wrapper to a proper Rust CLI binary with subcommands that communicate with the Tier 1 daemon via D-Bus. The upgraded CLI retains full backward compatibility with the Tier 0 invocation forms (`companion "prompt"`, `companion` for interactive mode) but adds first-class subcommands for status, logs, session management, and memory inspection.

## Motivation

Once the daemon exists, every interaction with the companion should go through it rather than spawning a fresh Claude subprocess per invocation — so that conversation state, session history, and channel activity are all coherent from the user's perspective. The CLI becomes the primary user-facing interface on the local machine, and it should be scriptable, pipe-friendly, and discoverable.

## Scope

### In scope

- Replace the Tier 0 shell wrapper with a Rust binary built on `clap` and `zbus`
- Subcommands:
  - `companion [prompt]` — send a message (interactive if no prompt, streams response)
  - `companion chat` — explicit interactive REPL mode
  - `companion status` — daemon health, active sessions, last activity per surface
  - `companion logs [-f] [--surface <name>]` — tail or filter daemon logs
  - `companion sessions list|show|resume|delete` — session management
  - `companion memory list|show|edit` — browse and edit workspace memory files
  - `companion surfaces` — list registered conversation surfaces
- Passthrough mode: unknown flags and arguments get forwarded to the daemon's SendMessage with appropriate context
- Shell completions generated via `clap_complete` for bash/fish/zsh
- Pipe friendliness: `companion -` reads prompt from stdin, writes response to stdout
- Exit code discipline: 0 on success, non-zero on errors, clear stderr messages

### Out of scope

- The daemon itself (`daemon-core` proposal)
- The TUI dashboard (`tui-dashboard` proposal — separate binary)
- Any non-D-Bus transport (remote access is Tier 2)
- GUI integration

### Non-goals

- Reinventing what Claude Code's own CLI already does — if the user wants to bypass the daemon entirely, Tier 0 `companion-raw` (or direct `claude` invocation) is still available
- Multi-user coordination — the CLI talks only to the user's own daemon

## Dependencies

- `bootstrap`
- `daemon-core`

## Success criteria

1. `companion "hello"` works identically to Tier 0 from the user's perspective, but internally goes through the daemon's D-Bus interface
2. `companion status` shows useful operational information including active sessions and uptime
3. `companion sessions resume <id>` resumes a specific session from any channel
4. Shell completions work in bash, fish, and zsh after `home-manager switch`
5. The Tier 0 raw wrapper remains available as `companion-raw` or similar for users who want to bypass the daemon
6. Tab completion for subcommands, session IDs, and memory file names works
