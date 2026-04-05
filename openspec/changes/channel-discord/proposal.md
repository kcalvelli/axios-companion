# Proposal: Discord Channel Adapter

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

Add a Discord channel adapter to the companion daemon using `serenity` (Rust Discord library). Similar shape to the Telegram adapter: bot auth, allowlist, per-channel session mapping, streaming responses with chunked messages to respect Discord's 2000-character limit.

## Motivation

Users who live in Discord servers (for communities, family chats, gaming, work coordination) want their companion accessible from the same client they're already using. Discord also supports richer formatting than Telegram (code blocks with syntax highlighting, embeds, threads) that suit technical conversations with the companion.

## Scope

### In scope

- Discord adapter inside the daemon using `serenity` or `twilight`
- Home-manager options under `services.axios-companion.channels.discord`:
  - `enable`
  - `botTokenFile`
  - `allowedUsers` — list of Discord user IDs (snowflakes)
  - `mentionOnly` — for guild channels, only respond when @mentioned
  - `streamMode` — `single_message` or `multi_message` for handling Discord's 2000-char limit
- Features:
  - DMs and guild channel messages
  - Code block detection and proper formatting in responses
  - Thread support — a Discord thread maps to a Claude session
  - Image attachment handling (pass to Claude as multimodal input where supported)

### Out of scope

- Slash commands (v2+)
- Voice channel integration
- Server management features (kick, ban, role assignment)

### Non-goals

- A Discord bot framework — this is a single-purpose adapter
- Replacing Discord's UI

## Dependencies

- `bootstrap`
- `daemon-core`
- `channel-telegram` (for pattern reference)

## Success criteria

1. User configures a Discord bot token file and allowlist via home-manager
2. The bot responds to DMs from allowed users and to @mentions in guild channels (when `mentionOnly = true`)
3. Long responses are chunked cleanly across multiple messages without breaking code blocks
4. Each DM thread and each guild channel conversation maps to a persistent Claude session
5. Code blocks in responses use Discord's triple-backtick syntax highlighting
