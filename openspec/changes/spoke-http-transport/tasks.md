# Tasks — spoke-http-transport

## Phase 1: HTTP transport in spoke-tools lib

- [x] Make `handle_request` pub in lib.rs
- [x] Add `serve_http()` — axum server, single `POST /mcp` endpoint,
      `Mcp-Session-Id` on initialize, 202 for notifications, 405 on GET
- [x] Add `run()` helper — branches on `MCP_TRANSPORT` env var
- [x] Add axum + tokio `net` to Cargo.toml
- [x] Add reqwest (dev) for HTTP transport tests
- [x] 5 HTTP transport tests (initialize+session-id, tools/call, notification→202, GET→405, session-id echo)

## Phase 2: Binary wiring

- [x] Replace `serve(H)` with `run(H)` in all 7 binaries
- [x] Handle shell.rs name collision (`run` local fn vs import)
- [x] All 11 tests pass (6 existing + 5 new)

## Phase 3: Home-manager module

- [x] Add `http.enable` + `http.port` sub-options to all 7 tool definitions
- [x] Generate systemd user services for HTTP-mode spokes (long-running, Restart=on-failure)
- [x] Shell HTTP service includes COMPANION_SHELL_ALLOWLIST in Environment
- [x] `nix flake check` passes

## Phase 4: Verification

- [ ] End-to-end: start a spoke in HTTP mode, hit it with curl
- [ ] End-to-end: configure edge's mcp-gateway with a remote HTTP spoke on mini
- [ ] Update proposal.md status
- [ ] Update ROADMAP.md
