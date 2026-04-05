# Proposal: Email Channel Adapter

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up.

## Tier

Tier 1

## Summary

Add an email channel adapter to the companion daemon, allowing users to email their companion and receive threaded responses. Uses IMAP IDLE for push delivery (no polling) and SMTP for outbound. Follows the pattern established by `channel-telegram`.

## Motivation

Email is the universal async channel. Every user has it, every platform supports it, and it's the natural medium for longer-form interactions, attachments, forwarded content, and delayed responses. Users can cc: their companion on emails to get summaries, forward items to archive in memory, or simply have email-style conversations that don't feel like chat.

## Scope

### In scope

- Email adapter running inside the daemon as an async task
- Home-manager options under `services.axios-companion.channels.email`:
  - `enable`
  - `imap.host`, `imap.port`, `imap.username`, `imap.passwordFile`
  - `smtp.host`, `smtp.port`, `smtp.username`, `smtp.passwordFile`
  - `fromAddress`
  - `allowedSenders` — list of email addresses allowed to send messages (supports wildcards)
- IMAP IDLE-based push delivery (no polling)
- Thread continuity via `In-Reply-To` and `References` headers → persistent `thread_id` → `claude_session_id` mapping
- Outbound email via SMTP with proper MIME structure
- Store sent emails in IMAP Sent folder via APPEND
- Skip no-reply and bounce addresses to prevent loops

### Out of scope

- Calendar integration (use mcp-dav via mcp-gateway)
- Address book sync
- Mail filtering rules (handle at the MUA level)
- Multiple email accounts in one adapter instance

### Non-goals

- Replacing a real mail client — the adapter is a bot interface, not a full MUA
- Managing the user's inbox organization — the adapter only handles messages addressed to its own allowlisted conversation

## Dependencies

- `bootstrap`
- `daemon-core`
- `channel-telegram` (optional — establishes the channel adapter pattern)

## Success criteria

1. User configures IMAP + SMTP credentials via agenix-backed password files
2. After `home-manager switch`, the daemon connects to IMAP IDLE and is push-notified of new messages
3. Messages from allowed senders get routed to the dispatcher and receive threaded responses
4. Reply threading is preserved across multi-turn conversations (subject prefixes, `In-Reply-To` chain)
5. Sent messages are saved to the IMAP Sent folder
6. Bounce and no-reply addresses do not trigger response loops
