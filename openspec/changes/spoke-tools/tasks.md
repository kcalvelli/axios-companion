# Tasks: Spoke Tools — Tier 2

One shippable commit per phase. Phase 0 + 1 are bundled because the
first tool is what proves the scaffolding works.

## Phase 0 + 1: scaffolding + `notify`

### Cargo package

- [x] **0.1** `packages/spoke-tools/Cargo.toml` declares the crate
  `companion-spoke-tools` with one `[[bin]]` entry
  `companion-mcp-notify`. Future tools add their own `[[bin]]`.
- [x] **0.2** `src/lib.rs` exposes the shared shell:
  `ToolHandler` trait (`server_name`, `tools`, async `call`), `serve()`
  loop, helpers `jsonrpc_result` / `jsonrpc_error` / `tool_def` /
  `ok_text` / `ok_image` / `err_text`. MCP convention: tool-level
  failures go out as `isError: true` on the result body, not as
  JSON-RPC errors. 6 unit tests cover initialize / tools/list /
  tools/call / unknown-method / notification-suppression / isError.
- [x] **0.3** `src/bin/notify.rs` — `notify` tool takes `summary`
  (required, non-empty), `body` (optional), `urgency` (optional
  low|normal|critical, default normal). Shells out to `notify-send
  --app-name=sid --urgency <level> <summary> [body]`. Urgency is
  validated in-process rather than trusted to notify-send (notify-send
  accepts garbage with a stderr warning the MCP client never sees).
- [x] **0.4** No warnings under `cargo check` after desugaring `async
  fn` in the trait to `impl Future + Send`.

### Nix package

- [x] **0.5** `packages/spoke-tools/default.nix` builds via
  `rustPlatform.buildRustPackage` with `libnotify` as a `buildInput`
  and a `postInstall` `wrapProgram` that prepends `libnotify/bin` to
  `companion-mcp-notify`'s PATH. Future tools add their own
  wrap steps (grim for screenshot, wl-clipboard for clipboard, etc.).
- [x] **0.6** `flake.nix` exposes
  `packages.<system>.companion-spoke-tools`.

### Home-manager wiring

- [x] **0.7** `services.cairn-companion.spoke = { enable, package,
  tools.notify.enable }` options added.
- [x] **0.8** When `spoke.enable && spoke.tools.notify.enable`, emits
  `services.mcp-gateway.servers.companion-notify`. Depends on the
  consumer having the mcp-gateway home-manager module imported —
  failure mode is a clear "unknown option" error at eval time.
- [x] **0.9** Assertion: `spoke.enable → spoke.package != null`.

### Validation

- [x] **0.10** `cargo check` clean, 6/6 tests passing.
- [x] **0.11** `nix build .#companion-spoke-tools` green.
- [x] **0.12** `nix flake check` green.
- [x] **0.13** Pre-deploy stdio smoke test: piped
  `initialize` + `tools/list` + `tools/call notify` at the wrapped
  binary, got three valid JSON-RPC responses and a visible desktop
  notification on edge. The full home-manager deploy test (consumer
  adds `spoke.enable` + `spoke.tools.notify.enable`, rebuilds, runs
  `companion "send me a notification..."` and the mcp-gw tool
  registry exposes `companion-notify`) is pending a rebuild of a
  consuming host.

## Phase 2: `screenshot`

- [ ] **2.1** Add `src/bin/screenshot.rs` with tools: `screenshot_full`,
  `screenshot_region` (requires user to draw region via `slurp`),
  `screenshot_window` (focused window only).
- [ ] **2.2** Shell out to `grim` for capture, pipe through `slurp` for
  region selection. Write to a tempfile, base64-encode, return as
  MCP `ImageContent` with `mimeType: "image/png"`.
- [ ] **2.3** `default.nix` adds `grim`, `slurp` to the package's
  runtime PATH via `makeWrapper`.
- [ ] **2.4** Home-manager `spoke.tools.screenshot.enable` + auto-
  registration as `companion-screenshot`.
- [ ] **2.5** Live test: `companion "take a screenshot and tell me
  what's on screen"` returns the image, Claude describes it correctly.

## Phase 3: `clipboard`

- [ ] **3.1** `src/bin/clipboard.rs` with `clipboard_read`,
  `clipboard_write`.
- [ ] **3.2** `wl-copy` / `wl-paste` via `wl-clipboard`.
- [ ] **3.3** Home-manager wiring.
- [ ] **3.4** Live test: write then read.

## Phase 4: `journal`

- [ ] **4.1** `src/bin/journal.rs` with one tool `journal_read` taking
  `unit` (optional), `since` (optional), `lines` (optional, default 100,
  max 1000).
- [ ] **4.2** Shell out to `journalctl --user` with appropriate flags.
- [ ] **4.3** Home-manager wiring.
- [ ] **4.4** Live test.

## Phase 5: `apps`

- [ ] **5.1** `src/bin/apps.rs` with `open_url`, `launch_desktop_entry`.
- [ ] **5.2** `xdg-open` for URLs, `gtk-launch` for `.desktop` entries.
- [ ] **5.3** Home-manager wiring.
- [ ] **5.4** Live test.

## Phase 6: `niri`

- [ ] **6.1** `src/bin/niri.rs` with tools covering the useful subset of
  `niri msg`: `focus_window`, `spawn`, `focus_workspace`,
  `list_windows`, `list_workspaces`.
- [ ] **6.2** Each tool shells out to `niri msg <subcommand> --json`
  and returns structured output.
- [ ] **6.3** Home-manager wiring.
- [ ] **6.4** Live test: spawn a terminal, switch workspace, focus back.

## Phase 7: `shell`

- [ ] **7.1** `src/bin/shell.rs` with one tool `run` taking `command`
  (the argv) and `stdin` (optional).
- [ ] **7.2** Allowlist enforcement: config passed via env
  (`COMPANION_SHELL_ALLOWLIST=git,ls,cat`). `*` as a single-element list
  means "allow all" with a loud audit log line per call. Empty
  allowlist rejects everything with a clear error.
- [ ] **7.3** Home-manager `spoke.tools.shell.enable` +
  `spoke.tools.shell.allowlist`. The allowlist is marshalled into the
  env var at module-evaluation time.
- [ ] **7.4** Audit-log every invocation to the user journal
  (`tracing-journald`): command argv, allowed/denied, exit code.
- [ ] **7.5** Live test, allowed command; live test, denied command;
  live test, empty-list-denies-everything.

## Phase 8: paperwork

- [ ] **8.1** Flip ROADMAP `spoke-tools` from `[ ]` to `[x]` with
  shipped date.
- [ ] **8.2** Archive: `mv openspec/changes/spoke-tools
  openspec/changes/archive/spoke-tools`.
