# Proposal: Telegram Channel Adapter

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

Add a Telegram channel adapter to the companion daemon, allowing users to chat with their companion via a personal Telegram bot. The first channel adapter — establishes the pattern for subsequent Discord, email, and XMPP adapters, and delivers the most commonly-requested remote access path for mobile users.

## Motivation

Telegram is the most accessible remote interface for a personal AI companion: mobile clients on every platform, rich media support, instant delivery without polling, free, and a well-documented bot API. Most axios-companion users will want to chat with their companion from a phone while away from their desk. Telegram is the highest-value first channel for that reason, and it's the most mature ecosystem in Rust (via `teloxide`).

## Scope

### In scope

- `companion-channel-telegram` module inside the daemon (not a separate process — runs as an async task inside `companion-core`)
- Home-manager options under `services.axios-companion.channels.telegram`:
  - `enable`
  - `botTokenFile` — path to file containing bot token (agenix-compatible)
  - `allowedUsers` — list of Telegram user IDs allowed to DM the bot (deny-by-default)
  - `mentionOnly` — for group chats, only respond when @mentioned
  - `streamMode` — one of `single_message` (edit in place) or `multi_message` (split long responses)
- Features:
  - Receive text messages and forward to the daemon dispatcher
  - Stream Claude responses back to Telegram with live message editing for progress indication
  - Handle Telegram's 4096-char message limit by splitting long responses
  - Persist `chat_id` → `claude_session_id` mapping in the session store
  - Respect allowlist: reject messages from unauthorized users silently or with a polite rejection
  - Support voice messages via transcription (deferred if transcription MCP tool not available)

### Out of scope

- Discord, email, XMPP adapters (separate proposals, same pattern)
- Bot management (creating the bot, setting commands, etc.) — user does that via BotFather
- Inline mode, keyboard buttons, or custom Telegram UI features (v2+)

### Non-goals

- Replacing Telegram's UI — responses render in Telegram's native chat view
- Multi-bot support — one bot per user, one token
- Group chat management beyond mention-only mode

## Dependencies

- `bootstrap`
- `daemon-core`

## Success criteria

1. User configures `services.axios-companion.channels.telegram.enable = true` with a bot token file and an allowlist
2. After `home-manager switch`, the daemon establishes a connection to Telegram and the bot responds to messages from allowed users
3. Messages from users not in the allowlist are ignored (or rejected with a configurable message)
4. Long responses are split into multiple Telegram messages without breaking mid-word
5. Each Telegram chat maps to a persistent Claude session that resumes correctly across daemon restarts
6. The pattern established here is documented in a "writing a new channel adapter" section of the README, making subsequent channel proposals straightforward
