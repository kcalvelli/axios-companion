# Proposal: Email Channel Adapter

> **Status**: Code complete 2026-04-10. 32 unit tests passing, `nix build .#companion-core` green. Ready to deploy once an operator configures the home-manager options against a real mailbox.

## Tier

Tier 1

## Summary

Add an email channel adapter to the companion daemon so a user can have the bot listen on its own mailbox and reply via SMTP. Inbound mail is pulled via IMAPS polling, parsed via `mail-parser`, quote-stripped, and dispatched with the RFC 5322 thread root Message-ID as the session key. Outbound replies go through SMTPS via `lettre` with `In-Reply-To` / `References` threading preserved, `Auto-Submitted: auto-replied` set to prevent mail loops, and a copy APPEND'd to the server's Sent folder.

## Motivation

Email is the universal async channel. Every user has it, every platform supports it, and it's the natural medium for longer-form interactions that don't fit the staccato rhythm of a chat window. For a companion bot specifically, email is also the only channel that naturally handles **threads** — a sender can maintain several parallel conversations with the bot without them bleeding into one session the way they would on XMPP DM or Telegram DM.

Equally important: email makes the bot reachable from anywhere. Telegram and XMPP require the sender to already be on those networks; a `mailto:` link works from every device, every OS, every browser, and every MUA ever written.

## Scope clarification

**Channel-email is the bot's own inbox as a message channel.** It is NOT a mechanism for the bot to read other mailboxes on the user's behalf. Those are two different capabilities:

| Capability | What it does | Where it lives |
|---|---|---|
| `channel-email` (this proposal) | Delivers mail addressed TO the configured mailbox into the dispatcher. Replies come back from the same address. | `packages/companion-core/src/channels/email/` |
| Mail-reading MCP tools | Queries other mailboxes the user owns so the bot can read them on the user's behalf during a tool-using turn. | An external MCP server exposed via mcp-gateway. Out of scope for this proposal. |

Conflating these would be a category error: the channel adapter receives messages addressed to the bot, while a mail-reading MCP tool acts on the user's mailboxes from inside a turn. Different trust boundaries, different persistence models, different failure domains.

## Scope

### In scope

- Email adapter running inside the daemon as an async task
- Home-manager options under `services.axios-companion.channels.email`:
  - `enable`
  - `address` — the bot's own mail address
  - `displayName` — optional `From:` header display name
  - `passwordFile` — agenix-compatible single-line password file
  - `imapHost`, `imapPort` (default 993)
  - `smtpHost`, `smtpPort` (default 465)
  - `pollIntervalSecs` (default 30, floored at 5)
  - `allowedSenders` — lowercase-compared address list, controls trust tier not delivery
- IMAPS connection with full CA verification via `webpki-roots` (Mozilla bundle)
- 30-second polling loop against `INBOX`, fetching `UNSEEN` messages via `BODY.PEEK`
- MIME parsing via `mail-parser`, plain-text preferred, HTML-with-tags-stripped as fallback
- Quote stripping (`^\s*>`, "On X wrote:" attribution, "-----Original Message-----" separator)
- Thread-root resolution: `References[0]` → `In-Reply-To` → own `Message-ID`. This becomes `conversation_id`, which gives each email thread its own Claude session via the existing `(surface, conversation_id)` dispatcher session store.
- Per-sender trust assignment: `allowedSenders` → `TrustLevel::Owner`; everyone else → `TrustLevel::Anonymous`
- SMTPS outbound via `lettre` on port 465 (implicit TLS wrapper, not STARTTLS)
- Reply header construction: `Subject: Re: <original>` (preserve existing `Re:` prefix, don't double-stack), `In-Reply-To: <inbound msg-id>`, `References: <original references chain> <inbound msg-id>`, `Auto-Submitted: auto-replied`, `Message-ID: <uuid@domain>`
- Bounce / auto-loop prevention on inbound: drop on `Auto-Submitted != no`, drop on `Precedence: bulk/list/junk`, drop on bounce / `no-reply` / `postmaster` / `mailer-daemon` sender patterns
- Outbound copy APPEND'd to IMAP Sent folder (tries `Sent`, `INBOX.Sent`, `Sent Items`)
- `STORE \Seen` on inbound messages only after `handle_message` has acted on them, so a daemon crash mid-turn causes the message to be reprocessed on the next poll
- Bang commands (`!new`, `!status`, `!help`) — same set as the xmpp adapter, prefixed with `!` because slash commands are intercepted by mail clients
- Exponential-backoff reconnect loop (1s → 60s) on IMAP session-fatal errors
- Shutdown-aware sleep inside the poll loop so `systemctl --user stop companion-core` is snappy

### Out of scope

- **IMAP IDLE push delivery.** Deferred in favor of polling for v1. IDLE is a localized follow-up in `fetch.rs` if latency ever matters.
- **Multiple accounts in one adapter instance.** One inbox per enabled adapter. If a second bot identity is ever needed, add a second `channels.email2` option — don't grow this one.
- **HTML outbound.** Plain-text only. Mail clients render plain text fine, and it sidesteps a whole universe of rendering inconsistencies.
- **Attachments.** Inbound attachments are silently dropped at the parser boundary. Outbound replies never carry attachments.
- **OAuth2 / XOAUTH2.** Password auth only. Gmail-style OAuth would be a separate change that touches the home-manager module, the agenix integration, and both the IMAP and SMTP auth paths.
- **STARTTLS on port 587.** Only SMTPS on 465 is wired today. STARTTLS support would be a localized switch from `Tls::Wrapper` to `Tls::Required` plus a new `smtpTls` enum option.
- **DKIM/SPF/DMARC signing.** The mail server handles outbound signing. The adapter just speaks SMTP submission.
- **Mail filtering rules / folder organization.** Handle at the MUA level or in the server's Sieve setup.
- **Reading other mailboxes the user owns.** That belongs in an MCP tool server, not in this channel adapter.

### Non-goals

- Replacing a real mail client — the adapter is a bot interface, not a full MUA.
- Managing the inbox's organization — we touch the `\Seen` flag on processed messages and APPEND outbound replies to Sent. Nothing else.
- Adding a new auth layer — trust derives from the `From:` header against `allowedSenders`. There are no API keys, tokens, or signatures. For hard delivery rejection, use server-side filtering; the adapter is the wrong layer for it.

## Dependencies

- `bootstrap`
- `daemon-core`
- `channel-telegram` (pattern reference)
- `channel-xmpp` (closer pattern reference — the exponential-backoff reconnect loop, the spawned-per-message handler, and the bang-command handler shape were all copied from xmpp)

## Coexisting with other mailbox-touching processes

If the same operator runs a separate process that also IMAPs into the configured mailbox (a mail-archiving sync worker, an MCP mail tool that polls, etc.), the two will race on `\Seen` flags and both will misbehave. The adapter assumes it owns the mailbox exclusively. Operators deploying both should either point the channel adapter at a dedicated bot mailbox that nothing else touches, or take the conflicting process out of that mailbox before enabling the channel.

## Success criteria

1. User configures `address`, `imapHost`, `smtpHost`, `passwordFile`, and `allowedSenders` via home-manager options. After `home-manager switch`, `systemctl --user status companion-core` shows `active (running)` with the email adapter initialized.
2. A mail sent from an `allowedSenders` address to the configured `address` receives a threaded reply from the bot within ~`pollIntervalSecs` seconds. The reply renders as an in-thread reply in the sender's mail client (not a new thread).
3. A follow-up reply in the same thread continues the same Claude session — the `journalctl` log shows `resume=<uuid>` on the second turn.
4. Two parallel threads from the same sender run as two independent sessions — the session store contains two rows with the same `surface='email'`, different `conversation_id`, different `claude_session_id`.
5. A mail from a non-allowlisted sender receives a reply at `TrustLevel::Anonymous` — the daemon log shows `trust=Anonymous` and the reply contains no tool output (tools were stripped at the model level via the Anonymous permission settings).
6. A mail with `Auto-Submitted: auto-replied` is dropped without a reply (`journalctl` shows `dropping auto-submitted message`).
7. A mail from `mailer-daemon@*` is dropped without a reply.
8. A `!status` bang command from an allowlisted sender returns the session info line, not a dispatched turn.
9. Outbound replies appear in the IMAP Sent folder, threaded correctly.
10. `nix build .#companion-core` is green; `cargo test --bin companion-core channels::email` reports 32/32 tests passing.

## Specs

No separate `specs/` subdirectory for this change. `channel-xmpp` set the precedent — a single self-contained channel adapter doesn't need per-concern spec files when the proposal + tasks cover the behavior end-to-end. If that changes later (e.g. if IDLE support lands as a follow-up with meaningful state-machine complexity), split out `specs/imap-idle/` at that point, not now.
