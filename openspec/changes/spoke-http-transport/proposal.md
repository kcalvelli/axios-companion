# Proposal: Spoke HTTP Transport — Remote Spoke Tools Over Tailscale

> **Status**: Implementation complete — pending end-to-end verification on mini.

## Tier

Tier 2

## Summary

Add an HTTP transport mode to the spoke-tools MCP shell so spoke
binaries can run as standalone HTTP servers on remote hosts. Edge's
mcp-gateway connects to them as HTTP transport entries, making remote
tools appear in the tool list alongside local ones. Host-prefixed
server IDs (`mini-companion-shell`) provide explicit routing — the
LLM picks the right tool by name, no routing layer needed.

## Motivation

Spoke tools currently run as stdio subprocesses of the local
mcp-gateway. They can only act on the machine that runs the gateway.
With companion running per-machine (each with its own daemon and
shared memory), the missing piece is cross-machine tool access: "take
a screenshot on edge" from a Telegram message that hits mini, or "run
a command on mini" from the CLI on edge.

The gateway already supports HTTP transport entries — it just needs
something to connect to. The spoke tools already have a
transport-agnostic `ToolHandler` trait. The gap is a 30-line HTTP
server wrapper and the Nix wiring to run spokes in HTTP mode on
remote hosts.

## Design principle

**Host in the name, not in the arguments.** The LLM picks from a flat
tool list. `mini-companion-shell` is unambiguous. `companion-shell`
with a `host` parameter pushes routing logic into the prompt and fails
silently when the model picks wrong.

## Architecture

```
  edge's mcp-gateway (:8085)
  ├── companion-screenshot     (local, stdio)
  ├── companion-niri           (local, stdio)
  ├── companion-clipboard      (local, stdio)
  ├── companion-shell          (local, stdio)
  ├── mini-companion-shell     (remote, http → mini:18790)
  └── mini-companion-journal   (remote, http → mini:18791)

  mini
  ├── companion-mcp-shell      (HTTP MCP server :18790)
  └── companion-mcp-journal    (HTTP MCP server :18791)
```

No changes to mcp-gateway. The gateway already connects to HTTP
transport servers. The spoke binaries gain an HTTP mode. Config on
edge points at mini's URLs.

## Scope

### In scope

1. **`serve_http` in spoke-tools lib.rs** — HTTP server (axum or
   minimal hyper) wrapping the existing `ToolHandler` trait. Single
   `POST /mcp` endpoint, JSON-RPC request/response, same dispatch
   logic as stdio mode.

2. **Transport selection** — env var (`MCP_TRANSPORT=http`) or CLI
   flag switches between stdio and HTTP mode. Each binary's `main()`
   picks the transport. Bind address/port via env
   (`MCP_HTTP_BIND=0.0.0.0:18790`).

3. **Home-manager module option** for remote spoke services — something
   like:
   ```nix
   services.cairn-companion.spoke.tools.shell.http = {
     enable = true;
     port = 18790;
   };
   ```
   Generates a systemd user service that runs the spoke binary in HTTP
   mode. Tailscale provides the network trust boundary — no additional
   auth.

4. **Documentation / persona context** — fleet layout note so the LLM
   knows which host-prefixed tools map to which machine. The shared
   memory system already handles this (`user_fleet_layout.md`).

### Out of scope

- Changes to mcp-gateway (it already supports HTTP transport)
- Automatic spoke discovery (static config is fine for a 3-machine fleet)
- Authentication beyond Tailscale network trust
- Dynamic port assignment (static ports in config)

### Non-goals

- A generic MCP server framework — this is the minimum HTTP wrapper
  around the existing spoke shell
- Replacing the stdio transport — local spokes stay stdio, remote
  spokes use HTTP

## Dependencies

- `spoke-tools` (shipped)

## Success criteria

1. [x] `MCP_TRANSPORT=http MCP_HTTP_BIND=0.0.0.0:18790 companion-mcp-shell` starts an HTTP server that responds to JSON-RPC over POST — 5 HTTP transport tests passing
2. [ ] Edge's mcp-gateway configured with `mini-companion-shell` as an HTTP transport entry can list and call the tool
3. [ ] A Claude Code session on edge can invoke `mini-companion-shell__run` and the command executes on mini
4. [x] Home-manager module generates systemd user services for HTTP-mode spokes — per-tool http.enable + http.port options
5. [x] Nix flake check passes
6. [x] Spoke tools in stdio mode are unchanged — 6 original tests passing, all binaries compile

## Implementation notes

The `handle_request` function in `lib.rs` already takes `&Value` and
returns `Value` with no IO dependency. `serve_http` wraps it:

```rust
pub async fn serve_http<H: ToolHandler + 'static>(
    handler: H,
    bind: &str,
) -> Result<()> {
    // axum router with single POST /mcp route
    // deserialize body → Value, call handle_request, serialize response
}
```

Each binary's main becomes:

```rust
if std::env::var("MCP_TRANSPORT").as_deref() == Ok("http") {
    spoke_tools::serve_http(handler, &bind).await
} else {
    spoke_tools::serve(handler).await
}
```

The `handle_request` function needs to be made `pub` (currently
`async fn` without visibility). That's a one-word change.

Axum adds one dependency. If that's too heavy, a raw hyper server
works too — the endpoint is one route, one method, no middleware.
