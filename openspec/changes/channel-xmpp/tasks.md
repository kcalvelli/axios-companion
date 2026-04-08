# Tasks: XMPP Channel Adapter — Tier 1

Second channel adapter, after telegram. Stress-tests the channel pattern against a genuinely different protocol (XML streams, presence, MUC, no native message editing). Connects to the existing Prosody server on mini at `127.0.0.1:5222`, JID `sid@chat.taile0fb4.ts.net`, password already in agenix at `secrets/xmpp-bot-password.age`. Deploys mini-only.

## Phase 1: Dependencies and skeleton

- [x] **1.1** ~~Pick the Rust XMPP stack~~ **DECIDED 2026-04-08**: use `tokio-xmpp` 5.0.0 + `xmpp-parsers` 0.22.0 directly. **Skip** the high-level `xmpp` crate — it's at 0.6.0 (last released July 2024), self-describes as "very much WIP," and its `Event` enum exposes only `ChatMessage`/`RoomMessage`/`RoomJoined`-style variants, with no surface for XEP-0308 corrections or XEP-0085 chat states. We'd be hand-rolling those stanzas via `xmpp-parsers` regardless, so the wrapper provides no value while adding a stale dependency. `xmpp-parsers` confirmed to have `message_correct` (XEP-0308), `chatstates` (XEP-0085), `muc` (XEP-0045), and `message` (RFC 6120) modules.
  - **ADDENDUM 2026-04-08 (Phase 2 spike)**: tokio-xmpp 5.0.0's shipped `StartTlsServerConnector` hardcodes its rustls `ClientConfig` inside `connect/tls_common.rs::establish_tls_connection` with no public override hook for a custom `ServerCertVerifier`. Our chat infra (Prosody behind Tailscale Serve TCP passthrough) presents a self-signed cert, which the upstream connector rejects unconditionally. Resolved by implementing a custom `ServerConnector` that mirrors `StartTlsServerConnector::connect` but slots in our own `Arc<ClientConfig>` at the TLS handshake step. Verified end-to-end via standalone spike at `~/.local/share/axios-companion/workspace/xmpp-spike` (DM connect, presence broadcast, MUC join, groupchat send — all green against mini's Prosody, Keith confirmed visually in his roster + the xojabo room). Production code lives at `packages/companion-core/src/channels/xmpp/connector.rs`. ~80 lines, single file, modeled directly on upstream. The spike crate is preserved for future debugging.
- [x] **1.2** Add `tokio-xmpp = "5"` and `xmpp-parsers = "0.22"` to `packages/companion-core/Cargo.toml`. Also adds `tokio-rustls = "0.26"` (with `aws_lc_rs` feature) and `sasl = "0.5"` as direct deps because the custom connector needs `TlsConnector` and `ChannelBinding` types. Defaults give rustls + starttls + hickory.
- [x] **1.3** `packages/companion-core/default.nix` untouched — no native build inputs needed beyond what was already present (verified via sandbox `nix build`).
- [x] **1.4** `cargo build -p companion-core` green.
- [x] **1.5** Channels namespace reorg shipped (Option A). `channels/util.rs` with shared `split_message`, telegram moved to `channels/telegram/`, new `channels/xmpp/` directory.
- [x] **1.6** `channels/xmpp/mod.rs` skeleton landed at Phase 1.6 (then replaced with real config + serve() in Phase 2).
- [x] **1.7** Pre-XMPP regression check passed: telegram on mini stayed working (`/new`, free-text DM, `/status`, `/help`) after the channels reorg. Verified in session 2026-04-08.

## Phase 2: Configuration and connection

- [x] **2.1** `XmppConfig` struct landed in `channels/xmpp/mod.rs`. Fields: `jid: BareJid`, `password`, `server`, `port`, `allowed_jids: HashSet<BareJid>`, `muc_rooms: Vec<MucRoom>` (struct of `BareJid` + `nick`), `mention_only`, `stream_mode`. **Note:** the `tls_verify` field from the original plan was deliberately omitted — see addendum below.
- [x] **2.2** `XmppConfig::from_env()` reads `COMPANION_XMPP_ENABLE`, `_JID`, `_PASSWORD_FILE`, `_SERVER`, `_PORT`, `_ALLOWED_JIDS`, `_MUC_ROOMS`, `_MENTION_ONLY`, `_STREAM_MODE`. Returns `None` when ENABLE != 1 or any required field is missing/invalid. Logs the failure reason at error level so a misconfigured deploy is loud, not silent. Tests cover the `parse_allowed_jids` and `parse_muc_rooms` helpers.
- [x] **2.3** Password is read from `COMPANION_XMPP_PASSWORD_FILE` (agenix-managed). Empty file → error → return None. Path read failure → error → return None.
- [x] **2.4** `StreamMode` enum identical in shape to telegram's: `SingleMessage` (will use XEP-0308 corrections in Phase 4) and `MultiMessage` (will use the shared `split_message` from `channels/util.rs`).
- [x] **2.5** Connect path lands in `serve()` → `run_session()`. Uses our custom `Connector` (see 1.1 addendum) with `DnsConfig::NoSrv { host, port }` to bypass SRV lookups. Authenticates via tokio-xmpp's built-in SASL PLAIN. The custom connector handles TCP, plaintext stream open, `<starttls/>` negotiation, the TLS handshake against our `ClientConfig`, and the post-TLS stream re-open. Spike (1.1 addendum) verified the full negotiation against mini's Prosody.
  - **ADDENDUM 2026-04-08**: The original task said "accept self-signed cert when `tls_verify = false`" with the implication that there'd be a `tls_verify` config field. This was dropped from `XmppConfig` because we currently support only the no-verify path — adding a config field for an unimplemented `tls_verify=true` branch would let an operator silently get insecure TLS while believing they asked for verification. When real certs land (e.g. Tailscale-issued certs for chat), the `tls_verify` field and the verified branch in `connector::build_tls_config` land in the same commit.
- [x] **2.6** `send_initial_presence()` runs on the first non-resumed `Online` event. Sends `<presence/>` with `Show::Chat` and a Sid status line ("Sid here — go ahead and waste my time."). On resumed sessions, presence is not re-sent (smacks resumption preserves it).
- [x] **2.7** Reconnect-with-backoff loop in `serve()`. Exponential backoff capped at 60 seconds, reset to 1s on a clean session end. The backoff sleep is awaited inside `tokio::select!` against the shutdown `Notify` so a stop signal doesn't have to wait the full window. Verified by code review against telegram's pattern; live verification deferred to Phase 8.
- [x] **2.8** **(folded forward from 7.1)** XMPP adapter wired into `main.rs` as step 5c, env-gated, shared `Arc<Dispatcher>`, shutdown via the existing `Notify`. Done now (instead of in Phase 7) so the dead-code warnings from the unwired connector don't pile up across Phase 3-6 commits. The systemd unit / NixOS module work (7.2-7.6) stays in Phase 7.

## Phase 3: Direct message handling

- [x] **3.1** Message stanza handler in `run_session`: filters `Stanza::Message` with `MessageType::Chat`, extracts `from.to_bare()` for the sender and the first body string. Messages with no body (chat-state notifications) are dropped at debug level.
- [x] **3.2** Allowlist filter via `is_allowed(&XmppConfig, &BareJid)`. Empty allowlist = deny everyone, matching telegram.
- [x] **3.3** Sender bare JID is used directly as `conversation_id` (`bare.to_string()`) for the session store — same shape as telegram's `chat_id.to_string()`. Bare JID, not full JID, so the same conversation persists across resource roaming (Conversations on phone vs Gajim on desktop).
- [x] **3.4** `TurnRequest { surface_id: "xmpp", conversation_id, message_text }` dispatched through the shared `Arc<Dispatcher>`.
- [x] **3.5** Reply sent as `<message type="chat">` to the sender's bare JID via `send_chat_reply()`. Phase 3 collects the full dispatcher response into one stanza (no streaming yet — Phase 4's job).
- [x] **3.6** Unit tests for allowlist enforcement: empty-denies-all, permits-listed, denies-unlisted, bare-jid-equality (4 tests, all passing).

## Phase 4: Streaming, corrections, and chat states

- [ ] **4.1** XMPP message splitting: call `crate::channels::util::split_message(text, 3000)` (3000-char cap is the empirical comfortable size for Conversations/Gajim/Dino — XMPP has no protocol limit, but clients get unhappy past a few thousand chars). Make the cap a constant in `channels/xmpp/mod.rs`, not a magic number.
- [ ] **4.2** *(folded into 1.5 — `split_message` already lives in `channels/util.rs` by this phase)*
- [ ] **4.3** Implement `MultiMessage` stream mode: collect dispatcher events into a buffer, on `Complete` split and send N stanzas in order.
- [ ] **4.4** Implement `SingleMessage` stream mode using XEP-0308: send the first chunk as a normal message, then on each subsequent chunk send a correction stanza referencing the previous message's `id` via `<replace xmlns="urn:xmpp:message-correct:0" id="..."/>`. Throttle corrections to ~1.5s like telegram.
- [ ] **4.5** Verify XEP-0308 behavior in Conversations (Android), Gajim (Linux), and Dino (Linux) — all three are clients in the household. If any of them ignore corrections, document the fallback expectation in the spec.
- [ ] **4.6** Implement XEP-0085 Chat States: send `<composing/>` when dispatch starts, `<active/>` when dispatch completes. This is the typing-indicator equivalent. Do NOT send `<paused/>` or `<inactive/>` — overkill for a bot.
- [ ] **4.7** Unit tests for `split_message()` (paragraph/line/sentence/word/hard-cut paths).

## Phase 5: MUC support

- [ ] **5.1** Implement MUC auto-join on connection: for each `(room_jid, nick)` in config, send a presence stanza to `room_jid/nick` to join.
- [ ] **5.2** Handle MUC message stanzas (`<message type="groupchat">`). Extract room JID, sender nick, body.
- [ ] **5.3** **Loop prevention**: drop any groupchat message whose sender nick equals our own nick in that room. The ZeroClaw incident (`# Disabled: MUC loop issue with zeroclaw` comment in mini.nix) was almost certainly this — verify by testing once integration is up.
- [ ] **5.4** **Mention parsing**: in `mention_only` mode, only respond when the body starts with our nick followed by `:`, `,`, or whitespace, OR contains an `@nick` reference. Strip the mention from the body before dispatching.
- [ ] **5.5** Map room JID → session ID (separate session per room, not per user-in-room — the bot has one conversation with the room as a whole).
- [ ] **5.6** Send responses as groupchat stanzas to the room JID. SingleMessage corrections work in MUC the same way as DMs.
- [ ] **5.7** Allowlist behavior in MUC: trust everyone in a room the bot has been told to join. Room membership is the access control boundary, not per-JID allowlists. Document this decision.
- [ ] **5.8** Unit test: own-nick loop prevention. Unit test: mention parsing edge cases.

## Phase 6: Slash commands

- [x] **6.1** `/new`, `/status`, `/help` implemented in `handle_command()`, mirroring telegram's command set with the same Sid voice on the replies. `/status` reuses `super::util::format_timestamp` (deduped from telegram in this same commit).
- [x] **6.2** Unrecognized `/commands` get a deflection reply ("Not a command. Try /help if you're lost.") and are NOT forwarded to the dispatcher — prevents Claude Code skill leakage from typos.
- [ ] **6.3** In MUC, slash commands only fire when the bot is being addressed. **Deferred to Phase 5** (MUC handling) since the addressing logic doesn't exist yet. The current Phase 6 handler runs unconditionally on DMs only.
- [x] **6.4** Unit tests for `extract_command_name` (4 tests): basic commands, argument stripping, `@suffix` stripping (for MUC clients that append the bot nick), and empty/garbage handling.

## Phase 7: Wiring

- [x] **7.1** ~~Add the xmpp adapter as step 5c in `packages/companion-core/src/main.rs`, env-gated, shared `Arc<Dispatcher>`, shutdown via the existing `Notify`.~~ Folded forward into Phase 2 (see 2.8). The systemd / module / host-config tasks below remain in Phase 7.
- [ ] **7.2** Add `services.axios-companion.channels.xmpp` options to `modules/home-manager/default.nix`: `enable`, `jid`, `passwordFile`, `server` (default `127.0.0.1`), `port` (default `5222`), `tlsVerify` (default `false`), `allowedJids`, `mucRooms` (list of `{ jid, nick }`), `mentionOnly` (default `true`), `streamMode` (default `single_message`).
- [ ] **7.3** Add an assertion: `channels.xmpp.enable -> daemon.enable`.
- [ ] **7.4** Wire the env vars into the systemd unit, mirroring telegram's block.
- [ ] **7.5** Enable on mini in `~/.config/nixos_config/hosts/mini.nix` via the existing `home-manager.users.keith` host override. Reuse `secrets/xmpp-bot-password.age`. Configure `xojabo@muc.chat.taile0fb4.ts.net` as a MUC room with nick `Sid`.
- [ ] **7.6** Verify `nix flake check` passes for both the companion repo and `~/.config/nixos_config`.

## Phase 8: Live test, docs, archive

- [ ] **8.1** Deploy to mini: `sudo nixos-rebuild switch --flake .#mini`.
- [ ] **8.2** Live DM test from Conversations on Keith's phone to `sid@chat.taile0fb4.ts.net`. Verify: response arrives, streaming works (single-message corrections render correctly in Conversations), `/new` resets session, `/status` reports correctly.
- [ ] **8.3** Live MUC test in `xojabo@muc.chat.taile0fb4.ts.net`. Verify: bot is present in room, ignores ambient chatter, responds when addressed by `Sid:` or `@Sid`, does not loop on its own messages. **Built-in test fixture**: John types "xojabo" in `xojabo` constantly because he likes the way it sounds. The bot must NOT respond to a bare "xojabo" message — that's the canonical false-positive case for `mention_only`. If Sid responds to John's xojabo spam even once, the mention parser is broken.
- [ ] **8.4** Watch `journalctl --user -u axios-companion -f` during the test for warnings/errors. Address anything noisy.
- [ ] **8.5** Update `README.md` with an XMPP setup section (briefly — link to channel-telegram's section as the model since the patterns rhyme).
- [ ] **8.6** Update `ROADMAP.md` to mark `channel-xmpp` complete.
- [ ] **8.7** Write a session handoff memory note matching the channel-telegram precedent (`project_session_handoff_<date>_xmpp.md`).
- [ ] **8.8** Archive: `mv openspec/changes/channel-xmpp openspec/changes/archive/channel-xmpp`.
- [ ] **8.9** Commit the archive move.

## Decisions deferred to implementation

- **OMEMO**: out of scope. Self-hosted Prosody on Tailscale, federation off — the trust model doesn't need it. Revisit only if a household member asks.
- **File transfer (XEP-0363)**: out of scope for v1. The Prosody server already has it enabled for human users; the bot can ignore it. Revisit when voice/image input lands.
- **Carbons (XEP-0280)**: not relevant for a bot — the bot has only one resource, it's not syncing across devices.
- **Smacks (XEP-0198)**: nice to have for connection resilience, but the high-level `xmpp` crate may handle it transparently. Decide during 1.1.
