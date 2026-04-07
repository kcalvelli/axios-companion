# Proposal: TUI Dashboard — Tier 1 Terminal-Native Dashboard

> **Status**: Shipped 2026-04-07. Core dashboard with sessions, conversation streaming, status bar, and vim-style navigation.

## Tier

Tier 1

## Summary

A terminal-native dashboard (`companion-tui`) built on `ratatui` that provides a live view into the companion daemon — active sessions per surface, streaming conversation view, and daemon status. Modeled on the design language of `lazygit`, `gitui`, `btop`, and `zellij`: fast, keyboard-driven, vim-style navigation, beautiful in a terminal.

## Motivation

Users who live in terminals deserve a native dashboard that doesn't require a browser or a GUI session. A TUI is also the only dashboard option that works over SSH, fits the aesthetic of the typical axios-companion user (terminal-first, NixOS, tiling WM), and runs in any environment with a terminal emulator — no desktop session required. The TUI is likely to be the primary dashboard experience for most users, making GUI clients an optional polish layer rather than a necessity.

## Scope

### Shipped

- `companion-tui` binary built on `ratatui` + `crossterm` + `zbus`
- Panels:
  - **Sessions**: live list of active conversations across all surfaces with last-activity timestamps, vim `j/k` selection
  - **Conversation**: focused session's streaming output, word-wrapped, scrollable
  - **Status bar**: daemon version, uptime, active session count, in-flight turns
- Vim-style keybindings: `j/k` navigate, `g/G` top/bottom, `Tab`/`1`/`2` panel switching
- `?` help overlay, `q` quit, `Ctrl-C` quit
- Live updates via D-Bus signals from the daemon (no polling for conversation data)
- Graceful degradation if the daemon is not running (shows connection status, auto-reconnects)
- Nix package (`companion-tui`), home-manager `tui.enable` option with daemon assertion

### Deferred (needs new daemon capabilities)

- **Memory panel**: file tree view of workspace — needs workspace path discovery via D-Bus
- **Events panel**: rolling log of tool calls, subprocess lifecycle — needs daemon event streaming
- **Usage panel**: cost and token counters — needs daemon usage tracking
- Syntax highlighting in conversation view
- `:` command mode

### Out of scope

- Any non-TUI dashboard (GUI is a separate proposal)
- Remote access — the TUI connects to the local session's daemon
- Direct Claude subprocess control (all goes through the daemon)
- Message sending from the TUI — this is a monitoring dashboard, not a chat client

### Non-goals

- A mouse-first UX — this is keyboard-driven
- Matching the feature set of web-based dashboards
- Built-in terminal multiplexing

## Dependencies

- `bootstrap`
- `daemon-core`

## Success criteria

1. [x] `companion-tui` launches and connects to the daemon via D-Bus
2. [x] Sessions panel renders live session list with selection
3. [x] Keyboard navigation is intuitive for users of `lazygit`/`gitui`
4. [x] Streaming conversation view updates live as the daemon receives events from Claude
5. [x] Connection loss is handled gracefully with a clear reconnect UI
6. [x] Works over SSH and inside tmux/zellij (crossterm backend)
7. [x] `nix build .#companion-tui` produces a clean build
8. [x] Home-manager integration with `tui.enable` option
