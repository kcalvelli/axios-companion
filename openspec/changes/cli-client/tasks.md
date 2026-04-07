# Tasks: cli-client

## Completed

- [x] Create `packages/cli-client/` Rust package with clap + zbus
- [x] Implement D-Bus proxy matching daemon's `org.axios.Companion1` interface
- [x] Default send mode: `companion "prompt"` streams via D-Bus signals
- [x] Interactive REPL: `companion chat` with streaming responses
- [x] Stdin mode: `companion -` reads from stdin
- [x] `companion status` — daemon health display
- [x] `companion sessions list` — tabular session listing
- [x] `companion surfaces` — active surface listing
- [x] Nix package (`default.nix`, flake output, Nix build verified)
- [x] Home-manager integration: `cli.enable`, `cli.package` options
- [x] PATH conflict resolution: CLI as `companion`, wrapper as `companion-raw`
- [x] Assertion: `cli.enable → daemon.enable`

## Deferred

- [ ] `companion logs [-f] [--surface <name>]` — needs daemon-side log streaming D-Bus method
- [ ] `companion sessions show <id>` — needs `GetSession(id)` D-Bus method
- [ ] `companion sessions resume <id>` — needs `ResumeSession(id)` D-Bus method
- [ ] `companion sessions delete <id>` — needs `DeleteSession(id)` D-Bus method
- [ ] `companion memory list|show|edit` — needs workspace path discovery
- [ ] Shell completions via `clap_complete` for bash/fish/zsh
- [ ] Passthrough mode: unknown flags forwarded to daemon's SendMessage
