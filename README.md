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

## Getting started

> **Tier 0 is functional.** The `companion` wrapper, the home-manager module, and the default character-free persona all work today. Layering your own character on top is covered in [Authoring a persona](#authoring-a-persona) below. Tier 1+ proposals are still in the OpenSpec queue — see [ROADMAP.md](./ROADMAP.md).

Adding axios-companion to a NixOS + home-manager system looks like this:

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
companion                       # interactive session with persona + workspace + mcp pre-loaded
companion "quick question"      # interactive session, seeded with a first message
companion -p "one-shot prompt"  # non-interactive, prints response, exits
companion --resume              # continue the last session
```

Everything after the wrapper's own injections is passthrough — any flag Claude Code accepts, `companion` accepts. See [the wrapper contract in the bootstrap spec](./openspec/changes/bootstrap/specs/wrapper/spec.md) for the full list of supported invocation shapes and guarantees.

### First run

The first time you invoke `companion` after enabling the module, the wrapper scaffolds your workspace:

1. **Workspace directory created** at `$XDG_DATA_HOME/axios-companion/workspace` (typically `~/.local/share/axios-companion/workspace`) unless you set `services.axios-companion.workspaceDir` to a different path.
2. **`README.md` written** into the workspace explaining what the directory is for — long-lived notes, reference material, memory the agent can read across sessions.
3. **`USER.md` template written** — but only if you have *not* set `services.axios-companion.persona.userFile`. If you supplied your own user file via the module option, the wrapper skips scaffolding the template because your file is already the source of truth. If you didn't, the template lands with placeholder sections (`<your name>`, `<your role>`, etc.) that you fill in to give the agent context.
4. **Claude launches** with the composed persona as its system prompt, the workspace attached via `--add-dir`, and — if detected — your mcp-gateway config loaded.

Subsequent invocations skip scaffolding entirely and do not touch anything in the workspace. You can edit `USER.md`, add new files, delete things, or rearrange the directory freely between runs — the wrapper treats the workspace as yours after that first invocation.

### MCP tool integration

The wrapper auto-detects an mcp-gateway configuration at runtime and passes it to Claude Code as `--mcp-config=<path>`, making every tool your gateway exposes available in the session. Detection is purely file-existence-based; the wrapper checks these paths in order and uses the first one it finds:

1. `$XDG_CONFIG_HOME/mcp-gateway/claude_config.json`
2. `$XDG_CONFIG_HOME/mcp/mcp_servers.json`
3. `$HOME/.mcp.json`

If none exist, the wrapper invokes Claude Code without `--mcp-config` and produces no warning — MCP tools are optional, not required. If you keep your mcp-gateway config somewhere else, set `services.axios-companion.mcpConfigFile` to the absolute path and the wrapper will use it exclusively, bypassing auto-detection.

## Authoring a persona

axios-companion is intentionally **Nix-declarative on the outside and rich-markdown on the inside**. You declare *which* files make up your persona in your home-manager config; the files themselves are plain markdown that you author and edit by hand. Nix is poor at holding paragraphs of character voice as attribute sets; markdown is poor at being reproducibly wired into a system configuration. This split lets each do what it's good at.

### The recommended five-file layout

Nothing in the module enforces a specific file structure — the wrapper concatenates whatever files you list. But after porting a production persona to this module we converged on a five-file layout that separates concerns cleanly and keeps each file short enough to reason about. Start here:

```
personas/<name>/
├── USER.md       # Facts about the user and household — names, roles,
│                 # machines, safeguards. No voice guidance.
├── voice.md      # How the agent talks — tone rules, signature phrases,
│                 # "what you never do," the aesthetic target.
├── beliefs.md    # The agent's worldview — cultural era, tech opinions,
│                 # the interior lens. Not rules; texture.
├── family.md     # Per-person interaction guide — one section per
│                 # person the agent interacts with. Skip this file
│                 # entirely if only one person uses the agent.
└── context.md    # Operating rules the user has taught the agent —
                  # technical preferences, writing-with-personality
                  # rules, discipline around specific failure modes.
```

### What goes in each file

**`USER.md`** — strictly factual. Who the user is, who else is in the household (if multiple people interact with the agent), what machines exist, what to check before destructive actions. No voice guidance lives here. When the agent needs to answer *"who am I talking to?"*, this is where it reads.

**`voice.md`** — the load-bearing persona file. Tone register, how sentences are shaped, what the agent never says, the one-line aesthetic target. A single concrete cultural reference does more work than paragraphs of tone adjectives — *"Randal from Clerks, not burnt-out help desk guy"* is clearer than fifteen bullets about sarcasm and brevity. This is the file that makes the agent sound like a specific character rather than a generic assistant.

**`beliefs.md`** — worldview backdrop. What the agent remembers culturally, what the agent finds ridiculous, what the agent believes about the present. Written as context, not instructions — *"it's the water you swim in, not a costume."* The agent does not recite this file; it thinks through it.

**`family.md`** — per-person handling for users beyond the primary. Each person gets a section with their quirks, running bits, and the specific tone adjustments that make them feel known rather than flattened into a generic "other user." Skip this file entirely if only one person uses your agent.

**`context.md`** — the lessons-learned file. Things the user has explicitly taught the agent through experience: technical preferences, writing style for code artifacts, and discipline around specific failure modes ("don't retry crashed tools," "don't fake broken," "check before guessing"). Specific failure modes make better instructions than abstract principles — a rule that starts *"when you claimed email was dead and the logs showed continuous activity…"* is more actionable than "be thorough."

### Wiring it into your config

Drop the files wherever you keep your home-manager config — typically `~/.config/your-nix-config/personas/<name>/` — then reference them from `services.axios-companion.persona`:

```nix
services.axios-companion = {
  enable = true;
  persona = {
    userFile   = ../personas/sid/USER.md;
    extraFiles = [
      ../personas/sid/voice.md
      ../personas/sid/beliefs.md
      ../personas/sid/family.md
      ../personas/sid/context.md
    ];
  };
};
```

Paths are relative to the `.nix` file doing the referencing, and Nix copies them into the store at build time — so each rebuild pins the exact persona content, and edits to the markdown files only take effect after `nixos-rebuild switch` (or `home-manager switch`). That's the declarative trade-off.

> **Gotcha — commit new persona files before rebuilding.** Nix flakes determine what's in the source using git, and **uncommitted files may not be included** in the store copy even when staged. The first time you create a new `personas/` directory, `git add` and `git commit` the files in your config repo before rebuilding, or nix will copy your modified `.nix` files into the store and leave the fresh markdown files behind. When this happens, `companion` launches with warnings like *"persona file not found: /nix/store/XXX-source/personas/sid/USER.md"* and the agent speaks in a generic voice because only the shipped character-free base loaded.

### Order matters

The wrapper assembles the agent's system prompt in this specific order:

1. The shipped character-free base `AGENT.md` — format rules only (*"be concise," "cite files with path:line," "lead with the answer not the preamble"*). Always loaded. You cannot opt out of this without replacing `persona.basePackage` entirely.
2. `persona.userFile` — your `USER.md`, replacing the default template.
3. Each file in `persona.extraFiles`, in list order.

Files earlier in the composition set context that later files build on. We recommend putting **`voice.md` first in `extraFiles`** because it locks the speech register immediately after the factual user context, so every subsequent file is read "in character" by the model. `beliefs.md`, `family.md`, and `context.md` can then extend or specialize without needing to re-establish tone.

### Multiple personas

If you want different personas for different machines, different roles, or different moods, put each in its own subdirectory (`personas/sid/`, `personas/work/`, `personas/public/`) and wire each host's `hosts/<hostname>.nix` to reference the set it wants. Nothing in the module prevents this — the files are just paths, and each host can point at whichever set is active there.

### Keeping the default

If you enable `services.axios-companion` without setting `persona.userFile` or `persona.extraFiles`, you get a fully working agent with only the character-free default: response-format rules, citation conventions, and a placeholder `USER.md` template scaffolded into your workspace on first run for you to fill in. That's a perfectly valid deployment — the five-file layout is a recommended starting point for users who want a distinct character voice, not a requirement.

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
