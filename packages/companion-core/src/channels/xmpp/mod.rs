//! XMPP channel adapter — connects the companion daemon to a self-hosted
//! XMPP server (Prosody, ejabberd, etc.) as a native client. Handles direct
//! messages and Multi-User Chat rooms, streams responses with XEP-0308
//! Last Message Correction, and signals presence via XEP-0085 Chat States.
//!
//! Runs as an async task inside companion-core (not a separate process).
//! Env-gated via `COMPANION_XMPP_ENABLE=1`. Uses `tokio-xmpp` for stream
//! management and `xmpp-parsers` for typed stanza construction. The TLS
//! handshake goes through our own [`connector::Connector`] (see that file's
//! header for the long version of why).

mod connector;

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Notify;
use tokio_xmpp::connect::DnsConfig;
use tokio_xmpp::jid::{BareJid, Jid};
use tokio_xmpp::xmlstream::Timeouts;
use tokio_xmpp::{Client, Event};
use tracing::{debug, error, info, warn};
use xmpp_parsers::message::{Lang, Message, MessageType};
use xmpp_parsers::muc::muc::{History, Muc};
use xmpp_parsers::presence::{Presence, Show as PresenceShow, Type as PresenceType};

use crate::dispatcher::{Dispatcher, TurnEvent, TurnRequest};

use connector::{build_tls_config, Connector};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How to render streaming responses on XMPP.
///
/// Mirrors [`crate::channels::telegram::StreamMode`] in shape but the
/// underlying mechanism is different: SingleMessage uses XEP-0308 Last
/// Message Correction (replace stanzas) instead of native message edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamMode {
    /// Send chunks as XEP-0308 corrections to the first message.
    SingleMessage,
    /// Send each chunk as a fresh message stanza.
    MultiMessage,
}

/// One MUC room the bot should auto-join on connection.
#[derive(Debug, Clone)]
pub struct MucRoom {
    /// Bare JID of the room (e.g. `xojabo@muc.chat.taile0fb4.ts.net`).
    pub jid: BareJid,
    /// Nick to use in the room.
    pub nick: String,
}

/// XMPP channel configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct XmppConfig {
    pub jid: BareJid,
    pub password: String,
    pub server: String,
    pub port: u16,
    pub allowed_jids: HashSet<BareJid>,
    pub muc_rooms: Vec<MucRoom>,
    pub mention_only: bool,
    pub stream_mode: StreamMode,
}

impl XmppConfig {
    /// Build config from environment variables. Returns `None` if the
    /// channel is not enabled (`COMPANION_XMPP_ENABLE != 1`).
    ///
    /// Env vars:
    /// - `COMPANION_XMPP_ENABLE` — required, must be `"1"`
    /// - `COMPANION_XMPP_JID` — required, bare JID e.g. `sid@chat.example.org`
    /// - `COMPANION_XMPP_PASSWORD_FILE` — required, path to a file containing the password
    /// - `COMPANION_XMPP_SERVER` — optional, defaults to `127.0.0.1`
    /// - `COMPANION_XMPP_PORT` — optional, defaults to `5222`
    /// - `COMPANION_XMPP_ALLOWED_JIDS` — comma-separated bare JIDs (deny by default)
    /// - `COMPANION_XMPP_MUC_ROOMS` — comma-separated `room@host/nick` entries
    /// - `COMPANION_XMPP_MENTION_ONLY` — `1`/`true` (default `1`, inverted from telegram)
    /// - `COMPANION_XMPP_STREAM_MODE` — `single_message` (default) or `multi_message`
    pub fn from_env() -> Option<Self> {
        if std::env::var("COMPANION_XMPP_ENABLE").ok()?.as_str() != "1" {
            return None;
        }

        let jid_str = match std::env::var("COMPANION_XMPP_JID") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                error!("COMPANION_XMPP_JID not set");
                return None;
            }
        };
        let jid = match BareJid::from_str(&jid_str) {
            Ok(j) => j,
            Err(e) => {
                error!(jid = %jid_str, %e, "invalid COMPANION_XMPP_JID");
                return None;
            }
        };

        let password_file = match std::env::var("COMPANION_XMPP_PASSWORD_FILE") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                error!("COMPANION_XMPP_PASSWORD_FILE not set");
                return None;
            }
        };
        let password = match std::fs::read_to_string(&password_file) {
            Ok(p) => p.trim().to_string(),
            Err(e) => {
                error!(path = %password_file, %e, "failed to read xmpp password file");
                return None;
            }
        };
        if password.is_empty() {
            error!(path = %password_file, "xmpp password file is empty");
            return None;
        }

        let server =
            std::env::var("COMPANION_XMPP_SERVER").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("COMPANION_XMPP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5222);

        let allowed_jids = parse_allowed_jids(
            std::env::var("COMPANION_XMPP_ALLOWED_JIDS").unwrap_or_default().as_str(),
        );

        let muc_rooms = parse_muc_rooms(
            std::env::var("COMPANION_XMPP_MUC_ROOMS").unwrap_or_default().as_str(),
        );

        // mention_only defaults to TRUE for xmpp (inverted from telegram).
        // The xojabo room is high-volume and the family already has Sid as
        // a member from ZeroClaw days — opt-out is the wrong default here.
        let mention_only = std::env::var("COMPANION_XMPP_MENTION_ONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let stream_mode = match std::env::var("COMPANION_XMPP_STREAM_MODE")
            .unwrap_or_default()
            .as_str()
        {
            "multi_message" | "multi-message" => StreamMode::MultiMessage,
            _ => StreamMode::SingleMessage,
        };

        Some(Self {
            jid,
            password,
            server,
            port,
            allowed_jids,
            muc_rooms,
            mention_only,
            stream_mode,
        })
    }
}

/// Parse a comma-separated list of bare JIDs. Empty / unparseable entries
/// are dropped with a warning. An empty input yields an empty allowlist —
/// which means **deny by default**, matching telegram.
fn parse_allowed_jids(raw: &str) -> HashSet<BareJid> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| match BareJid::from_str(s) {
            Ok(j) => Some(j),
            Err(e) => {
                warn!(entry = %s, %e, "skipping invalid jid in COMPANION_XMPP_ALLOWED_JIDS");
                None
            }
        })
        .collect()
}

/// Parse a comma-separated list of `room@host/nick` entries.
fn parse_muc_rooms(raw: &str) -> Vec<MucRoom> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|entry| {
            let (room_part, nick) = entry.rsplit_once('/')?;
            let jid = match BareJid::from_str(room_part) {
                Ok(j) => j,
                Err(e) => {
                    warn!(entry = %entry, %e, "skipping invalid muc room");
                    return None;
                }
            };
            if nick.is_empty() {
                warn!(entry = %entry, "skipping muc room with empty nick");
                return None;
            }
            Some(MucRoom {
                jid,
                nick: nick.to_string(),
            })
        })
        .collect()
}

/// Look up the bot's nick in a given MUC room. Returns `None` if the room
/// is not in the configured list (which means we shouldn't be in it and
/// any groupchat we received from it is suspect).
fn nick_for_room<'a>(config: &'a XmppConfig, room: &BareJid) -> Option<&'a str> {
    config
        .muc_rooms
        .iter()
        .find(|r| &r.jid == room)
        .map(|r| r.nick.as_str())
}

/// How a MUC body addressed (or didn't address) the bot.
///
/// This drives the `mention_only` decision in [`handle_groupchat_message`]:
/// `Addressed` and `Mentioned` both cause the bot to respond, `None` causes
/// the body to be dropped silently. The distinction between `Addressed` and
/// `Mentioned` exists so address-style prefixes ("Sid: hello") can be
/// stripped before dispatch — otherwise the persona reads its own name as
/// the first token of every turn, which is annoying for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Addressing {
    /// The body began with the bot's nick + a separator. The string is the
    /// rest of the body with the prefix stripped.
    Addressed(String),
    /// The body contains an `@nick` reference somewhere. Body unchanged.
    Mentioned,
    /// The bot was not addressed. Drop in `mention_only` mode.
    None,
}

/// Decide whether `body` addresses a bot named `nick`. Case-insensitive on
/// the nick — humans are sloppy. The recognized prefix forms are:
///
/// - `Sid: hello` / `Sid, hello` / `Sid - hello` / `Sid hello`
/// - `@Sid: hello` / `@Sid, hello` / `@Sid - hello` / `@Sid hello`
/// - bare `Sid` or bare `@Sid` (treated as a ping with empty body)
///
/// Beyond prefixes, any standalone `@Sid` token (followed by whitespace or
/// punctuation, or at end of string) elsewhere in the body counts as a
/// mention but does not modify the body — the @reference is presumably
/// load-bearing in the user's sentence.
///
/// **Crucial false-positive case**: a body of `xojabo` (the room name) must
/// NOT match a bot named anything other than `xojabo`. The fixture for this
/// is in tests — John types "xojabo" in the xojabo room constantly and the
/// bot must ignore him.
fn parse_mention(body: &str, nick: &str) -> Addressing {
    let trimmed = body.trim_start();

    // Try to match `nick<sep>` or `@nick<sep>` at the start.
    for prefix_len in [0usize, 1usize] {
        // prefix_len = 0 → match "Sid..." ; prefix_len = 1 → match "@Sid..."
        if prefix_len == 1 && !trimmed.starts_with('@') {
            continue;
        }
        let after_at = &trimmed[prefix_len..];
        if after_at.len() < nick.len() {
            continue;
        }
        let (head, rest) = after_at.split_at(nick.len());
        if !head.eq_ignore_ascii_case(nick) {
            continue;
        }
        // What follows the nick token?
        let next = rest.chars().next();
        match next {
            None => {
                // Bare "Sid" or "@Sid" — treat as a ping with no payload.
                return Addressing::Addressed(String::new());
            }
            Some(':') | Some(',') | Some('-') => {
                // Strip the separator AND any leading whitespace after it.
                // The `-` form covers "Sid - hi", which humans type all
                // the time and which would otherwise leak a leading dash
                // into the dispatch body (and historically tripped the
                // claude CLI parser — see dispatcher.rs comment).
                return Addressing::Addressed(rest[1..].trim_start().to_string());
            }
            Some(c) if c.is_whitespace() => {
                // After consuming the leading whitespace, also consume one
                // more separator char if present, so "Sid - hi" and
                // "Sid -hi" both yield "hi" and not "- hi" / "-hi".
                let after_ws = rest.trim_start();
                let stripped = after_ws
                    .strip_prefix([':', ',', '-'])
                    .map(|s| s.trim_start())
                    .unwrap_or(after_ws);
                return Addressing::Addressed(stripped.to_string());
            }
            _ => {
                // "Sidney", "Sidekick", etc — not a match, fall through.
            }
        }
    }

    // No prefix match. Look for a standalone @nick token elsewhere in the
    // body. Word-boundary check on what follows; the @ before the nick is
    // the boundary on the left.
    let needle = format!("@{}", nick);
    let needle_lower = needle.to_ascii_lowercase();
    let body_lower = body.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel_idx) = body_lower[search_from..].find(&needle_lower) {
        let idx = search_from + rel_idx;
        let after_idx = idx + needle.len();
        let next = body_lower[after_idx..].chars().next();
        let is_boundary = match next {
            None => true,
            Some(c) => !c.is_alphanumeric() && c != '_',
        };
        if is_boundary {
            return Addressing::Mentioned;
        }
        search_from = after_idx;
    }

    Addressing::None
}

// ---------------------------------------------------------------------------
// Serve — entry point. Phase 2 lands the connect/auth/presence path and the
// reconnect loop. DM/MUC message handling are Phase 3+ and live downstream.
// ---------------------------------------------------------------------------

/// Start the XMPP adapter. Blocks until `shutdown` fires. On any connection
/// error the loop reconnects with exponential backoff so the bot survives
/// Prosody restarts during nixos-rebuild.
pub async fn serve(
    dispatcher: Arc<Dispatcher>,
    config: XmppConfig,
    shutdown: Arc<Notify>,
) {
    // rustls 0.23+ requires a crypto provider be installed before any
    // ClientConfig is built. Install once, ignore "already installed".
    let _ = tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider().install_default();

    let config = Arc::new(config);
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    info!(
        jid = %config.jid,
        server = %config.server,
        port = config.port,
        muc_rooms = config.muc_rooms.len(),
        "XMPP adapter starting"
    );

    loop {
        let cfg = config.clone();
        let dispatcher = dispatcher.clone();
        let session = run_session(cfg, dispatcher);

        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                info!("XMPP adapter shutting down");
                return;
            }
            outcome = session => {
                match outcome {
                    Ok(()) => {
                        warn!("XMPP session ended cleanly — reconnecting");
                        backoff = Duration::from_secs(1);
                    }
                    Err(e) => {
                        error!(%e, ?backoff, "XMPP session error — reconnecting after backoff");
                        // Sleep with shutdown awareness so a stop signal
                        // doesn't have to wait the full backoff window.
                        tokio::select! {
                            _ = shutdown.notified() => {
                                info!("XMPP adapter shutting down during backoff");
                                return;
                            }
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
            }
        }
    }
}

/// One connect → auth → presence → event-loop cycle. Returns `Ok(())` on
/// graceful disconnect, `Err(_)` on any failure (caller decides whether to
/// reconnect).
async fn run_session(
    config: Arc<XmppConfig>,
    dispatcher: Arc<Dispatcher>,
) -> Result<(), tokio_xmpp::Error> {
    let connector = Connector {
        dns_config: DnsConfig::NoSrv {
            host: config.server.clone(),
            port: config.port,
        },
        tls_config: build_tls_config(),
    };

    // BareJid → Jid for tokio-xmpp's constructor (which takes Into<Jid>).
    let jid: Jid = Jid::from(config.jid.clone());

    let mut client = Client::new_with_connector(
        jid,
        config.password.clone(),
        connector,
        Timeouts::default(),
    );

    while let Some(event) = client.next().await {
        match event {
            Event::Online { bound_jid, resumed } => {
                if resumed {
                    info!(%bound_jid, "XMPP stream resumed");
                } else {
                    info!(%bound_jid, "XMPP online");
                    if let Err(e) = send_initial_presence(&mut client).await {
                        error!(%e, "failed to send initial presence");
                        return Err(e);
                    }
                    if !config.muc_rooms.is_empty() {
                        if let Err(e) = join_muc_rooms(&mut client, &config).await {
                            error!(%e, "failed to send MUC joins");
                            return Err(e);
                        }
                    }
                }
            }
            Event::Disconnected(err) => {
                warn!(%err, "XMPP disconnected");
                return Err(err);
            }
            Event::Stanza(stanza) => {
                if let Ok(message) = Message::try_from(stanza) {
                    match message.type_ {
                        MessageType::Chat => {
                            if let Err(e) = handle_chat_message(
                                &mut client,
                                &message,
                                &config,
                                &dispatcher,
                            )
                            .await
                            {
                                warn!(%e, "error handling XMPP chat message");
                            }
                        }
                        MessageType::Groupchat => {
                            if let Err(e) = handle_groupchat_message(
                                &mut client,
                                &message,
                                &config,
                                &dispatcher,
                            )
                            .await
                            {
                                warn!(%e, "error handling XMPP groupchat message");
                            }
                        }
                        _ => {
                            debug!(
                                ty = ?message.type_,
                                from = ?message.from,
                                "ignoring message of unhandled type"
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Returns true if the sender is on the allowlist. An empty allowlist
/// means nobody gets through — deny by default, mirroring telegram.
fn is_allowed(config: &XmppConfig, sender: &BareJid) -> bool {
    config.allowed_jids.contains(sender)
}

/// Handle one inbound `<message type="chat">`. Phase 3 does the simplest
/// possible thing: collect the dispatcher response into one final string
/// and send it back as a single chat stanza. Streaming with XEP-0308
/// corrections is Phase 4's job.
///
/// Errors are propagated up so the caller can log them; the session loop
/// continues regardless.
async fn handle_chat_message(
    client: &mut Client,
    message: &Message,
    config: &XmppConfig,
    dispatcher: &Dispatcher,
) -> Result<(), tokio_xmpp::Error> {
    // Extract sender bare JID — drop messages with no `from` (server pings,
    // some chat-state notifications) or those that don't parse cleanly.
    let from_jid = match message.from.as_ref() {
        Some(j) => j,
        None => {
            debug!("dropping chat message with no `from`");
            return Ok(());
        }
    };
    let sender_bare = from_jid.to_bare();

    // Extract body. A message with no body is typically a chat-state
    // notification (composing/active/paused) and we ignore those — we don't
    // need to react to typing indicators.
    let body = match message.bodies.values().next() {
        Some(b) => b.clone(),
        None => {
            debug!(from = %sender_bare, "dropping chat message with no body");
            return Ok(());
        }
    };

    // Allowlist enforcement.
    if !is_allowed(config, &sender_bare) {
        debug!(from = %sender_bare, "dropping chat message from non-allowlisted JID");
        return Ok(());
    }

    info!(
        from = %sender_bare,
        body_len = body.len(),
        "XMPP DM received"
    );

    let conversation_id = sender_bare.to_string();

    // Bang commands short-circuit the dispatcher. We use `!` instead of `/`
    // because Gajim (and probably other XMPP clients) intercept slash
    // commands locally for /me, /say, /clear, MUC moderation, etc — they
    // never reach the wire. Bang is the standard XMPP/IRC bot convention.
    let trimmed = body.trim();
    if trimmed.starts_with('!') {
        let reply_text = handle_command(&conversation_id, trimmed, dispatcher).await;
        send_chat_reply(client, &sender_bare, &reply_text).await?;
        return Ok(());
    }

    // Build the turn request and dispatch.
    let turn_req = TurnRequest {
        surface_id: "xmpp".into(),
        conversation_id,
        message_text: body,
    };
    let mut rx = dispatcher.dispatch(turn_req).await;

    // Collect everything into one final response. Phase 4 will replace this
    // with streaming corrections (XEP-0308); for now, one stanza per turn.
    let mut full_text = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => full_text.push_str(&chunk),
            TurnEvent::Complete(text) => {
                full_text = text;
                break;
            }
            TurnEvent::Error(e) => {
                full_text = format!("Something went sideways: {e}");
                break;
            }
        }
    }

    if full_text.is_empty() {
        warn!(from = %sender_bare, "dispatcher produced empty response — sending nothing");
        return Ok(());
    }

    send_chat_reply(client, &sender_bare, &full_text).await
}

/// Handle one inbound `<message type="groupchat">`. Phase 5.
///
/// The execution order matters and is load-bearing — change with care:
///
/// 1. **Resolve room and sender nick.** Drop on parse failure.
/// 2. **Loop trap drop.** If the sender nick == our nick in this room, this
///    message is the server echoing our own outbound stanza. Drop it
///    without logging at info level — empirically confirmed in the spike,
///    and the canonical infinite-loop bug from the ZeroClaw incident lives
///    here.
/// 3. **Body extraction.** Empty body = chat state notification, drop.
/// 4. **Mention parsing** (only if `mention_only`). Address-style prefixes
///    are stripped from the body before dispatch; `@nick` references are
///    accepted with the body unchanged; everything else is dropped.
/// 5. **Bang commands** are honored only on addressed messages. Reply goes
///    to the room as groupchat so everyone sees the result of `!new`.
/// 6. **Dispatch** with `conversation_id = room_bare.to_string()` — the
///    room is one conversation with the bot, not per-user-in-room.
///    Allowlist is BYPASSED for groupchat: room membership is the access
///    control boundary (per task 5.7).
async fn handle_groupchat_message(
    client: &mut Client,
    message: &Message,
    config: &XmppConfig,
    dispatcher: &Dispatcher,
) -> Result<(), tokio_xmpp::Error> {
    let from_jid = match message.from.as_ref() {
        Some(j) => j,
        None => {
            debug!("dropping groupchat message with no `from`");
            return Ok(());
        }
    };
    let room_bare = from_jid.to_bare();
    let sender_nick = match from_jid.resource() {
        Some(r) => r.as_str(),
        None => {
            // No resource = sent by the room itself (subject changes,
            // history end markers, etc). Nothing to respond to.
            debug!(room = %room_bare, "dropping groupchat with no resource (room-level stanza)");
            return Ok(());
        }
    };

    // Look up our nick in this room. If we don't have an entry, the bot
    // wasn't told to be in this room — log loud and bail.
    let our_nick = match nick_for_room(config, &room_bare) {
        Some(n) => n,
        None => {
            warn!(
                room = %room_bare,
                "received groupchat from a room not in muc_rooms config — dropping"
            );
            return Ok(());
        }
    };

    // LOOP TRAP. If this is our own message coming back, drop it. The
    // ZeroClaw `# Disabled: MUC loop issue` incident lives in this branch
    // — without this drop, the bot responds to itself until you pull the
    // plug. The cost of forgetting this is real token burn.
    if sender_nick == our_nick {
        debug!(
            room = %room_bare,
            nick = %sender_nick,
            "dropping own groupchat echo (loop trap)"
        );
        return Ok(());
    }

    // Body extraction. Empty body = chat state notification, drop.
    let body = match message.bodies.values().next() {
        Some(b) => b.clone(),
        None => {
            debug!(
                room = %room_bare,
                from = %sender_nick,
                "dropping groupchat with no body"
            );
            return Ok(());
        }
    };

    // Mention parsing: in mention_only mode, decide whether to respond and
    // (for address-style prefixes) what body text to send to the dispatcher.
    let dispatch_body = if config.mention_only {
        match parse_mention(&body, our_nick) {
            Addressing::Addressed(stripped) => stripped,
            Addressing::Mentioned => body.clone(),
            Addressing::None => {
                debug!(
                    room = %room_bare,
                    from = %sender_nick,
                    "dropping groupchat: not addressed and mention_only is on"
                );
                return Ok(());
            }
        }
    } else {
        body.clone()
    };

    info!(
        room = %room_bare,
        from = %sender_nick,
        body_len = dispatch_body.len(),
        "XMPP MUC message received"
    );

    let conversation_id = room_bare.to_string();

    // Bang commands fire only on addressed messages. The mention parser has
    // already stripped any "Sid:" prefix, so `dispatch_body` starts with `!`
    // iff the user typed e.g. "Sid: !new". Bang commands are deliberately
    // ALSO accepted on unaddressed bodies in non-mention_only rooms — if a
    // room is configured to respond to everything, every command is fair
    // game. Reply goes back as groupchat so the room sees the result.
    let trimmed = dispatch_body.trim();
    if trimmed.starts_with('!') {
        let reply_text = handle_command(&conversation_id, trimmed, dispatcher).await;
        send_groupchat_reply(client, &room_bare, &reply_text).await?;
        return Ok(());
    }

    let turn_req = TurnRequest {
        surface_id: "xmpp".into(),
        conversation_id,
        message_text: dispatch_body,
    };
    let mut rx = dispatcher.dispatch(turn_req).await;

    // Same collect-everything-into-one-stanza pattern as DMs. Phase 4 will
    // add streaming corrections; in MUC, XEP-0308 corrections are addressed
    // to the room and every client in the room sees the in-place updates.
    let mut full_text = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => full_text.push_str(&chunk),
            TurnEvent::Complete(text) => {
                full_text = text;
                break;
            }
            TurnEvent::Error(e) => {
                full_text = format!("Something went sideways: {e}");
                break;
            }
        }
    }

    if full_text.is_empty() {
        warn!(
            room = %room_bare,
            from = %sender_nick,
            "dispatcher produced empty response — sending nothing"
        );
        return Ok(());
    }

    send_groupchat_reply(client, &room_bare, &full_text).await
}

/// Send a single `<message type="chat">` stanza back to the given bare JID.
/// Replying to the bare JID (not a specific resource) lets the user's
/// server pick the best resource to deliver to — handles roaming between
/// Conversations on phone and Gajim on desktop.
async fn send_chat_reply(
    client: &mut Client,
    to: &BareJid,
    body: &str,
) -> Result<(), tokio_xmpp::Error> {
    let to_jid = Jid::from(to.clone());
    let mut reply = Message::new(Some(to_jid));
    reply.type_ = MessageType::Chat;
    reply.bodies.insert(Lang(String::new()), body.to_string());
    client.send_stanza(reply.into()).await?;
    Ok(())
}

/// Send a single `<message type="groupchat">` stanza to a MUC room. The
/// destination is the bare room JID — the server fans the message out to
/// every occupant including the sender (which is what the loop trap drop
/// in [`handle_groupchat_message`] is there to handle).
async fn send_groupchat_reply(
    client: &mut Client,
    room: &BareJid,
    body: &str,
) -> Result<(), tokio_xmpp::Error> {
    let to_jid = Jid::from(room.clone());
    let mut reply = Message::new(Some(to_jid));
    reply.type_ = MessageType::Groupchat;
    reply.bodies.insert(Lang(String::new()), body.to_string());
    client.send_stanza(reply.into()).await?;
    Ok(())
}

/// Join every configured MUC room. Sends a presence stanza addressed to
/// `room@host/nick` with a `<x xmlns="http://jabber.org/protocol/muc"/>`
/// payload requesting zero history stanzas — bots have no use for the
/// scrollback and processing it on every join would be a token sink.
///
/// Errors on the first failed join short-circuit the function. The caller
/// (`run_session`) returns the error and `serve()`'s reconnect-with-backoff
/// loop handles the retry.
async fn join_muc_rooms(
    client: &mut Client,
    config: &XmppConfig,
) -> Result<(), tokio_xmpp::Error> {
    for room in &config.muc_rooms {
        let occupant_str = format!("{}/{}", room.jid, room.nick);
        let occupant_jid = match Jid::from_str(&occupant_str) {
            Ok(j) => j,
            Err(e) => {
                error!(
                    room = %room.jid,
                    nick = %room.nick,
                    %e,
                    "failed to construct MUC occupant JID — skipping room"
                );
                continue;
            }
        };
        let join = Presence::new(PresenceType::None)
            .with_to(occupant_jid)
            .with_payload(Muc::new().with_history(History::new().with_maxstanzas(0)));
        client.send_stanza(join.into()).await?;
        info!(room = %room.jid, nick = %room.nick, "MUC join sent");
    }
    Ok(())
}

/// Extract the command token from a bang command body. Takes the first
/// whitespace-separated token and strips any trailing `@suffix` (some MUC
/// clients append the bot nick — `!new@Sid`). Returns `""` for empty input.
/// Prefix-agnostic — caller is responsible for matching against `!cmd` etc.
fn extract_command_name(text: &str) -> &str {
    let cmd = text.split_whitespace().next().unwrap_or("");
    cmd.split('@').next().unwrap_or(cmd)
}

/// Parse and handle bang commands. Returns the reply text to send back.
/// Unrecognized `!commands` get a deflection reply rather than being
/// forwarded to the dispatcher — same reason as telegram, prevents Claude
/// Code skill leakage from typos.
///
/// `conversation_id` is the dispatcher session key — for DMs that's the
/// sender's bare JID; for MUC that's the room's bare JID. The command
/// applies to the right session because the routing key is the right
/// thing.
async fn handle_command(
    conversation_id: &str,
    text: &str,
    dispatcher: &Dispatcher,
) -> String {
    let cmd = extract_command_name(text);

    match cmd {
        "!new" => {
            let store = dispatcher.store().await;
            let had_session = store
                .delete_session("xmpp", conversation_id)
                .unwrap_or(false);
            drop(store);
            info!(conversation_id, "xmpp !new — session reset");
            if had_session {
                "Fine. Everything we just talked about? Gone. Hope it wasn't important."
                    .to_string()
            } else {
                "There's nothing to forget. We haven't even started yet.".to_string()
            }
        }
        "!status" => {
            let store = dispatcher.store().await;
            let session = store
                .lookup_session("xmpp", conversation_id)
                .ok()
                .flatten();
            drop(store);
            match session {
                Some(s) => {
                    let claude_id = s
                        .claude_session_id
                        .as_deref()
                        .unwrap_or("(not yet assigned)");
                    format!(
                        "Session active\nClaude session: {}\nLast active: {}",
                        claude_id,
                        super::util::format_timestamp(s.last_active_at),
                    )
                }
                None => "No active session. Send a message to start one.".to_string(),
            }
        }
        "!help" => "\
!new — clear session, start fresh\n\
!status — show current session info\n\
!help — this message\n\
\n\
Everything else goes straight to the companion."
            .to_string(),
        _ => "Not a command. Try !help if you're lost.".to_string(),
    }
}

/// Send the initial `<presence/>` so the bot shows as available with a Sid
/// status line. Equivalent to telegram's "I'm online" — but on XMPP this is
/// also the prerequisite for being able to receive any messages at all.
async fn send_initial_presence(client: &mut Client) -> Result<(), tokio_xmpp::Error> {
    let mut presence = Presence::new(PresenceType::None);
    presence.show = Some(PresenceShow::Chat);
    presence.statuses.insert(
        Lang(String::new()),
        "Sid here — go ahead and waste my time.".to_string(),
    );
    client.send_stanza(presence.into()).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allowed_jids_empty_yields_empty() {
        let parsed = parse_allowed_jids("");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_allowed_jids_handles_whitespace_and_commas() {
        let parsed = parse_allowed_jids("keith@example.org , alice@example.org,, ");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&BareJid::from_str("keith@example.org").unwrap()));
        assert!(parsed.contains(&BareJid::from_str("alice@example.org").unwrap()));
    }

    #[test]
    fn parse_allowed_jids_drops_garbage() {
        let parsed = parse_allowed_jids("not a jid,keith@example.org");
        assert_eq!(parsed.len(), 1);
        assert!(parsed.contains(&BareJid::from_str("keith@example.org").unwrap()));
    }

    #[test]
    fn parse_muc_rooms_basic() {
        let parsed = parse_muc_rooms("xojabo@muc.example.org/Sid");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].nick, "Sid");
        assert_eq!(
            parsed[0].jid,
            BareJid::from_str("xojabo@muc.example.org").unwrap()
        );
    }

    #[test]
    fn parse_muc_rooms_multiple() {
        let parsed = parse_muc_rooms(
            "xojabo@muc.example.org/Sid, lounge@muc.example.org/SidBot",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].nick, "SidBot");
    }

    #[test]
    fn parse_muc_rooms_drops_missing_nick() {
        let parsed = parse_muc_rooms("xojabo@muc.example.org");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_muc_rooms_drops_empty_nick() {
        let parsed = parse_muc_rooms("xojabo@muc.example.org/");
        assert!(parsed.is_empty());
    }

    #[test]
    fn stream_mode_variants_distinct() {
        assert_ne!(StreamMode::SingleMessage, StreamMode::MultiMessage);
    }

    fn make_config(allowed: &[&str]) -> XmppConfig {
        XmppConfig {
            jid: BareJid::from_str("sid@example.org").unwrap(),
            password: "x".into(),
            server: "127.0.0.1".into(),
            port: 5222,
            allowed_jids: allowed
                .iter()
                .map(|s| BareJid::from_str(s).unwrap())
                .collect(),
            muc_rooms: vec![],
            mention_only: true,
            stream_mode: StreamMode::SingleMessage,
        }
    }

    #[test]
    fn allowlist_empty_denies_all() {
        let config = make_config(&[]);
        let stranger = BareJid::from_str("stranger@example.org").unwrap();
        assert!(!is_allowed(&config, &stranger));
    }

    #[test]
    fn allowlist_permits_listed_jid() {
        let config = make_config(&["keith@example.org"]);
        let keith = BareJid::from_str("keith@example.org").unwrap();
        assert!(is_allowed(&config, &keith));
    }

    #[test]
    fn allowlist_denies_unlisted_jid() {
        let config = make_config(&["keith@example.org"]);
        let alice = BareJid::from_str("alice@example.org").unwrap();
        assert!(!is_allowed(&config, &alice));
    }

    #[test]
    fn allowlist_does_not_match_resource() {
        // Resources should already be stripped before is_allowed runs, but
        // verify that bare-jid equality is what's used (not full-jid string
        // matching). A typo here would let resource-spoofing past the gate.
        let config = make_config(&["keith@example.org"]);
        let keith_phone = BareJid::from_str("keith@example.org").unwrap();
        assert!(is_allowed(&config, &keith_phone));
    }

    #[test]
    fn extract_command_name_basic() {
        assert_eq!(extract_command_name("!new"), "!new");
        assert_eq!(extract_command_name("!status"), "!status");
        assert_eq!(extract_command_name("!help"), "!help");
    }

    #[test]
    fn extract_command_name_strips_arguments() {
        // Users sometimes type "!new keep this part"; the parser should
        // isolate the command and ignore everything after.
        assert_eq!(extract_command_name("!new keep this part"), "!new");
    }

    #[test]
    fn extract_command_name_strips_at_suffix() {
        // MUC clients sometimes append the bot's nick: "!new@Sid"
        assert_eq!(extract_command_name("!new@Sid"), "!new");
        assert_eq!(extract_command_name("!help@SidBot extra"), "!help");
    }

    #[test]
    fn extract_command_name_handles_empty_and_garbage() {
        assert_eq!(extract_command_name(""), "");
        assert_eq!(extract_command_name("   "), "");
        // Non-slash inputs are passed through unchanged — handle_command
        // matches against `"/new"` etc, so anything else falls through to
        // the deflection branch automatically.
        assert_eq!(extract_command_name("hello"), "hello");
    }

    // -----------------------------------------------------------------------
    // Phase 5 — MUC support: nick lookup, mention parsing, loop trap
    // -----------------------------------------------------------------------

    fn config_with_room(room: &str, nick: &str) -> XmppConfig {
        XmppConfig {
            jid: BareJid::from_str("sid@example.org").unwrap(),
            password: "x".into(),
            server: "127.0.0.1".into(),
            port: 5222,
            allowed_jids: HashSet::new(),
            muc_rooms: vec![MucRoom {
                jid: BareJid::from_str(room).unwrap(),
                nick: nick.to_string(),
            }],
            mention_only: true,
            stream_mode: StreamMode::SingleMessage,
        }
    }

    #[test]
    fn nick_for_room_hits_configured_room() {
        let cfg = config_with_room("xojabo@muc.example.org", "Sid");
        let room = BareJid::from_str("xojabo@muc.example.org").unwrap();
        assert_eq!(nick_for_room(&cfg, &room), Some("Sid"));
    }

    #[test]
    fn nick_for_room_misses_unknown_room() {
        let cfg = config_with_room("xojabo@muc.example.org", "Sid");
        let other = BareJid::from_str("lounge@muc.example.org").unwrap();
        assert_eq!(nick_for_room(&cfg, &other), None);
    }

    #[test]
    fn parse_mention_strips_colon_prefix() {
        assert_eq!(
            parse_mention("Sid: hello there", "Sid"),
            Addressing::Addressed("hello there".to_string())
        );
    }

    #[test]
    fn parse_mention_strips_comma_prefix() {
        assert_eq!(
            parse_mention("Sid, hello there", "Sid"),
            Addressing::Addressed("hello there".to_string())
        );
    }

    #[test]
    fn parse_mention_strips_space_prefix() {
        assert_eq!(
            parse_mention("Sid hello there", "Sid"),
            Addressing::Addressed("hello there".to_string())
        );
    }

    #[test]
    fn parse_mention_strips_dash_separator() {
        // The case that broke the very first live MUC test (2026-04-08).
        // Keith typed "Sid - hi"; the parser used to leave "- hi" in the
        // dispatch body, and the dispatcher's `claude -p "- hi"` invocation
        // tripped on the leading dash with `error: unknown option '- hi'`.
        // The dispatcher.rs reorder is the real fix; this test makes sure
        // the parser also strips the dash so the dispatch body is clean.
        assert_eq!(
            parse_mention("Sid - hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
        assert_eq!(
            parse_mention("Sid -hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
        assert_eq!(
            parse_mention("Sid- hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
        assert_eq!(
            parse_mention("@Sid - hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
    }

    #[test]
    fn parse_mention_bare_nick_is_ping() {
        assert_eq!(
            parse_mention("Sid", "Sid"),
            Addressing::Addressed(String::new())
        );
        assert_eq!(
            parse_mention("@Sid", "Sid"),
            Addressing::Addressed(String::new())
        );
    }

    #[test]
    fn parse_mention_at_prefix_strips() {
        assert_eq!(
            parse_mention("@Sid: hello", "Sid"),
            Addressing::Addressed("hello".to_string())
        );
        assert_eq!(
            parse_mention("@Sid hello", "Sid"),
            Addressing::Addressed("hello".to_string())
        );
    }

    #[test]
    fn parse_mention_case_insensitive() {
        assert_eq!(
            parse_mention("sid: hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
        assert_eq!(
            parse_mention("SID: hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
    }

    #[test]
    fn parse_mention_leading_whitespace_ignored() {
        assert_eq!(
            parse_mention("  Sid: hi", "Sid"),
            Addressing::Addressed("hi".to_string())
        );
    }

    #[test]
    fn parse_mention_inline_at_reference_is_mentioned() {
        // @-mention not at the start, body unchanged.
        assert_eq!(
            parse_mention("hey @Sid look at this", "Sid"),
            Addressing::Mentioned
        );
    }

    #[test]
    fn parse_mention_inline_at_reference_at_end() {
        assert_eq!(
            parse_mention("look at this @Sid", "Sid"),
            Addressing::Mentioned
        );
        assert_eq!(
            parse_mention("look at this @Sid.", "Sid"),
            Addressing::Mentioned
        );
    }

    #[test]
    fn parse_mention_no_address_no_mention() {
        assert_eq!(
            parse_mention("hello world", "Sid"),
            Addressing::None
        );
    }

    #[test]
    fn parse_mention_substring_is_not_a_match() {
        // Sidney starts with "Sid" but is not "Sid"+separator.
        assert_eq!(
            parse_mention("Sidney is here", "Sid"),
            Addressing::None
        );
        // "@Sidney" similarly is not "@Sid"+separator/end.
        assert_eq!(
            parse_mention("hey @Sidney whatup", "Sid"),
            Addressing::None
        );
    }

    #[test]
    fn parse_mention_xojabo_fixture() {
        // The canonical false-positive case from tasks.md 8.3: John types
        // "xojabo" in the xojabo room constantly. The bot is named "Sid".
        // The bot must NOT respond to John. If this test ever fails, the
        // mention parser is broken and the next deploy will spam the room.
        assert_eq!(parse_mention("xojabo", "Sid"), Addressing::None);
        assert_eq!(parse_mention("XOJABO", "Sid"), Addressing::None);
        assert_eq!(parse_mention("xojabo!", "Sid"), Addressing::None);
        assert_eq!(parse_mention("xojabo xojabo xojabo", "Sid"), Addressing::None);
    }

    #[test]
    fn parse_mention_command_addressed_in_muc() {
        // The intended pattern for MUC commands: address the bot, then
        // include the bang command in the body. Mention parser strips the
        // prefix and the body becomes "!new" — handle_command then fires.
        assert_eq!(
            parse_mention("Sid: !new", "Sid"),
            Addressing::Addressed("!new".to_string())
        );
    }

    #[test]
    fn parse_mention_multiline_body() {
        // Multi-line addresses: first line is the address, the rest of the
        // body is the actual message. Should still be a clean strip.
        assert_eq!(
            parse_mention("Sid: hello\nhow are you", "Sid"),
            Addressing::Addressed("hello\nhow are you".to_string())
        );
    }
}
