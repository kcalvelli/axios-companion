# Proposal: Bootstrap — Tier 0 Shell Wrapper and Home-Manager Module

## Summary

Ship the minimum viable `axios-companion` as a home-manager module exposing a `companion` shell wrapper around `claude -p`. The wrapper loads the user's persona files as a system prompt, attaches the user's workspace directory, and passes through any existing mcp-gateway configuration. No daemon, no channels, no persistent state beyond the workspace. This is the first shippable artifact: after it lands, any axios (or plain NixOS) user with Claude Code installed can enable a few lines of home-manager config and get a personalized AI companion invokable from any terminal.

## Motivation

### Current state

axios users who want a persistent, customizable Claude Code experience today have two options, neither of which is good:

1. **Use `claude` directly with no persona.** Every session is a fresh assistant. No user context loads, no response format rules apply, no workspace is attached. The user either retypes context every time or accepts a generic assistant experience.
2. **Run ZeroClaw/similar heavyweight agent frameworks.** These carry hundreds of thousands of lines of infrastructure for capabilities (multi-provider routing, distributed execution, plugin systems, hardware integration) that a single-user home Linux setup will never use. The fork-maintenance cost for a single user is disproportionate to the benefit.

The gap between "plain `claude`" and "full agent framework" is very wide. Almost all of the value of a personal AI companion comes from a small set of ergonomic wins: persona injection, workspace attachment, and MCP tool availability. These can be delivered as a shell wrapper in ~100 lines of code. Shipping that wrapper as a declarative home-manager module makes it installable with three lines of Nix.

### Why this is the first proposal

A 2026-04-04 proof of concept validated that Claude Code's voice holds across realistic session lengths (5–20 turns covering tool use, errors, pushback, nostalgia, family context, and long-form technical questions) when given persona files via `--append-system-prompt` and a workspace directory via `--add-dir`. The persona scaffolding is doing almost all of the work; no daemon is required to make the experience feel coherent for users whose interactions are typically brief (e.g. spec-driven development sessions and short channel conversations).

That finding means the right first artifact is not a daemon. It is a shell wrapper that makes Claude Code feel like a persistent companion without any persistent process. Every tier after this one is an incremental upgrade on a foundation that already works.

### Proposed solution

Ship three things:

1. **A home-manager module** at `modules/home-manager/default.nix` exposing `services.axios-companion` with options for enabling the module, selecting the Claude Code package, providing a user-specific context file, layering additional persona files, and setting the workspace directory.

2. **A `companion` binary** built as a `pkgs.writeShellApplication` that resolves persona files (default base + user override + extras), concatenates them into a system prompt, locates the user's workspace directory (creating it on first run), and invokes `claude` with the correct flags:
   - `--append-system-prompt "$PERSONA"` — the merged persona content
   - `--add-dir "$WORKSPACE"` — the companion workspace
   - `--mcp-config "$MCP_CONFIG"` — the user's mcp-gateway config (if present)
   - Plus any args the user passed through

3. **Default persona files** at `persona/default/` containing only a minimal `AGENT.md` (response format rules, no character voice) and a `USER.md` template the user fills in with their own context. These are installed to the user's workspace on first run and can be overridden per-user via module options.

### Benefits

1. **Ships a working product on day one.** Every axios user with Claude Code can enable the module and have a personalized companion immediately — no daemon to set up, no Tailscale to configure.
2. **Proves the architecture.** Tier 0 is the smallest viable slice of the overall design. If it works, every later tier is additive; if it doesn't, higher tiers are saved from building on a broken foundation.
3. **Zero runtime cost.** No daemon, no background process, no memory footprint when the user isn't actively invoking it.
4. **Composes with mcp-gateway.** If the user already runs mcp-gateway (as axios users typically do), the wrapper picks up its MCP config automatically, giving the companion access to every tool the user has already declared.
5. **Trivially testable.** A wrapper script has no state, no network dependencies, and no concurrency concerns. Testing is `companion "hello"` and reading the output.

### Trade-offs

1. **No persistent memory across sessions beyond what the workspace git repo provides.** Tier 0 has no database, no session cache, no conversation history. If the user wants memory beyond "files the agent can read and write in the workspace," they need Tier 1. This is acceptable because SDD workflows and short channel-style interactions rarely benefit from long conversational history.
2. **No channels.** Tier 0 is CLI-only. Telegram, Discord, email, and XMPP require the Tier 1 daemon. Users who need channel access must wait for the daemon proposal.
3. **No distributed agency.** The wrapper runs Claude Code locally in whichever terminal invoked it. It cannot take actions on another machine. Tier 2 addresses this.
4. **Workspace sync is the user's problem.** If the user wants their workspace (memory, personal persona files) to be consistent across machines, they sync it themselves (git, syncthing, Tailscale Drive). This proposal does not prescribe a sync mechanism.

## Scope

### In scope

- `modules/home-manager/default.nix` — declarative home-manager module with options:
  - `enable` — boolean
  - `package` — the axios-companion package (default: self overlay)
  - `claudePackage` — the Claude Code CLI package (default: `pkgs.claude-code`)
  - `persona.basePackage` — package containing default persona files (default: self)
  - `persona.userFile` — optional path to user's own `USER.md` override
  - `persona.extraFiles` — list of additional persona markdown files to layer on
  - `workspaceDir` — path to the companion workspace (default: `$XDG_DATA_HOME/axios-companion/workspace`)
  - `mcpConfigFile` — path to mcp-gateway config file (default: auto-detect common locations, null if absent)
- `packages/companion/default.nix` — `pkgs.writeShellApplication` building the `companion` binary
- `persona/default/AGENT.md` — minimal response format rules, no character voice
- `persona/default/USER.md` — template with placeholder sections for the user to fill in
- First-run scaffolding — if the workspace directory does not exist, create it with `README.md`, copy the default `USER.md` template if no user-provided file exists
- `flake.nix` — populate `overlays.default`, `homeManagerModules.default`, `packages.<system>.default`
- Usage documentation appended to `README.md`

### Out of scope

- **Persistent daemon of any kind** (Tier 1 — see `daemon-core` proposal)
- **Channel adapters** (Tier 1 — see `channel-*` proposals)
- **CLI subcommands beyond raw passthrough** (Tier 1 — see `cli-client` proposal)
- **TUI dashboard** (Tier 1 — see `tui-dashboard` proposal)
- **Session history or conversation memory beyond what Claude Code stores in `~/.claude/`** (higher tiers)
- **Multi-machine routing** (Tier 2 — see `distributed-routing` proposal)
- **MCP tool servers for local machine agency** (Tier 2 — see `spoke-tools` proposal)
- **GUI clients** (see `gui-gtk4` proposal)
- **Workspace synchronization across machines** (user's responsibility)
- **Cost tracking, usage ledgers, or rate limiting** (Claude Code handles rate limits; subscription users have no per-token cost to track)

### Non-goals

- **Any authentication layer.** The wrapper uses whatever credentials the user's `claude` CLI is already configured with. No tokens, no OAuth flows, no account management.
- **Character personality in the default persona.** The default `AGENT.md` contains only response format rules (be concise, cite files, no preambles, report tool results tersely). Users who want a character voice provide their own files via `persona.extraFiles`.
- **Any interaction with claude-code's internals.** The wrapper shells out to the `claude` binary exactly as a user would. It does not parse state, inject into the event loop, or depend on undocumented Claude Code behavior.

## Dependencies

None. This is the first proposal and has no predecessors.

## Success criteria

1. A user with a NixOS + home-manager system and Claude Code installed can enable `services.axios-companion` in their home configuration, run `home-manager switch`, and invoke `companion "hello"` successfully.
2. The first invocation creates the workspace directory if it does not exist, with a `README.md` and the default `USER.md` template (unless the user provided one).
3. Persona files resolve in the documented order: base → user override → extras (later files can reference or extend earlier ones via their content; the wrapper just concatenates).
4. If mcp-gateway is running and its config file is at one of the auto-detected paths (or explicitly provided via `mcpConfigFile`), the companion can invoke tools from it.
5. The `companion` binary is a pure passthrough for flags it does not consume — e.g. `companion --resume`, `companion --model claude-haiku-4-5`, `companion -p "prompt"` all work.
6. `nix flake check` passes.
7. A NixOS user who is *not* on axios can install axios-companion from its flake and have the exact same functionality (verify: no axios references in the module code, only in docs).
