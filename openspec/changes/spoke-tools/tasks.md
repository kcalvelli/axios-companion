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
- [x] **0.13** Full end-to-end on edge 2026-04-16:
  - `services.cairn-companion.spoke = { enable = true;
    tools.notify.enable = true; };` added to edge's `home-manager.users.keith`
    block (not NixOS level — cairn-companion's module is home-manager).
  - `nixos-rebuild switch --flake .#edge` green.
  - `mcp-gw --json list` shows `companion-notify` with status
    `connected` and `enabled: true` after mcp-gateway restart.
  - `companion "send me a desktop notification that says hello from sid"`
    triggered a visible notification on edge's desktop. Confirmed.

**Architecture note recorded during live test:** Keith's mcp-gateway
is centralized — one instance on edge serves the fleet via Tailscale
Serve. Spoke tools at this tier execute wherever the gateway runs
(always edge), regardless of which host the caller sat at. That
distributed-routing limitation is an explicit non-goal for this
change; see the `distributed-routing` Tier 2 phase 2 proposal. The
`services.cairn-companion.spoke` block therefore only belongs in
edge's home-manager config, not in shared user config files.

## Phase 2: `screenshot`

- [x] **2.1** `src/bin/screenshot.rs` with one tool: `screenshot_full`
  (no args). Region and window variants deferred — region requires
  `slurp` for interactive selection (not a flow Sid can drive), and
  window requires `niri msg focused-window` geometry parsing (better
  to land niri tool first). Full-screen is the canonical multimodal
  demo; the other two land in a follow-up.
- [x] **2.2** Shell out to `grim -` (PNG to stdout, no tempfile
  juggling), base64-encode via `base64` 0.22's STANDARD engine, wrap
  in `ok_image(data, "image/png")`.
- [x] **2.3** `default.nix` adds `grim` to `buildInputs` and wraps
  `companion-mcp-screenshot`'s PATH with `grim/bin`.
- [x] **2.4** Home-manager `spoke.tools.screenshot.enable` + auto-
  registration as `services.mcp-gateway.servers.companion-screenshot`.
- [x] **2.5** Pre-deploy stdio smoke: piped `initialize` + `tools/call
  screenshot_full` at the wrapped binary, got valid JSON-RPC
  `ImageContent` with a base64-encoded PNG (verified the data starts
  with `iVBORw0KGgo` = PNG magic). Full end-to-end test (consumer
  rebuilds edge, enables `tools.screenshot.enable = true`, restarts
  mcp-gateway, runs `companion "describe what's on my screen"`)
  pending Keith's rebuild.
- [ ] **2.6** Follow-up: `screenshot_region` (slurp-interactive) +
  `screenshot_window` (niri focused-window geometry). Deferred to a
  later commit; not blocking Phase 2 shipment.

## Phase 3: `clipboard`

- [x] **3.1** `src/bin/clipboard.rs` with `clipboard_read` (no args,
  returns current text) and `clipboard_write` (`text` required).
- [x] **3.2** Read: shell out to `wl-paste -n` (strip trailing
  newline). Empty-clipboard / non-text-payload stderr ("No selection"
  / "No suitable type") surfaces as an empty `ok_text("")` rather
  than an error — "nothing to read" is a valid state.
  Write: spawn `wl-copy` with `stdin=piped, stdout=null, stderr=null`,
  write text to stdin, drop to close pipe, wait for exit. **The null
  redirection on stdout/stderr matters**: wl-copy forks a daemon to
  hold the selection in the background; without the redirection that
  forked daemon inherits the MCP server's JSON-RPC pipe and keeps
  the write end open past our exit, so downstream readers (including
  mcp-gateway) never see EOF and hang.
- [x] **3.3** Home-manager `spoke.tools.clipboard.enable` +
  auto-registration as `services.mcp-gateway.servers.companion-clipboard`.
  `wl-clipboard` added to `buildInputs` and wrapped onto the
  clipboard binary's PATH.
- [x] **3.4** Live stdio smoke on edge: piped write `sid-was-here` +
  read, got back the exact text round-trip. Full mcp-gateway path
  pending Keith's rebuild.

## Phase 4: `journal`

- [x] **4.1** `src/bin/journal.rs` with one tool `journal_read` —
  `unit` (optional), `since` (optional, any `journalctl --since` value),
  `lines` (optional, default 100, max 1000, clamped server-side).
- [x] **4.2** Shells out to `journalctl --user --no-pager
  --output=short -n <lines> [-u <unit>] [--since <value>]`. Capture
  via `.output()`, UTF-8-decode, hand back as `ok_text`. Empty-result
  case ("no matching journal lines") handled explicitly.
- [x] **4.3** `default.nix`: `systemd` joins buildInputs,
  `companion-mcp-journal` wrapped with `systemd/bin` on PATH.
  Home-manager gets `spoke.tools.journal.enable` + auto-registration
  as `services.mcp-gateway.servers.companion-journal`.
- [x] **4.4** Live stdio smoke on edge: `journal_read {unit:
  "companion-core", lines: 3}` returned three real turn-complete
  lines from the companion-core user unit. Full mcp-gateway path
  pending Keith's rebuild.

## Phase 5: `apps`

- [x] **5.1** `src/bin/apps.rs` with two tools: `open_url` (required
  `url`) and `launch_desktop_entry` (required `name`). Both
  fire-and-forget, stdio redirected to null so the forked child
  can't hold the JSON-RPC pipe open past our exit.
- [x] **5.2** `xdg-open` for URLs. Switched from `gtk-launch` to
  `dex -a` for desktop entries because gtk-launch only ships inside
  the full gtk3 package (~30 MB closure for one binary), and dex
  is tiny + purpose-built + has name-based lookup.
- [x] **5.3** Home-manager gets `spoke.tools.apps.enable` +
  auto-registration as `companion-apps`. `xdg-utils` and `dex`
  added to buildInputs and wrapped onto the apps binary's PATH.
- [x] **5.4** Live tests on edge 2026-04-16:
  - `companion "open https://nixos.org in my browser"` → browser
    tab opened. Clean.
  - `companion "launch com.mitchellh.ghostty"` → Ghostty spawned,
    Sid confirmed "Launched. Whatever."
  - Two bugs caught during the live test, both fixed in follow-up
    commits (929a51b, d28bdb0):
    1. mcp-gateway's systemd unit has no `XDG_DATA_DIRS`, so dex
       saw only freedesktop-default paths and missed NixOS per-user
       and system profile share dirs. Fixed by constructing
       XDG_DATA_DIRS in the tool at runtime from $HOME, $USER,
       /etc/profiles/per-user/, /run/current-system/sw/share.
    2. `dex -a` is `--autostart` (runs every ~/.config/autostart/
       entry), not name-based lookup — I misread the flag. First
       live test fired Solaar repeatedly because Solaar was in
       autostart. Fixed by doing the XDG name-to-path resolution
       in Rust (~20 lines, no new deps) and passing the full
       .desktop path to dex as a positional arg (its actual
       contract). Also fixed a secondary bug where systemd
       doesn't set $USER for system services running as a user,
       so the per-user path resolved to `/etc/profiles/per-user/root/`
       — now falls back to deriving username from $HOME's basename.

## Phase 6: `niri`

- [x] **6.1** `src/bin/niri.rs` with seven tools covering both the
  read and write halves of `niri msg`:
  - Read: `niri_windows`, `niri_workspaces`, `niri_focused_window`
    (returns Niri's native JSON; Claude parses directly, no
    double-serialization tax).
  - Write: `niri_focus_window(id)`, `niri_focus_workspace(reference)`
    (index OR name, Niri accepts both), `niri_close_focused`,
    `niri_spawn(command)` (argv array).
- [x] **6.2** Read path shells `niri msg --json <subcommand>`; write
  path shells `niri msg action <subcommand> [args]`. Shared helpers
  `niri_json()` and `niri_action()` keep each tool ~3 lines.
- [x] **6.3** `default.nix`: `niri` added to buildInputs, the niri
  binary gets its own wrapProgram with niri/bin on PATH.
  Home-manager gets `spoke.tools.niri.enable` + auto-registration
  as `services.mcp-gateway.servers.companion-niri`.
- [x] **6.4** Pre-deploy stdio smoke on edge: `niri_workspaces`
  returned real workspace list (3 workspaces on DP-2), `niri_focused_window`
  correctly identified the ghostty window hosting this very
  conversation (title: "⠂ Resume development after rebranding from
  axios to cairn"). Write-path tests (focus / spawn / close) pending
  Keith's rebuild — skipped from the pre-deploy smoke because they
  have visible side effects on an active session.

## Phase 7: `shell`

- [x] **7.1** `src/bin/shell.rs` with one tool `run` — argv array
  (required, min 1), optional `stdin` (bytes piped in), optional
  `timeout_secs` (default 30, max 300, enforced in-process via
  `tokio::time::timeout`), optional `cwd`. argv passed directly to
  `tokio::process::Command` — no shell wrapping, no interpolation
  vector via arguments.
- [x] **7.2** Allowlist via env `COMPANION_SHELL_ALLOWLIST`. Three
  modes represented as an `Allowlist` enum so every call site has
  to handle them:
  - unset / empty → `DenyAll` (safe default)
  - `"*"` alone → `AllowAll` (every call emits WARN in the audit log)
  - `"git,ls,cat"` → `Specific(HashSet)`
  Match is on the basename of argv[0]: `"git"` matches `git` and
  `/usr/bin/git`; `"/usr/bin/git"` matches neither. Any other rule
  is an injection vector waiting to happen.
- [x] **7.3** Home-manager `spoke.tools.shell.enable` +
  `spoke.tools.shell.allowlist`. The list is marshalled into
  `env.COMPANION_SHELL_ALLOWLIST` at module-evaluation time via
  `concatStringsSep ","`.
- [x] **7.4** Audit log to the user journal via `tracing-journald`,
  under the `companion-mcp-shell` identifier. Every invocation
  logs one structured event: argv, allow/deny decision, exit code
  (or timeout), duration. `journalctl --user -t
  companion-mcp-shell` is the operator's audit trail. If journald
  isn't reachable (running outside systemd), init silently falls
  back — tool still works, just without audit.
- [x] **7.5** Six pre-deploy behavioral smoke tests on edge, all
  green:
  1. Deny-all default → `ls` rejected.
  2. Specific allowlist → `echo "allowlist works"` permitted, ran.
  3. Specific allowlist → `rm -rf /` rejected (basename `rm` not
     on `ls,echo`).
  4. Basename matching → `/run/current-system/sw/bin/echo hi`
     permitted by allowlist `["echo"]`.
  5. Timeout → `sleep 60` with `timeout_secs=2` killed at 2s,
     "did not exit within 2s" error surfaced.
  6. Stdin pipe → `cat` with `stdin="hello via stdin"` echoes back.
  Full deploy test (live invocation via mcp-gateway) pending Keith's
  rebuild with a real allowlist chosen.

## Phase 8: paperwork

- [ ] **8.1** Flip ROADMAP `spoke-tools` from `[ ]` to `[x]` with
  shipped date.
- [ ] **8.2** Archive: `mv openspec/changes/spoke-tools
  openspec/changes/archive/spoke-tools`.
