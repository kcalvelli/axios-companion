# axios-companion

A persistent, customizable persona wrapper around [Claude Code](https://docs.claude.com/en/docs/claude-code) — turn your Claude subscription into an AI agent that lives with you across your Linux machines.

> **Part of the axios ecosystem, but axios is not required.** This project ships as a home-manager module that works on any NixOS system with Claude Code installed. It integrates naturally with [axios](https://github.com/kcalvelli/axios) and [mcp-gateway](https://github.com/kcalvelli/mcp-gateway), but neither is a hard dependency.

## What this is

axios-companion is a thin wrapper that gives Claude Code three things it doesn't have out of the box:

1. **A persistent persona.** Response-format rules, user context, and (optionally) a full character voice that the agent adopts in every session. Your agent feels like the same person every time, not a stateless assistant.
2. **A home on your filesystem.** A workspace directory for memory, reference data, and long-lived state that the agent can read, write, and evolve across sessions.
3. **A path to distributed agency.** Optional tiers add a persistent daemon, channel adapters (Telegram/Discord/email/XMPP), a terminal dashboard, and — ultimately — multi-machine tool routing so the agent can act on whichever machine you're currently using.

## What this is NOT

- **Not a new AI model.** Claude Code does all the actual thinking. This is a wrapper.
- **Not a multi-tenant service.** Each user on each machine runs their own isolated instance using their own Claude subscription. There is no shared server, no accounts, no API tokens managed by this project.
- **Not a replacement for Claude Code.** If you want a chat interface to Claude, use Claude Code directly. This project exists for people who want Claude Code to feel like a persistent AI agent with a consistent identity and local agency.
- **Not axios-specific.** Despite the name, nothing here requires axios. The name reflects that this is the canonical companion for axios users — not an axios dependency.

## Core commitments

These are enforced in `openspec/config.yaml` as architectural rules and apply to every change proposal:

- **Wrapper around claude-code, nothing more.** If Claude Code already does it, axios-companion doesn't reimplement it. Auth, tool execution, the agent loop, permission prompts, streaming — all belong to Claude Code.
- **Per-user, home-manager only.** No system-level services, no `sid` system user, no multi-tenant infrastructure. Everything runs under the user's systemd slice with the user's own credentials.
- **Tailscale for network trust.** When machines talk to each other (Tier 2), access control is network-level via Tailscale. No application-level auth, no tokens, no OAuth layer.
- **Personality is opt-in.** The default persona that ships with this repo is deliberately character-free — only response format and citation rules. Users who want a voice bring their own persona files.

## Tiers

axios-companion ships in opt-in tiers. You can stop at any tier and still have a working agent.

| Tier | What you get | What runs |
|------|--------------|-----------|
| **0 — Shell wrapper** | A `companion` command that runs Claude with your persona files and workspace pre-loaded | Nothing persistent. Just a binary. |
| **1 — Single-machine daemon** | Persistent sessions, channel adapters (Telegram/Discord/email/XMPP), CLI with subcommands, TUI dashboard | A user-level systemd daemon |
| **2 — Distributed agency** | The agent can act on whichever machine you're currently using via mcp-gateway + Tailscale; active-spoke routing follows your presence | Hub daemon on one machine + mcp-gateway with companion tool servers on every machine |
| **(optional) GUI** | GTK4/libadwaita desktop app for visual dashboards and memory browsing | Opt-in GUI client |

See [ROADMAP.md](./ROADMAP.md) for the full build order and which OpenSpec proposals ship each tier.

## Getting started (once Tier 0 ships)

> **Not yet functional.** This repo currently contains only OpenSpec proposals. The bootstrap change implements Tier 0. Follow its progress in [openspec/changes/bootstrap/](./openspec/changes/bootstrap/).

After the bootstrap change lands, adding axios-companion to a NixOS + home-manager system will look like this:

```nix
# flake.nix
{
  inputs = {
    axios-companion.url = "github:kcalvelli/axios-companion";
    # ... your other inputs
  };
}

# home-manager configuration
{
  imports = [ inputs.axios-companion.homeManagerModules.default ];

  services.axios-companion = {
    enable = true;
    # Optional: layer your own persona files on top of the minimal default
    persona.userFile = ./my-user-context.md;
    persona.extraFiles = [ ./my-persona-voice.md ];
  };
}
```

Then, from any terminal:

```bash
companion "what do you think of this dumpster fire?"
companion chat                  # interactive session
companion --resume              # continue the last session
```

## Repository layout

```
axios-companion/
├── flake.nix                   # Nix flake exposing the home-manager module
├── ROADMAP.md                  # Tiered build order with links to proposals
├── openspec/
│   ├── config.yaml             # Context, non-goals, and architectural rules
│   └── changes/
│       ├── bootstrap/          # Tier 0: shell wrapper + module + default persona
│       ├── daemon-core/        # Tier 1 foundation
│       ├── cli-client/         # Tier 1 CLI subcommands
│       ├── tui-dashboard/      # Tier 1 terminal dashboard
│       ├── channel-telegram/   # Tier 1 first channel adapter
│       ├── channel-email/      # Tier 1 email adapter
│       ├── channel-discord/    # Tier 1 Discord adapter
│       ├── channel-xmpp/       # Tier 1 XMPP adapter
│       ├── spoke-tools/        # Tier 2 machine-local MCP tool servers
│       ├── distributed-routing/# Tier 2 hub/spoke multi-machine routing
│       ├── gui-gtk4/           # Optional GUI
│       └── axios-integration/  # Thin consumer-side proposal (lives in axios)
```

Each change is a self-contained proposal with `proposal.md`, `specs/` describing behavior, and `tasks.md` with an implementation checklist. Only the `bootstrap` change is fully drafted; the others are skeleton proposals that will be fleshed out when picked up.

## Development workflow

This project follows spec-driven development via [OpenSpec](https://github.com/openspec-dev/openspec):

1. Changes start as proposals in `openspec/changes/<name>/proposal.md`
2. Behavior is specified in `openspec/changes/<name>/specs/`
3. Implementation steps are tracked in `openspec/changes/<name>/tasks.md`
4. Only after proposal + specs + tasks are reviewed is any code written
5. Completed changes are archived to `openspec/changes/archive/`

To work on this project:

```bash
nix develop                     # enter devshell with nixfmt, git, gh
cat ROADMAP.md                  # see what's next
cd openspec/changes/bootstrap   # start with the bootstrap proposal
```

## Related projects

- **[axios](https://github.com/kcalvelli/axios)** — The NixOS-based distribution that axios-companion is primarily designed to integrate with
- **[mcp-gateway](https://github.com/kcalvelli/mcp-gateway)** — MCP server aggregator; serves as the spoke daemon in Tier 2 distributed mode
- **[Claude Code](https://docs.claude.com/en/docs/claude-code)** — The underlying agent runtime this project wraps

## License

MIT — see [LICENSE](./LICENSE).
