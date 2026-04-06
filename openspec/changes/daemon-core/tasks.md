# Tasks: Daemon Core — Tier 1 Foundation

## Phase 1: Project scaffolding and Rust workspace

- [x] **1.1** Create Rust project at `packages/companion-core/` with `Cargo.toml` and `src/main.rs` (tokio async main, tracing init, placeholder event loop)
- [x] **1.2** Create `packages/companion-core/default.nix` using `rustPlatform.buildRustPackage` (or crane — whichever is already available in the flake's nixpkgs)
- [x] **1.3** Wire `packages.${system}.companion-core` into `flake.nix`
- [x] **1.4** Add `devShells.default` with Rust toolchain (rustc, cargo, rust-analyzer, clippy, rustfmt) if not already present
- [x] **1.5** Verify `nix build .#companion-core` produces a binary that starts and exits cleanly

## Phase 2: Session store (SQLite)

- [x] **2.1** Add `rusqlite` dependency (with `bundled` feature for static SQLite)
- [x] **2.2** Implement schema creation and migration logic: `schema_version` table, version check on startup, apply pending migrations
- [x] **2.3** Implement `sessions` table creation (schema version 1) per `specs/session-store/spec.md`
- [x] **2.4** Implement CRUD operations: `create_session`, `lookup_session`, `set_claude_session_id`, `touch_session`, `list_sessions`, `list_by_surface`
- [x] **2.5** Enable WAL mode on connection open
- [x] **2.6** Unit tests for all store operations (in-memory SQLite for test speed)
- [x] **2.7** Verify database file is created at `$XDG_DATA_HOME/axios-companion/sessions.db` when run from the binary

## Phase 3: Dispatcher core

- [x] **3.1** Define the `Surface` trait with `surface_id()` method, and `TurnRequest` / `TurnEvent` / `TurnResponse` types per `specs/dispatcher/spec.md`
- [x] **3.2** Implement the dispatcher: accept `TurnRequest`, resolve session via store (create if new), build the `companion` invocation command
- [x] **3.3** Implement subprocess spawning: `tokio::process::Command` for `companion -p "..." --output-format stream-json --verbose [--resume <id>]`
- [x] **3.4** Implement stream-json line parser: read stdout line-by-line, deserialize JSON, dispatch by `type` field
- [x] **3.5** Implement session-id capture from the `init` event; call `set_claude_session_id` on the store
- [x] **3.6** Implement `TurnEvent` emission: `TextChunk` from `assistant` events, `Complete` from `result/success`, `Error` from `result/error` or subprocess failure
- [x] **3.7** Implement per-session turn serialization: tokio mutex keyed by `(surface, conversation_id)`, queue concurrent requests to the same session
- [x] **3.8** Implement cancellation: drop the `TurnResponse` stream sends SIGTERM to the subprocess
- [x] **3.9** Integration test with a mock `companion` script that emits canned stream-json output

## Phase 4: D-Bus interface

- [x] **4.1** Add `zbus` dependency for async D-Bus on tokio
- [x] **4.2** Implement the `org.axios.Companion1` interface struct with `zbus::interface` macro
- [x] **4.3** Implement `SendMessage` method: create `TurnRequest`, dispatch, accumulate `Complete`, return string (or D-Bus error)
- [x] **4.4** Implement `StreamMessage` method: create `TurnRequest`, dispatch, return immediately, emit `ResponseChunk`/`ResponseComplete`/`ResponseError` signals
- [x] **4.5** Implement `ListSessions` method: query the session store, return array of tuples
- [x] **4.6** Implement `GetStatus` method: return dict with uptime, active sessions, in-flight turns, version
- [x] **4.7** Implement `GetActiveSurfaces` method: query the session store for distinct surfaces
- [x] **4.8** Implement D-Bus error replies per `specs/dbus-interface/spec.md`
- [x] **4.9** Implement the D-Bus adapter as a `Surface` trait implementation wiring into the dispatcher
- [ ] **4.10** Integration test: start the daemon, call methods via `zbus` client, verify responses and signals

## Phase 5: Daemon lifecycle and systemd integration

- [ ] **5.1** Wire `main()`: init tracing → open session store → init dispatcher → acquire D-Bus name → enter event loop
- [ ] **5.2** Implement `sd_notify(READY=1)` via the `sd-notify` crate after successful initialization
- [ ] **5.3** Implement SIGTERM handler: stop accepting D-Bus calls, drain in-flight turns (120s timeout), close connections, exit 0
- [ ] **5.4** Implement SIGHUP handler: log and ignore (reserved for future use)
- [ ] **5.5** Verify error recovery: subprocess crash does not crash daemon, daemon continues accepting requests
- [ ] **5.6** Create the systemd unit file template: `companion-core.service` (Type=notify, Restart=on-failure, RestartSec=5)

## Phase 6: Home-manager module updates

- [ ] **6.1** Add `services.axios-companion.daemon.enable` option (boolean, default `false`)
- [ ] **6.2** Add `services.axios-companion.daemon.package` option (default: the flake's `companion-core` package)
- [ ] **6.3** Wire `systemd.user.services.companion-core` unit in the module when `daemon.enable = true`, with `Environment=` for session store path
- [ ] **6.4** Ensure Tier 0 `companion` wrapper continues to work unchanged when `daemon.enable = false` (no regressions)
- [ ] **6.5** Verify: `home-manager switch` with `daemon.enable = true` installs and starts the service

## Phase 7: End-to-end testing, documentation, and close

- [ ] **7.1** Test: `busctl --user call org.axios.Companion /org/axios/Companion org.axios.Companion1 SendMessage sss "dbus" "test" "hello"` returns a response
- [ ] **7.2** Test: `StreamMessage` emits `ResponseChunk` and `ResponseComplete` signals observable via `busctl --user monitor`
- [ ] **7.3** Test: session persistence — stop and restart the daemon, send a follow-up to a prior conversation, verify context is retained
- [ ] **7.4** Test: concurrent sessions from different conversation IDs run in parallel
- [ ] **7.5** Test: turn serialization — rapid-fire two messages to the same conversation, second waits for first
- [ ] **7.6** Test: daemon survives claude subprocess failure and handles the next message normally
- [ ] **7.7** Test: Tier 0 `companion` wrapper still works independently of the daemon
- [ ] **7.8** Code review: verify `Surface` trait design accommodates openai-gateway's needs (session policies, streaming, HTTP surface) without daemon-core modifications
- [ ] **7.9** Update `README.md` with daemon setup instructions and `daemon.enable` option documentation
- [ ] **7.10** Update `ROADMAP.md` to mark `daemon-core` complete
- [ ] **7.11** Archive to `openspec/changes/archive/daemon-core/`
