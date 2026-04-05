# Proposal: Distributed Routing — Hub/Spoke Multi-Machine Agency

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 2

## Summary

Teach the companion daemon to route tool invocations across multiple mcp-gateway instances reachable over Tailscale, maintaining per-user presence state ("which machine is Keith on right now?") and namespacing tools by host (`laptop.shell_exec`, `desktop.niri_control`, `mini.journal`). When combined with `spoke-tools`, this delivers the core Tier 2 experience: one Sid identity, many physical presences, full agency on whichever machine the user is currently using.

## Motivation

The companion currently (at Tier 1) runs on one machine and can only touch that machine's filesystem and tools. For a user with multiple NixOS machines on a Tailscale network, the experience should be: "whatever machine I'm typing on, that's where Sid acts." Opening Firefox from a Telegram message should open it on my laptop if I'm on the laptop, on my desktop if I'm at the desk, on mini if I'm nowhere and need a fallback. This is the unique value proposition of axios-companion at Tier 2 and is the reason the hub-and-spoke architecture was chosen over a full per-machine-daemon mesh.

## Scope

### In scope

Hub-side changes in `companion-core`:

- `services.axios-companion.daemon.spokes` — map of `hostname → mcp-gateway URL` for known spokes
- MCP client pool — maintain persistent connections to each spoke's mcp-gateway
- Tool namespacing — when the hub exposes tools to claude-code, each remote tool is prefixed with the spoke hostname
- Active-spoke tracking — track which spoke last received user input (via CLI beacon; see below) with a decay timeout
- System-prompt injection — include "Active host: X, available hosts: [...]" in the persona context so Claude knows where to route tool calls
- Routing logic — when Claude invokes a tool like `shell_exec` (unqualified), the hub routes it to the active spoke; qualified tools like `laptop.shell_exec` are routed to the named host

CLI beacon changes:

- `companion` CLI invocations tag their requests with `origin=<hostname>` so the hub knows which spoke the user is on
- The hub updates `active_spoke` on every received message

Home-manager module options:

- `services.axios-companion.daemon.spokes` — static spoke definitions
- `services.axios-companion.daemon.hubRole` — boolean (true if this machine hosts the hub, false if spoke-only)
- On spoke machines, the `companion` CLI is configured to point at the hub's D-Bus or HTTP endpoint over Tailscale
- Hub discovery — simple config for now; nothing dynamic

### Out of scope

- Nomadic hub (hub that migrates between machines) — fixed hub-on-one-machine model
- Full state sync across machines — spokes are stateless tool surfaces; all conversation state lives on the hub
- Delayed/deferred intents ("next time I'm on the laptop, do X") — interesting feature, not v1
- Cross-tenant routing — one user's hub, one user's spokes

### Non-goals

- A generic distributed agent framework — this is purpose-built for "one user, many machines, one companion identity"
- Replacing Tailscale's trust model — Tailscale ACLs are the only network boundary
- Solving CAP theorem problems — if a spoke is unreachable, its tools are unavailable and the hub reports that clearly

## Dependencies

- `bootstrap`
- `daemon-core`
- `spoke-tools` (the spokes need tools to route)
- `cli-client` (the beacon lives in the CLI)

## Success criteria

1. User configures `services.axios-companion.daemon.spokes = { laptop = "http://laptop.tail0efb4.ts.net:8085"; desktop = "..."; mini = "http://localhost:8085"; }` on the hub machine
2. After `home-manager switch`, the hub establishes MCP client connections to each spoke and the tool list available to Claude includes namespaced variants (`laptop.shell`, `desktop.niri_control`, etc.)
3. User runs `companion "echo hello from \$HOSTNAME"` on the laptop; the hub routes the `shell_exec` call to `laptop.shell` and the response shows the laptop's hostname
4. User sends the same message via Telegram (which arrives at the hub) without being at any machine; the hub reports "no active host" and either asks the user to specify or falls back to `mini` as the configured fallback
5. Switching between machines during a single conversation works — the active spoke updates on each new message and subsequent tool calls route to the new machine
6. Spoke unreachability (laptop closed, mini rebooting) is handled gracefully: the hub marks the spoke as unavailable, the tool list excludes its tools, and reconnection happens automatically when the spoke returns
7. Audit log on each spoke captures every tool invocation with timestamp, tool name, and hub origin
