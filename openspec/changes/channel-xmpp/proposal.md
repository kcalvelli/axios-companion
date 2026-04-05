# Proposal: XMPP Channel Adapter

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

Add an XMPP channel adapter for users running their own XMPP server (common in self-hosted setups and families using XMPP instead of proprietary chat platforms). Supports direct messages and MUC (multi-user chat) rooms. Uses `tokio-xmpp` or similar Rust XMPP library.

## Motivation

XMPP is the open federated chat protocol with mature self-hosted server implementations (Prosody, ejabberd). Users who run their own XMPP server for family or small group communication (the typical axios user profile) can integrate their companion as another contact in the same chat system their family already uses — no separate app, no third-party service, no account creation.

## Scope

### In scope

- XMPP adapter inside the daemon using a Rust XMPP library
- Home-manager options under `services.axios-companion.channels.xmpp`:
  - `enable`
  - `jid` — the bot's full JID
  - `passwordFile`
  - `server`, `port`, `sslVerify`
  - `allowedJids` — list of JIDs allowed to DM the bot
  - `mucRooms` — list of MUC room JIDs to auto-join
  - `mucNick` — nickname to use in MUC rooms
  - `mentionOnly` — for MUC rooms, only respond when @mentioned or addressed by nick
- Direct message handling
- MUC room support with nick-based addressing
- Presence management (online when daemon is up)

### Out of scope

- E2EE (OMEMO) — deferred; XMPP plaintext over TLS to a trusted self-hosted server is the initial target
- File transfer
- Voice/video (Jingle)

### Non-goals

- A general XMPP client — the adapter is a bot interface
- Federation routing concerns — the user's XMPP server handles federation

## Dependencies

- `bootstrap`
- `daemon-core`
- `channel-telegram` (for pattern reference)

## Success criteria

1. User configures a JID, password file, and allowlist via home-manager
2. The daemon connects to the XMPP server and shows online presence
3. DMs from allowed JIDs get routed to the dispatcher and receive responses
4. MUC rooms in the auto-join list are joined on startup
5. In MUC rooms, the bot responds when addressed by its nick or @mentioned
6. Each DM JID and each MUC room maps to a persistent Claude session
