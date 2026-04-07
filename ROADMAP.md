# Roadmap

axios-companion is built in tiers. Each tier is a complete, shippable feature set that users can stop at if it meets their needs. Each OpenSpec change in `openspec/changes/` corresponds to one discrete piece of work that can be picked up independently, respecting the dependency ordering below.

This document is the master index. See individual `proposal.md` files for details.

## Tiering philosophy

- **Tier 0** is the minimum viable product — a persona'd shell wrapper around Claude Code. Every axios user gets this on day one.
- **Tier 1** adds a persistent user-level daemon that unlocks channel adapters (Telegram/Discord/email/XMPP), an OpenAI-compatible HTTP gateway for voice and other non-interactive consumers (Home Assistant, etc.), a proper CLI, and a TUI dashboard. Users who want a "Sid is always there" experience on one machine stop here.
- **Tier 2** adds distributed agency — one companion identity that can act on whichever machine the user is currently using, via mcp-gateway over Tailscale. Users with multiple NixOS machines unlock this.
- **Optional** polish layers come after Tier 2 — GUI clients, desktop shell integrations, advanced tooling. Never required.

## The build order

Work proceeds roughly top-to-bottom. Items at the same level can be built in parallel once their dependencies are satisfied.

### Tier 0 — Foundation

- [x] **[bootstrap](./openspec/changes/bootstrap/)** — Shell wrapper, home-manager module, minimal default persona files. Shipped 2026-04-05. Validated end-to-end on `edge` with a full production persona port (Sid Friday, five-file layout). `companion` binary, `lib.${system}.buildCompanion` helper, `homeManagerModules.default`, and character-free default persona are all live. *No dependencies.*

### Tier 1 — Daemon and local surfaces

- [x] **[daemon-core](./openspec/changes/archive/daemon-core/)** — User-level systemd daemon, D-Bus interface, session store. The foundation for every Tier 1+ feature. Shipped 2026-04-06. Validated end-to-end on `edge` — daemon starts via systemd, acquires D-Bus name, routes messages through `companion`, persists session mappings in SQLite, streams responses via signals. *Depends on: bootstrap*
- [x] **[openai-gateway](./openspec/changes/archive/openai-gateway/)** ⚠️ **REQUIRED FOR ZEROCLAW DECOMMISSION** — OpenAI-compatible `/v1/chat/completions` HTTP endpoint inside the daemon. Used by Home Assistant's Conversation integration as Sid's voice backend across every room. Shipped 2026-04-06. Gateway starts alongside D-Bus when enabled via `gateway.openai.enable = true`, serves streaming and non-streaming completions, configurable session policy, 32/32 tests passing. *Depends on: bootstrap, daemon-core*
- [~] **[cli-client](./openspec/changes/cli-client/)** — Rust CLI with subcommands, replacing the Tier 0 shell wrapper (while keeping backward compatibility). Core subcommands shipped 2026-04-07 (send, chat, status, sessions list, surfaces, stdin mode). Deferred: logs, session management, memory, shell completions. *Depends on: bootstrap, daemon-core*
- [x] **[tui-dashboard](./openspec/changes/tui-dashboard/)** — Terminal-native dashboard built on ratatui. Shipped 2026-04-07. Sessions panel, live streaming conversation view, status bar, vim-style navigation, auto-reconnect. Deferred: memory/events/usage panels (need new daemon methods). *Depends on: bootstrap, daemon-core*
- [ ] **[channel-telegram](./openspec/changes/channel-telegram/)** — First channel adapter; establishes the pattern for subsequent adapters. *Depends on: bootstrap, daemon-core*
- [ ] **[channel-email](./openspec/changes/channel-email/)** — IMAP IDLE + SMTP adapter with thread preservation. *Depends on: bootstrap, daemon-core, channel-telegram (pattern reference)*
- [ ] **[channel-discord](./openspec/changes/channel-discord/)** — Discord bot adapter. *Depends on: bootstrap, daemon-core, channel-telegram (pattern reference)*
- [ ] **[channel-xmpp](./openspec/changes/channel-xmpp/)** — XMPP adapter for self-hosted servers. *Depends on: bootstrap, daemon-core, channel-telegram (pattern reference)*

**Tier 1 priority ordering:** `daemon-core` first (foundation), then `openai-gateway` immediately — it restores voice interaction that Sid users depend on today via HA Conversation, and its absence during migration would mean every voice satellite in the house stops working. `cli-client`, `tui-dashboard`, and `channel-telegram` can proceed in parallel after that. Remaining channels (`channel-email`, `channel-discord`, `channel-xmpp`) are new capabilities (not replacements) and can be scheduled based on desire, not urgency.

### Tier 2 — Distributed agency

- [ ] **[spoke-tools](./openspec/changes/spoke-tools/)** — MCP tool servers exposing local machine capabilities (shell, apps, screenshot, clipboard, notify, journal, niri) via mcp-gateway. *Depends on: bootstrap. Standalone-useful even without distributed-routing.*
- [ ] **[distributed-routing](./openspec/changes/distributed-routing/)** — Hub daemon learns to route tool calls to multiple spokes over Tailscale with active-spoke presence tracking. *Depends on: bootstrap, daemon-core, spoke-tools, cli-client*

### Optional polish

- [ ] **[gui-gtk4](./openspec/changes/gui-gtk4/)** — GTK4/libadwaita desktop application for visual dashboards. Explicitly last and optional. *Depends on: bootstrap, daemon-core, cli-client, tui-dashboard*

### Consumer-side (lives in axios repo, not this one)

- [ ] **[axios-integration](./openspec/changes/axios-integration/)** — Thin axios-side wiring that imports this flake and exposes axios-friendly defaults. *Depends on: bootstrap (at minimum). The actual change artifact will live in axios/openspec/changes/, not here.*

## How to pick the next proposal

When you're ready to implement something:

1. Check `ROADMAP.md` (this file) for the current status of each proposal
2. Pick any proposal whose dependencies are all marked complete
3. Open that proposal's directory (`openspec/changes/<name>/`)
4. Read `proposal.md` to understand the motivation, scope, and success criteria
5. If the proposal is still a skeleton, flesh out `specs/` and `tasks.md` before writing any code
6. Implement against `tasks.md`, checking items off as they complete
7. Once all tasks are done and success criteria are met, archive the change to `openspec/changes/archive/<name>/` and update this ROADMAP

## Key non-negotiables (enforced in `openspec/config.yaml`)

Every proposal must honor these rules:

- **Wrapper around claude-code only.** If Claude Code already does it, we don't reimplement it.
- **No separate auth layer.** User's existing claude-code credentials + Tailscale network trust are the only auth mechanisms.
- **Per-user home-manager only.** No system-level NixOS modules for companion runtime.
- **Character-free default persona.** Personality is opt-in via user-supplied files.
- **Tier discipline.** Proposals declare their tier and don't depend on tiers above themselves.

## Estimated total scope

- **Tier 0 alone**: ~200 lines of Nix + shell + two markdown files. Shippable in a single sitting.
- **Tier 1 complete**: ~3,500 LoC Rust + ~350 lines of Nix. Multiple proposals, weeks of work if pursued sequentially. (Increased from original estimate to account for the `openai-gateway` HTTP server component.)
- **Tier 2 complete**: +1,500 LoC Rust (mostly tool servers and hub routing) + ~200 lines of Nix.
- **Optional GUI**: +1,500 LoC Rust for a solid libadwaita app.

Total under 6,000 LoC Rust and ~800 LoC Nix for the entire project — an order of magnitude smaller than comparable agent frameworks, because we delegate everything Claude Code already does.
