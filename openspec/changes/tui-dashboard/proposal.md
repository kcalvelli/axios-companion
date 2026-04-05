# Proposal: TUI Dashboard — Tier 1 Terminal-Native Dashboard

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

A terminal-native dashboard (`companion-tui`) built on `ratatui` that provides a live view into the companion daemon — active sessions per surface, streaming conversation view, memory browser, event log, and cost/usage counters. Modeled on the design language of `lazygit`, `gitui`, `btop`, and `zellij`: fast, keyboard-driven, vim-style navigation, beautiful in a terminal.

## Motivation

Users who live in terminals deserve a native dashboard that doesn't require a browser or a GUI session. A TUI is also the only dashboard option that works over SSH, fits the aesthetic of the typical axios-companion user (terminal-first, NixOS, tiling WM), and runs in any environment with a terminal emulator — no desktop session required. The TUI is likely to be the primary dashboard experience for most users, making GUI clients an optional polish layer rather than a necessity.

## Scope

### In scope

- `companion-tui` binary built on `ratatui` + `crossterm` + `zbus`
- Panels:
  - **Sessions**: live list of active conversations across all surfaces with last-activity timestamps
  - **Conversation**: focused session's streaming claude output, syntax-highlighted
  - **Memory**: file tree view of workspace with fuzzy search and inline preview
  - **Events**: rolling log of tool calls, subprocess lifecycle, errors
  - **Usage**: cost and token counters (if subscription allows; optional for Max users)
- Vim-style keybindings: `h/j/k/l`, `/` search, `:` command mode, `g/G` top/bottom
- Tab/panel switching with number keys and `<C-n>`/`<C-p>`
- Live updates via D-Bus signals from the daemon (no polling)
- Graceful degradation if the daemon is not running (shows connection status)

### Out of scope

- Any non-TUI dashboard (GUI is a separate proposal)
- Remote access — the TUI connects to the local session's daemon
- Direct Claude subprocess control (all goes through the daemon)

### Non-goals

- A mouse-first UX — this is keyboard-driven
- Matching the feature set of web-based dashboards — the TUI is intentionally scoped to "what a terminal-native user wants to see"
- Built-in terminal multiplexing — use tmux, zellij, or your WM

## Dependencies

- `bootstrap`
- `daemon-core`

## Success criteria

1. `companion-tui` launches and connects to the daemon via D-Bus
2. All panels render without lag on realistic workloads
3. Keyboard navigation is intuitive for users of `lazygit`/`gitui`
4. Streaming conversation view updates live as the daemon receives events from Claude
5. Connection loss is handled gracefully with a clear reconnect UI
6. Works over SSH and inside tmux/zellij
