# Tasks: Email Channel Adapter — Tier 1

Third channel adapter, after telegram and xmpp. Mirrors the xmpp pattern (exponential-backoff reconnect loop, spawn-per-message handler, bang commands, dispatcher `TurnRequest` with explicit `TrustLevel`) but skips the streaming machinery — email isn't interactive, so the adapter collects the full dispatcher reply and sends it in one SMTP message.

The adapter assumes it owns the configured mailbox exclusively. If something else also IMAPs into the same inbox they will race on `\Seen` flags — deployers should configure a dedicated bot mailbox or remove the conflicting process before enabling.

## Phase 1: Dependencies and skeleton

- [x] **1.1** Pick the Rust email stack. **DECIDED 2026-04-10**: `async-imap` 0.10 (tokio runtime) + `lettre` 0.11 (SMTPS, rustls, builder) + `mail-parser` 0.11 (MIME + header inspection) + `webpki-roots` 0.26 (Mozilla CA bundle for TLS verification).
- [x] **1.2** Add the four dependencies to `packages/companion-core/Cargo.toml` with `default-features = false` and the minimum feature set each needs. lettre gets `smtp-transport`, `tokio1-rustls-tls`, `rustls-tls`, `builder`, `hostname`. async-imap gets `runtime-tokio`. mail-parser gets defaults off. webpki-roots is a plain dep.
- [x] **1.3** Create `src/channels/email/` directory with five files: `mod.rs` (entry point, serve loop, handle_message), `config.rs` (EmailConfig + from_env), `parse.rs` (MIME + threading + quote strip), `fetch.rs` (IMAP connect/poll/fetch), `send.rs` (SMTP + Sent APPEND), `command.rs` (bang commands). Pattern mirrors `channels/xmpp/` with the XMPP's single mod.rs split into concerns because email has more distinct IO surfaces (IMAP + SMTP) than xmpp does.
- [x] **1.4** Register the module in `channels/mod.rs` as `pub mod email;`.
- [x] **1.5** `cargo check -p companion-core` green on the skeleton. First pass hit two issues: (a) lettre's `.header()` takes a typed `Header` impl, not a raw `HeaderValue` — fixed by defining one-line wrapper structs via a macro for `Message-ID`, `In-Reply-To`, `References`, `Auto-Submitted`. (b) async-imap's `session.append()` takes 4 args not 2 — fixed by passing `None::<&str>, None::<&str>` for flags and datetime.

## Phase 2: Configuration

- [x] **2.1** `EmailConfig` struct in `config.rs` with fields: `address`, `display_name`, `password`, `imap_host`, `imap_port`, `smtp_host`, `smtp_port`, `poll_interval`, `allowed_senders: HashSet<String>` (lowercased). `display_name` defaults to the local part of `address` if unset.
- [x] **2.2** `EmailConfig::from_env()` reads `COMPANION_EMAIL_ENABLE`, `_ADDRESS`, `_DISPLAY_NAME`, `_PASSWORD_FILE`, `_IMAP_HOST`, `_IMAP_PORT`, `_SMTP_HOST`, `_SMTP_PORT`, `_POLL_INTERVAL_SECS`, `_ALLOWED_SENDERS`. Returns `None` on missing required fields, logging each failure at `error!` level. Address must contain `@` or the config is rejected. Poll interval is floored at 5 seconds.
- [x] **2.3** Password is read from `COMPANION_EMAIL_PASSWORD_FILE` (agenix-managed). Empty file → error → return None. Path read failure → error → return None.
- [x] **2.4** `is_allowed(&self, from_address)` method: lowercase comparison against the allowlist. Empty allowlist returns false for every sender — those messages still get processed, they just land at `TrustLevel::Anonymous` instead of `Owner`.
- [x] **2.5** Unit tests for `parse_allowed_senders` (lowercases, drops invalid `@`-free entries, handles empty input), `is_allowed` (case-insensitive match, empty allowlist). 5 tests total, all passing.

## Phase 3: MIME parse, threading, quote strip, loop detection

- [x] **3.1** `parse::parse(raw: &[u8]) -> Option<ParsedMessage>` walks the raw RFC 5322 bytes via `mail-parser`. Extracts `message_id` (angle-bracketed), `from_address` (lowercased), `from_raw` (original case + display name), `subject`, `body_text`, `references_raw`, and flags for `is_auto_submitted` / `is_bounce_or_no_reply`. Returns `None` if the bare minimum (From, some kind of body) is missing.
- [x] **3.2** `header_text` and `header_message_ids` helpers. The second one is load-bearing: mail-parser stores `References` and `In-Reply-To` as `TextList` with the angle brackets **stripped**, but downstream code in this module works on `<id>` tokens. `header_message_ids` re-adds the brackets at the boundary. Missing this was the cause of the one test failure on the first test run (`parse_reply_uses_references_for_thread_root`).
- [x] **3.3** `extract_body_text` walks the MIME tree: prefer the first `text/plain` via `msg.body_text(0)`, fall back to `text/html` via `msg.body_html(0)` with a naive `html_strip_tags` pass that also drops `<script>` and `<style>` content wholesale.
- [x] **3.4** `strip_quoted(body: &str) -> String` drops `^\s*>`-prefixed lines and truncates at `On X wrote:` / `-----Original Message-----` separators. Preserves internal blank lines. Trims leading/trailing blank lines. If the stripped body is empty (a pure forward quote), the caller bails before dispatch.
- [x] **3.5** `resolve_thread_root(references, in_reply_to, own_message_id) -> String`. Per RFC 5322, the first id in `References` is the thread root. Falls back to `In-Reply-To` (first id), then to the message's own `Message-ID` (which means this message starts a new thread). Returns the bracketed form.
- [x] **3.6** `is_auto_submitted`: true if the `Auto-Submitted:` header is present and not `"no"`. RFC 3834 compliance, prevents vacation-responder loops.
- [x] **3.7** `is_bounce_or_no_reply`: sender local-part matches `mailer-daemon`, `postmaster`, `no-reply`, `noreply`, `do-not-reply`, `donotreply`, or starts with `bounce`; OR the `Precedence:` header is `bulk`/`list`/`junk`.
- [x] **3.8** Unit tests for everything in parse.rs — 22 tests covering quote stripping (5 variants), message-id extraction (3 variants), thread-root resolution (3 variants), bounce detection, auto-submitted detection (including the `"no"` non-loop case), precedence detection, HTML tag stripping, and two end-to-end parse tests against real RFC 5322 byte fixtures. All passing.

## Phase 4: IMAP fetch

- [x] **4.1** `build_tls_config()` in `fetch.rs` builds a rustls `ClientConfig` seeded with `webpki_roots::TLS_SERVER_ROOTS`. This is the **real** verifier path — the email adapter deliberately does not reuse the xmpp connector's no-verify shortcut. Public mail servers have real certs; if yours doesn't, fix that before enabling this channel.
- [x] **4.2** `connect_and_login(&EmailConfig) -> Result<ImapSession, EmailError>`: TCP connect, TLS handshake via `tokio_rustls::TlsConnector`, `async_imap::Client::new()` wrapping the TlsStream, read the IMAP greeting, LOGIN, SELECT INBOX. `ImapSession` type alias pins the generic to `Session<TlsStream<TcpStream>>` so it doesn't leak into mod.rs signatures. rustls crypto provider is installed as a no-op if already done by the xmpp adapter at startup.
- [x] **4.3** `fetch_unseen(&mut ImapSession) -> Result<Vec<RawMessage>, EmailError>`: `UID SEARCH UNSEEN` → sort UIDs ascending → `UID FETCH <set> (UID BODY.PEEK[])` → drain the response stream, collecting `(uid, body)` pairs. `BODY.PEEK[]` is the critical bit — it avoids implicitly setting `\Seen`, which lets `handle_message` set it explicitly only after the reply has been sent.
- [x] **4.4** `mark_seen(&mut ImapSession, uid: u32)`: `UID STORE <uid> +FLAGS (\Seen)`. Drains the response stream so subsequent commands don't trip on leftover data.

## Phase 5: SMTP send + Sent APPEND

- [x] **5.1** `build_reply(config, inbound, body)`: assembles a `lettre::Message` with typed From/To/Subject plus four custom raw-header wrappers (`RawMessageId`, `RawInReplyTo`, `RawReferences`, `RawAutoSubmitted`) defined via a local `raw_header_struct!` macro. Returns an `OutboundMessage` carrying both the parsed `lettre::Message` (for SMTP submit) and its `formatted()` bytes (for IMAP APPEND) to avoid double serialization.
- [x] **5.2** `re_prefixed(subject)` preserves an existing `Re:` prefix (case-insensitive) and does not double-stack. Stacked `Re: Re: Re:` chains from the sender are preserved verbatim — that's a mail-client choice, not ours to second-guess.
- [x] **5.3** `generate_message_id(address)` builds a UUID-based ID rooted at the bot's domain. Falls back to `@local` for malformed addresses. Used as the outbound `Message-ID:` header.
- [x] **5.4** `send_smtp` constructs an `AsyncSmtpTransport<Tokio1Executor>` via `builder_dangerous(host).port(port).tls(Tls::Wrapper(TlsParameters::new(host)))` — implicit TLS on port 465. `builder_dangerous` is the no-default-TLS constructor; we add TLS back explicitly, which is the correct path for SMTPS. Credentials are the same `address`/`password` the IMAP side uses.
- [x] **5.5** `append_to_sent` opens a one-shot IMAP session (separate from the polling session — threading it through is more bookkeeping than it's worth at this traffic level), logs in, tries `APPEND` to `Sent` / `INBOX.Sent` / `Sent Items` in that order, logs out. Missing Sent folder is a warning, not an error — the reply was still delivered.
- [x] **5.6** Unit tests for `re_prefixed` (added / not double-stacked / preserves existing chain) and `generate_message_id` (uses address domain / falls back to local). 5 tests, all passing.

## Phase 6: Bang commands

- [x] **6.1** `command::handle(surface_id, conversation_id, text, dispatcher) -> String` parses `!new` / `!status` / `!help` and returns the reply text as a plain string. Unrecognized bangs return a deflection reply (`"Not a command. Try !help if you're lost."`) rather than falling through to the dispatcher — prevents Claude Code skill leakage on typos.
- [x] **6.2** `!new` calls `dispatcher.store().await.delete_session(surface_id, conversation_id)`. Reply text varies by whether there was a session to delete.
- [x] **6.3** `!status` reuses `crate::channels::util::format_timestamp` (same helper xmpp's `/status` uses) to format `last_active_at` as "Xm ago" / "Xh ago" / "Xd ago".
- [x] **6.4** `!help` is a static string enumerating the three commands and noting that anything else goes to the dispatcher.

## Phase 7: Serve loop and handle_message

- [x] **7.1** `serve(dispatcher, config, shutdown)` runs the outer reconnect loop with exponential backoff 1s → 60s, capped at 60s, reset to 1s on a clean session end. Shutdown-aware sleep during the backoff window. Modeled directly on xmpp's pattern.
- [x] **7.2** `run_session` opens an IMAP session via `fetch::connect_and_login` then enters an inner poll loop: `fetch_unseen` → iterate → `handle_message` → `mark_seen` → sleep `poll_interval`. Shutdown fires inside the sleep (not in `fetch_unseen`) so a stop signal can wait up to one `fetch_unseen` round-trip but no longer.
- [x] **7.3** `handle_message` wires the Phase 3-6 pieces together: parse, drop if auto-submitted or bounce-pattern, assign `TrustLevel` via allowlist, quote-strip, bail on empty stripped body, branch on bang-prefix for command vs dispatch, build `TurnRequest { surface_id: "email", conversation_id: thread_root, ... }`, drain the dispatcher receiver into a single reply string via `collect_reply`, send SMTP, APPEND Sent. Per-message errors are logged and swallowed — one bad message must not kill the poll loop.
- [x] **7.4** `collect_reply` drains `mpsc::Receiver<TurnEvent>` accumulating `TextChunk` payloads until `Complete` (which carries the canonical full response and is preferred over the streamed accumulation) or `Error` (which becomes a `"Something went sideways on this end: {e}"` reply). Email is not streaming.

## Phase 8: Wiring (main.rs + home-manager)

- [x] **8.1** Add step 6d to `packages/companion-core/src/main.rs`: env-gated bootstrap of the email adapter, shared `Arc<Dispatcher>`, dedicated `email_shutdown` Notify. Info log on startup covers address, imap/smtp host:port, poll interval, and allowlist count.
- [x] **8.2** Add the email adapter to the graceful shutdown sequence (notify + await).
- [x] **8.3** Add `services.axios-companion.channels.email` options block to `modules/home-manager/default.nix`: `enable`, `address`, `displayName`, `passwordFile`, `imapHost`, `imapPort` (default 993), `smtpHost`, `smtpPort` (default 465), `pollIntervalSecs` (default 30, between 5 and 3600), `allowedSenders`. Option descriptions are prose-rich per the project's house style.
- [x] **8.4** Add the daemon-requirement assertion: `channels.email.enable -> daemon.enable`, matching telegram and xmpp.
- [x] **8.5** Marshal the options into `COMPANION_EMAIL_*` environment variables in the systemd unit's `Environment=` block, parallel to telegram and xmpp.
- [x] **8.6** `cargo build` green, `cargo test --bin companion-core channels::email` reports 32/32 passing, `nix flake check` green, `nix build .#companion-core` green end-to-end. (First nix build failed because the new `email/` directory was untracked and nix's git source filter couldn't see it — fixed by `git add`'ing the new files.)

## Phase 9: Deploy, live test, docs, archive

- [x] **9.1** README channel adapters section updated with the email block, configuration example, and email-specific notes.
- [x] **9.2** Proposal + tasks fleshed out (this document).
- [x] **9.3** Operator-side: provision a dedicated bot mailbox on the IMAP/SMTP server, store its password in a single-line file readable by the user the daemon runs as (agenix is the canonical mechanism on axios), and set `services.axios-companion.channels.email` in home-manager. Confirm nothing else is polling the same mailbox before enabling.
- [x] **9.4** Live test, allowlisted sender: send a mail to the bot from an address in `allowedSenders`. Expect a threaded reply within `pollIntervalSecs` seconds, rendered as an in-thread reply (not a new thread) by the sending mail client.
- [x] **9.5** Live test, multi-turn in same thread: reply to the bot's reply. Expect the same Claude session continued; `journalctl --user -u companion-core` should show `resume=<uuid>` on the second turn.
- [x] **9.6** Live test, parallel threads: send a second mail with a different subject. Expect a new Claude session — verify with `sqlite3 ~/.local/share/axios-companion/sessions.db "select surface, conversation_id, claude_session_id from sessions where surface = 'email'"` that two rows exist with different `conversation_id` and `claude_session_id` values.
- [x] **9.7** Live test, anonymous sender: from an address NOT in `allowedSenders`, send a mail. Expect a reply at `TrustLevel::Anonymous` with no tool calls; `journalctl` should show `trust=Anonymous`.
- [x] **9.8** Live test, auto-loop: send a mail with `Auto-Submitted: auto-replied`. Expect no reply; log shows `dropping auto-submitted message`.
- [x] **9.9** Live test, bang command: send `!status` from an allowlisted address. Expect the status line reply, not a dispatched turn.
- [x] **9.10** Sent folder check: confirm outbound replies appear in the server's Sent folder.
- [x] **9.11** ROADMAP.md flipped: `channel-email` marked `[x]` with shipped-on date and archived-path link.
- [x] **9.12** Archive: `mv openspec/changes/channel-email openspec/changes/archive/channel-email`.

## Decisions deferred to follow-up

- **IMAP IDLE.** Polling is fine for a low-traffic channel. If latency becomes a real complaint (which it won't, because humans expect email to take a minute anyway), IDLE is a localized refactor in `fetch.rs` — replace the SEARCH/FETCH loop with an IDLE wait, keep the rest of the architecture intact.
- **STARTTLS on port 587.** Only SMTPS on 465 is wired today. If a future deployment needs STARTTLS, switch `Tls::Wrapper` to `Tls::Required` at the builder step and expose the choice via a new `smtpTls` enum option.
- **OAuth2 / XOAUTH2.** Password auth is sufficient for a self-hosted mailbox. Gmail-style OAuth is a larger change that touches agenix, the home-manager module, and the SMTP/IMAP auth path — keep it for a dedicated proposal if the need arises.
- **Multiple account support.** One inbox per adapter. If a second bot identity is ever wanted, add a second `channels.email2` option — don't grow this one.
- **HTML outbound.** Plain-text replies render fine everywhere. HTML outbound introduces a universe of rendering inconsistencies with essentially no upside for a bot's voice.
- **Attachment handling.** Dropped on inbound, never generated on outbound. If the model ever needs to attach generated artifacts (screenshots, files), that's a new feature entirely.
- **Inbound body size cap.** Not capped today. If a sender mails the bot a 10 MB body, the dispatcher will happily send it to Claude and burn the tokens. Add a body-size filter at the `handle_message` boundary if this becomes a real problem.
