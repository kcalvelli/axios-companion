# Tasks: TUI Dashboard

## Phase 1: Scaffold + connection — DONE

- [x] Create `packages/tui-dashboard/` Rust crate with ratatui + crossterm + zbus
- [x] D-Bus proxy copied from cli-client
- [x] App state module with session/conversation/status types
- [x] crossterm raw mode + ratatui terminal init/restore
- [x] Event loop: keyboard input task + D-Bus poller/signal subscriber task
- [x] Connection state handling: auto-connect, reconnect on disconnect

## Phase 2: Status bar + sessions panel — DONE

- [x] Bottom status bar: version, uptime, active sessions, in-flight turns
- [x] Status polling every 2 seconds via get_status()
- [x] Sessions panel: table with surface, conversation ID, status, last active
- [x] Relative timestamp formatting (just now, 5m ago, 2h ago, etc.)
- [x] vim j/k selection with highlight, selection preservation on refresh

## Phase 3: Live conversation panel — DONE

- [x] Right panel subscribes to D-Bus response_chunk/complete/error signals
- [x] Filters signals by selected session's (surface, conversation_id)
- [x] Accumulates streaming chunks in per-session buffer
- [x] Word-wrapped text with scroll support (j/k when focused, g/G for top/bottom)
- [x] Turn separators (---) on completion

## Phase 4: Keybindings + polish — DONE

- [x] Tab / 1 / 2 for panel switching with cyan focus highlight
- [x] q to quit, Ctrl-C to quit
- [x] ? help overlay with keybinding reference
- [x] Esc closes help overlay
- [x] Graceful reconnect loop when daemon restarts

## Nix integration — DONE

- [x] `packages/tui-dashboard/default.nix` (rustPlatform.buildRustPackage)
- [x] `companion-tui` exposed in `flake.nix` packages
- [x] Home-manager: `services.axios-companion.tui.enable` + `tui.package`
- [x] Assertion: `tui.enable → daemon.enable`
- [x] `nix build .#companion-tui` — clean

## Deferred

- [ ] Memory panel (needs daemon workspace path method)
- [ ] Events panel (needs daemon event streaming)
- [ ] Usage panel (needs daemon cost/token tracking)
- [ ] Syntax highlighting in conversation view
- [ ] `:` command mode
