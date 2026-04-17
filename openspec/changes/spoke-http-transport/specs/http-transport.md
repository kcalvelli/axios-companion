# Spec: HTTP Transport for Spoke Tools

## Transport selection

Env var `MCP_TRANSPORT` controls which transport the binary uses:

| Value    | Behavior                              |
|----------|---------------------------------------|
| `http`   | Bind HTTP server, serve `POST /mcp`   |
| (other)  | Stdio loop (existing behavior)        |

`MCP_HTTP_BIND` sets the listen address for HTTP mode. Default: `127.0.0.1:0`.

## HTTP protocol

MCP Streamable HTTP transport (2025-03-26 spec). Single endpoint.

### `POST /mcp`

- Request body: JSON-RPC 2.0 message
- Response: JSON-RPC 2.0 response, `Content-Type: application/json`
- `initialize` response includes `Mcp-Session-Id` header (generated server-side)
- Subsequent requests that include `Mcp-Session-Id` get it echoed back
- Notifications (`notifications/initialized`, `notifications/cancelled`) return 202 Accepted with empty body

### `GET /mcp`

Returns 405 Method Not Allowed. SSE streaming is not implemented — spoke tools are request/response only.

## Session management

Minimal. Session IDs are generated from timestamp + counter (no external dependency). No session state is tracked — spoke tools are stateless. The ID exists solely for protocol compliance with the MCP SDK's `streamable_http_client`.

## Home-manager options

Each tool gains:

```nix
spoke.tools.<name>.http.enable  # bool, default false
spoke.tools.<name>.http.port    # port, default varies per tool
```

Default ports:
- shell: 18790
- journal: 18791
- notify: 18792
- screenshot: 18793
- clipboard: 18794
- apps: 18795
- niri: 18796

When enabled, generates a systemd user service `companion-mcp-<name>-http` that runs the binary with `MCP_TRANSPORT=http` and `MCP_HTTP_BIND=0.0.0.0:<port>`.

## Gateway configuration (consumer-side)

On the machine running mcp-gateway, register the remote spoke:

```nix
services.mcp-gateway.servers.mini-companion-shell = {
  enable = true;
  transport = "http";
  url = "http://mini:18790/mcp";
};
```

This is configured on the gateway host, not in this module.
