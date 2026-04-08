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
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;
use tokio_xmpp::connect::DnsConfig;
use tokio_xmpp::jid::{BareJid, Jid};
use tokio_xmpp::xmlstream::Timeouts;
use tokio_xmpp::{Client, Event, Stanza};
use tracing::{debug, error, info, warn};
use xmpp_parsers::message::{Id, Lang, Message, MessageType};
use xmpp_parsers::message_correct::Replace;
use xmpp_parsers::muc::muc::{History, Muc};
use xmpp_parsers::presence::{Presence, Show as PresenceShow, Type as PresenceType};

use crate::dispatcher::{Dispatcher, TurnEvent, TurnRequest};

use connector::{build_tls_config, Connector};

/// Minimum gap between XEP-0308 correction stanzas during a streaming
/// turn. Mirrors telegram's edit-message rate (`channels::telegram::
/// EDIT_THROTTLE`) so the streaming feel is consistent across surfaces.
/// Faster than this and partial-support clients (anyone who renders
/// corrections as N separate messages instead of in-place) get spammed;
/// slower and the streaming stops feeling like streaming.
///
/// The first chunk of a turn is sent immediately and does not respect
/// this throttle — the goal is "the user sees something the moment Sid
/// has anything to say." The throttle only applies to mid-stream
/// corrections after the initial send.
const STREAM_THROTTLE: Duration = Duration::from_millis(1500);

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

    // Outbound stanza channel. Spawned turn tasks build their reply
    // stanzas and push them through `out_tx`; the read loop is the only
    // thing that ever calls `client.send_stanza`. Buffer 64 is plenty —
    // the channel only fills if the network is wedged, in which case
    // backpressure on the turn tasks is exactly what we want.
    //
    // This is the structural piece that makes Phase 4 streaming possible
    // without blocking inbound reads. With the old architecture, a long
    // turn held `&mut client` for its entire duration, so no inbound
    // stanzas were processed until it finished. With the writer channel,
    // each turn runs in its own spawned task, the read loop stays
    // responsive, and the per-(surface, conversation) lock inside
    // [`Dispatcher`] keeps same-conversation turns from racing.
    let (out_tx, mut out_rx) = mpsc::channel::<Stanza>(64);

    loop {
        tokio::select! {
            // Bias toward draining outbound first. If the read loop and a
            // turn task are both ready, we'd rather get the response on
            // the wire promptly than read another inbound stanza first.
            biased;

            Some(stanza) = out_rx.recv() => {
                // The unselected `client.next()` future has been dropped
                // by select!, so `&mut client` is free here. Errors from
                // send_stanza are fatal to the session — caller will
                // reconnect via `serve()`'s backoff loop.
                if let Err(e) = client.send_stanza(stanza).await {
                    error!(%e, "failed to send outbound XMPP stanza");
                    return Err(e.into());
                }
            }

            event_opt = client.next() => {
                let event = match event_opt {
                    Some(e) => e,
                    None => {
                        // Stream ended without an explicit Disconnected
                        // event. Treat as a clean shutdown — caller will
                        // reconnect.
                        return Ok(());
                    }
                };
                match event {
                    Event::Online { bound_jid, resumed } => {
                        if resumed {
                            info!(%bound_jid, "XMPP stream resumed");
                        } else {
                            info!(%bound_jid, "XMPP online");
                            // Initial presence and MUC joins are sent
                            // inline rather than through the channel.
                            // We're already in the read loop with `&mut
                            // client`, nothing else competes for it at
                            // this instant, and these are bounded
                            // setup-time operations — no reason to
                            // round-trip them through the writer queue.
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
                        // Spawn the turn handler so the read loop can
                        // get back to selecting immediately. The handler
                        // pushes its reply stanzas into `out_tx`; the
                        // read loop drains them on the next iteration.
                        let cfg = config.clone();
                        let disp = dispatcher.clone();
                        let tx = out_tx.clone();
                        tokio::spawn(async move {
                            let message = match Message::try_from(stanza) {
                                Ok(m) => m,
                                Err(_) => return,
                            };
                            match message.type_ {
                                MessageType::Chat => {
                                    handle_chat_message(&message, &cfg, &disp, &tx).await;
                                }
                                MessageType::Groupchat => {
                                    handle_groupchat_message(&message, &cfg, &disp, &tx).await;
                                }
                                _ => {
                                    debug!(
                                        ty = ?message.type_,
                                        from = ?message.from,
                                        "ignoring message of unhandled type"
                                    );
                                }
                            }
                        });
                    }
                }
            }
        }
    }
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
/// Runs inside a spawned task per inbound message. Outbound replies go
/// through `out_tx` to the read loop, which owns `&mut Client` and is the
/// only thing that ever calls `send_stanza`. Errors are logged here and
/// not propagated — the spawned task simply ends.
async fn handle_chat_message(
    message: &Message,
    config: &XmppConfig,
    dispatcher: &Dispatcher,
    out_tx: &mpsc::Sender<Stanza>,
) {
    // Extract sender bare JID — drop messages with no `from` (server pings,
    // some chat-state notifications) or those that don't parse cleanly.
    let from_jid = match message.from.as_ref() {
        Some(j) => j,
        None => {
            debug!("dropping chat message with no `from`");
            return;
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
            return;
        }
    };

    // Allowlist enforcement.
    if !is_allowed(config, &sender_bare) {
        debug!(from = %sender_bare, "dropping chat message from non-allowlisted JID");
        return;
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
        if let Err(e) = send_chat_reply(out_tx, &sender_bare, &reply_text).await {
            warn!(%e, from = %sender_bare, "failed to enqueue chat reply (channel closed?)");
        }
        return;
    }

    // Build the turn request and dispatch. Hand the response stream to
    // the streaming sender, which emits an initial message immediately and
    // throttled XEP-0308 corrections thereafter. TurnEvent::Complete and
    // TurnEvent::Error are both handled inside `stream_single_message`.
    let turn_req = TurnRequest {
        surface_id: "xmpp".into(),
        conversation_id,
        message_text: body,
    };
    let rx = dispatcher.dispatch(turn_req).await;

    stream_single_message(
        out_tx,
        &sender_bare,
        MessageType::Chat,
        rx,
        STREAM_THROTTLE,
    )
    .await;
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
    message: &Message,
    config: &XmppConfig,
    dispatcher: &Dispatcher,
    out_tx: &mpsc::Sender<Stanza>,
) {
    let from_jid = match message.from.as_ref() {
        Some(j) => j,
        None => {
            debug!("dropping groupchat message with no `from`");
            return;
        }
    };
    let room_bare = from_jid.to_bare();
    let sender_nick = match from_jid.resource() {
        Some(r) => r.as_str(),
        None => {
            // No resource = sent by the room itself (subject changes,
            // history end markers, etc). Nothing to respond to.
            debug!(room = %room_bare, "dropping groupchat with no resource (room-level stanza)");
            return;
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
            return;
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
        return;
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
            return;
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
                return;
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
        if let Err(e) = send_groupchat_reply(out_tx, &room_bare, &reply_text).await {
            warn!(%e, room = %room_bare, "failed to enqueue groupchat reply (channel closed?)");
        }
        return;
    }

    // Build the turn request and dispatch. Same streaming path as DMs —
    // the only difference is the message type and the recipient is the
    // room JID, not a user. In MUC, XEP-0308 corrections are addressed to
    // the room and every occupant's client sees the in-place updates
    // (Conversations/Cheogram/Gajim handle this cleanly; older clients
    // see N separate messages, which the throttle keeps to a minimum).
    let turn_req = TurnRequest {
        surface_id: "xmpp".into(),
        conversation_id,
        message_text: dispatch_body,
    };
    let rx = dispatcher.dispatch(turn_req).await;

    stream_single_message(
        out_tx,
        &room_bare,
        MessageType::Groupchat,
        rx,
        STREAM_THROTTLE,
    )
    .await;
}

/// Build a `<message type="chat">` stanza addressed to `to` and push it
/// onto the writer channel. Replying to the bare JID (not a specific
/// resource) lets the user's server pick the best resource to deliver to
/// — handles roaming between Conversations on phone and Gajim on desktop.
///
/// Returns `Err` only if the writer channel has been closed, which means
/// the read loop has already exited and this reply will not make it to
/// the wire. Callers log and move on.
async fn send_chat_reply(
    out_tx: &mpsc::Sender<Stanza>,
    to: &BareJid,
    body: &str,
) -> Result<(), mpsc::error::SendError<Stanza>> {
    let to_jid = Jid::from(to.clone());
    let mut reply = Message::new(Some(to_jid));
    reply.type_ = MessageType::Chat;
    reply.bodies.insert(Lang(String::new()), body.to_string());
    out_tx.send(reply.into()).await
}

/// Build a `<message type="groupchat">` stanza addressed to a MUC room
/// and push it onto the writer channel. The destination is the bare room
/// JID — the server fans the message out to every occupant including the
/// sender (which is what the loop trap drop in [`handle_groupchat_message`]
/// is there to handle).
async fn send_groupchat_reply(
    out_tx: &mpsc::Sender<Stanza>,
    room: &BareJid,
    body: &str,
) -> Result<(), mpsc::error::SendError<Stanza>> {
    let to_jid = Jid::from(room.clone());
    let mut reply = Message::new(Some(to_jid));
    reply.type_ = MessageType::Groupchat;
    reply.bodies.insert(Lang(String::new()), body.to_string());
    out_tx.send(reply.into()).await
}

// ---------------------------------------------------------------------------
// XEP-0308 streaming corrections
// ---------------------------------------------------------------------------

/// Stream a dispatcher response to one recipient using XEP-0308 Last
/// Message Correction. The first non-empty content arriving from the
/// dispatcher is sent immediately as a fresh `<message>` with an explicit
/// `id`; subsequent text accumulates and, once `throttle` has elapsed
/// since the last send AND the displayed text would actually change, a
/// correction stanza referencing the original id is emitted.
///
/// On `TurnEvent::Complete(text)` the canonical final text replaces
/// whatever was streamed (always sent if it differs from what's currently
/// displayed). On `TurnEvent::Error(e)` an error message is sent — either
/// as the initial message (if nothing has streamed yet) or as a final
/// correction (if a streaming message is already on the wire).
///
/// The recipient is the bare JID for DMs and the bare room JID for MUC.
/// `msg_type` selects between `Chat` and `Groupchat`. The throttle is
/// taken as a parameter so tests can drive the function with a much
/// smaller value than production's [`STREAM_THROTTLE`].
///
/// Returns when the dispatcher rx is exhausted (Complete/Error or the
/// channel closing). If the writer channel has been closed mid-stream
/// — which means the read loop has gone away during a session reconnect
/// — the function logs once and returns silently. Whatever was already
/// on the wire is whatever the user got.
async fn stream_single_message(
    out_tx: &mpsc::Sender<Stanza>,
    to: &BareJid,
    msg_type: MessageType,
    mut rx: tokio::sync::mpsc::Receiver<TurnEvent>,
    throttle: Duration,
) {
    let mut full_text = String::new();
    let mut message_id: Option<Id> = None;
    let mut last_send: Option<Instant> = None;
    let mut last_sent_text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => {
                full_text.push_str(&chunk);
                // The dispatcher can produce empty initial chunks during
                // claude's startup phase — don't send a hollow stanza.
                if full_text.is_empty() {
                    continue;
                }

                if message_id.is_none() {
                    // First non-empty content: send the initial message
                    // immediately, no throttle. Stash the id so future
                    // corrections can reference it via Replace.
                    let id = new_message_id();
                    if let Err(e) =
                        send_initial(out_tx, to, msg_type.clone(), &id, &full_text).await
                    {
                        warn!(%e, %to, "failed to enqueue initial streaming message");
                        return;
                    }
                    message_id = Some(id);
                    last_send = Some(Instant::now());
                    last_sent_text = full_text.clone();
                } else if let Some(last) = last_send {
                    // Mid-stream correction. Two gates: throttle elapsed
                    // AND the visible text would actually change. The
                    // text-changed check matters because the dispatcher
                    // can emit chunks that resolve into nothing (think
                    // tool-use roundtrips that produce no user-visible
                    // delta) and we don't want to spam the room with
                    // identical corrections.
                    let now = Instant::now();
                    if now.duration_since(last) >= throttle && full_text != last_sent_text {
                        let id = message_id.as_ref().expect("checked above");
                        if let Err(e) =
                            send_correction(out_tx, to, msg_type.clone(), id, &full_text).await
                        {
                            warn!(%e, %to, "failed to enqueue streaming correction");
                            return;
                        }
                        last_send = Some(now);
                        last_sent_text = full_text.clone();
                    }
                }
            }
            TurnEvent::Complete(text) => {
                // The dispatcher's Complete carries the canonical final
                // text. Replace whatever we streamed with this — even if
                // throttling skipped the last chunk, the user always sees
                // the right thing at the end.
                full_text = text;
                if full_text.is_empty() {
                    if message_id.is_none() {
                        // No prior chunks AND no final text. Dispatcher
                        // produced literally nothing — log and bail, same
                        // as the pre-streaming behavior.
                        warn!(%to, "dispatcher produced empty response — sending nothing");
                    }
                    // If we DID stream something and Complete is empty,
                    // leave the streamed text as the final state. Empty
                    // Complete means "no further changes," not "blank
                    // out the message."
                    return;
                }
                match &message_id {
                    None => {
                        // Single-shot path: nothing streamed, send one
                        // initial message and we're done.
                        let id = new_message_id();
                        if let Err(e) =
                            send_initial(out_tx, to, msg_type, &id, &full_text).await
                        {
                            warn!(%e, %to, "failed to enqueue final message");
                        }
                    }
                    Some(id) => {
                        // Streamed path: emit a final correction iff the
                        // canonical text differs from what's currently on
                        // the wire. Otherwise the last mid-stream send
                        // already showed the right thing.
                        if full_text != last_sent_text {
                            if let Err(e) = send_correction(
                                out_tx,
                                to,
                                msg_type,
                                id,
                                &full_text,
                            )
                            .await
                            {
                                warn!(%e, %to, "failed to enqueue final correction");
                            }
                        }
                    }
                }
                return;
            }
            TurnEvent::Error(e) => {
                let err_text = format!("Something went sideways: {e}");
                match &message_id {
                    None => {
                        // No prior message — send the error as a single
                        // initial stanza. User sees only the error, which
                        // is the right thing.
                        let id = new_message_id();
                        if let Err(send_err) =
                            send_initial(out_tx, to, msg_type, &id, &err_text).await
                        {
                            warn!(%send_err, %to, "failed to enqueue error message");
                        }
                    }
                    Some(id) => {
                        // Streaming was in progress — replace the partial
                        // reply with the error so the user doesn't see a
                        // truncated answer dangling next to the failure.
                        if let Err(send_err) =
                            send_correction(out_tx, to, msg_type, id, &err_text).await
                        {
                            warn!(%send_err, %to, "failed to enqueue error correction");
                        }
                    }
                }
                return;
            }
        }
    }

    // Dispatcher rx closed without Complete or Error. Should not happen
    // under normal operation — the dispatcher always emits one of those
    // as the terminal event. Log and move on.
    if message_id.is_some() {
        debug!(%to, "dispatcher rx closed mid-stream without Complete/Error");
    }
}

/// Generate a unique message id for the first stanza of a streaming
/// turn. Subsequent corrections reference this id via the Replace
/// payload, so it has to be unique within the conversation. UUID v4 is
/// overkill in entropy but cheap and already a workspace dependency.
/// The `sid-` prefix makes it easy to spot Sid's stanzas in raw XMPP
/// traces.
fn new_message_id() -> Id {
    Id(format!("sid-{}", uuid::Uuid::new_v4()))
}

/// Build and enqueue the initial `<message>` stanza of a streaming turn.
/// The id is set explicitly (not left for tokio-xmpp's auto-fill) so
/// subsequent corrections can reference it via the Replace payload.
async fn send_initial(
    out_tx: &mpsc::Sender<Stanza>,
    to: &BareJid,
    msg_type: MessageType,
    id: &Id,
    body: &str,
) -> Result<(), mpsc::error::SendError<Stanza>> {
    let to_jid = Jid::from(to.clone());
    let mut msg = Message::new(Some(to_jid));
    msg.id = Some(id.clone());
    msg.type_ = msg_type;
    msg.bodies.insert(Lang(String::new()), body.to_string());
    out_tx.send(msg.into()).await
}

/// Build and enqueue a XEP-0308 correction stanza referencing
/// `original_id` with the updated body. The correction stanza itself
/// gets an auto-generated id from tokio-xmpp at send time — only the
/// `<replace id="..."/>` payload references the original.
async fn send_correction(
    out_tx: &mpsc::Sender<Stanza>,
    to: &BareJid,
    msg_type: MessageType,
    original_id: &Id,
    body: &str,
) -> Result<(), mpsc::error::SendError<Stanza>> {
    let to_jid = Jid::from(to.clone());
    let mut msg = Message::new(Some(to_jid));
    msg.type_ = msg_type;
    msg.bodies.insert(Lang(String::new()), body.to_string());
    msg = msg.with_payload(Replace {
        id: original_id.clone(),
    });
    out_tx.send(msg.into()).await
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

    // -----------------------------------------------------------------------
    // Phase 4 — XEP-0308 streaming corrections
    // -----------------------------------------------------------------------

    /// Pull a Message out of a Stanza, panic if it isn't one. Tests only
    /// emit chat/groupchat messages, so anything else is a bug.
    fn unwrap_message(stanza: Stanza) -> Message {
        match stanza {
            Stanza::Message(m) => m,
            other => panic!("expected Stanza::Message, got {:?}", other),
        }
    }

    /// Read the body text of a Message. Bodies live in a BTreeMap keyed by
    /// language tag — for our purposes there's exactly one entry under the
    /// empty-lang key.
    fn body_text(msg: &Message) -> &str {
        msg.bodies
            .values()
            .next()
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Try to extract a Replace payload from a Message. Returns Some only
    /// if the message is a XEP-0308 correction.
    fn replace_payload(msg: &Message) -> Option<Replace> {
        msg.payloads
            .iter()
            .find_map(|el| Replace::try_from(el.clone()).ok())
    }

    fn test_jid() -> BareJid {
        BareJid::from_str("alice@example.org").unwrap()
    }

    /// Drive `stream_single_message` end-to-end with a feeder closure.
    /// The closure gets the turn-event sender and may emit any sequence
    /// of TurnEvents; the function returns when both halves complete.
    /// Returns the full ordered list of stanzas pushed onto the writer
    /// channel.
    async fn run_stream_test<F, Fut>(throttle: Duration, feeder: F) -> Vec<Stanza>
    where
        F: FnOnce(tokio::sync::mpsc::Sender<TurnEvent>) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        // Capacity 1 on the turn channel gives us natural backpressure:
        // the feeder's `.send().await` only returns once stream_single_
        // message has accepted the prior event, which means we can drive
        // the function step by step without races.
        let (turn_tx, turn_rx) = tokio::sync::mpsc::channel::<TurnEvent>(1);
        let (out_tx, mut out_rx) = mpsc::channel::<Stanza>(16);
        let to = test_jid();

        let stream_handle = tokio::spawn(async move {
            stream_single_message(&out_tx, &to, MessageType::Chat, turn_rx, throttle).await;
        });

        feeder(turn_tx).await;

        stream_handle.await.expect("stream task panicked");

        let mut stanzas = Vec::new();
        while let Ok(s) = out_rx.try_recv() {
            stanzas.push(s);
        }
        stanzas
    }

    #[tokio::test]
    async fn stream_complete_only_sends_one_initial() {
        // The simplest case: dispatcher emits no chunks, just a final
        // Complete with the canonical text. We should send exactly one
        // initial message stanza.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::Complete("hello world".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(stanzas.len(), 1, "expected exactly one stanza");
        let msg = unwrap_message(stanzas.into_iter().next().unwrap());
        assert_eq!(body_text(&msg), "hello world");
        assert!(msg.id.is_some(), "initial message must have an explicit id");
        assert!(
            replace_payload(&msg).is_none(),
            "initial message must not carry a Replace payload"
        );
        assert_eq!(msg.type_, MessageType::Chat);
    }

    #[tokio::test]
    async fn stream_chunk_then_complete_same_text_sends_only_initial() {
        // First chunk lands → initial stanza. Complete arrives with the
        // same canonical text → no extra correction (the user already
        // sees the right thing).
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::TextChunk("hi there".into())).await.unwrap();
            tx.send(TurnEvent::Complete("hi there".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(stanzas.len(), 1, "duplicate Complete should not trigger a correction");
        let msg = unwrap_message(stanzas.into_iter().next().unwrap());
        assert_eq!(body_text(&msg), "hi there");
        assert!(msg.id.is_some());
        assert!(replace_payload(&msg).is_none());
    }

    #[tokio::test]
    async fn stream_chunk_then_complete_different_text_sends_correction() {
        // Chunk arrives, initial sent. Complete arrives with a different
        // (longer) canonical text → final correction emitted referencing
        // the initial stanza's id.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::TextChunk("hi".into())).await.unwrap();
            tx.send(TurnEvent::Complete("hi there friend".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(stanzas.len(), 2);
        let mut iter = stanzas.into_iter();

        let initial = unwrap_message(iter.next().unwrap());
        assert_eq!(body_text(&initial), "hi");
        assert!(replace_payload(&initial).is_none());
        let initial_id = initial.id.expect("initial id");

        let correction = unwrap_message(iter.next().unwrap());
        assert_eq!(body_text(&correction), "hi there friend");
        let replace = replace_payload(&correction).expect("Replace payload");
        assert_eq!(
            replace.id, initial_id,
            "correction must reference the initial message's id"
        );
    }

    #[tokio::test]
    async fn stream_throttle_collapses_rapid_chunks() {
        // Three chunks back to back with a non-zero throttle. The first
        // chunk fires the initial; the second and third are within the
        // throttle window so neither triggers a correction; Complete
        // emits one final correction with the canonical text.
        let stanzas = run_stream_test(Duration::from_secs(60), |tx| async move {
            tx.send(TurnEvent::TextChunk("a".into())).await.unwrap();
            tx.send(TurnEvent::TextChunk("b".into())).await.unwrap();
            tx.send(TurnEvent::TextChunk("c".into())).await.unwrap();
            tx.send(TurnEvent::Complete("abc".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(
            stanzas.len(),
            2,
            "expected initial + final correction; mid-chunks must be throttled"
        );
        let mut iter = stanzas.into_iter();
        assert_eq!(body_text(&unwrap_message(iter.next().unwrap())), "a");
        let final_msg = unwrap_message(iter.next().unwrap());
        assert_eq!(body_text(&final_msg), "abc");
        assert!(replace_payload(&final_msg).is_some());
    }

    #[tokio::test]
    async fn stream_zero_throttle_emits_correction_per_chunk() {
        // With throttle = 0, every chunk after the first triggers a
        // correction (provided the visible text actually changes).
        // Complete with the same final text adds no extra correction.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::TextChunk("a".into())).await.unwrap();
            tx.send(TurnEvent::TextChunk("b".into())).await.unwrap();
            tx.send(TurnEvent::TextChunk("c".into())).await.unwrap();
            tx.send(TurnEvent::Complete("abc".into())).await.unwrap();
            drop(tx);
        })
        .await;

        // initial("a") + correction("ab") + correction("abc") + Complete same → no extra
        assert_eq!(stanzas.len(), 3);
        let bodies: Vec<String> = stanzas
            .iter()
            .map(|s| {
                let m = match s {
                    Stanza::Message(m) => m,
                    _ => panic!(),
                };
                body_text(m).to_string()
            })
            .collect();
        assert_eq!(bodies, vec!["a", "ab", "abc"]);
    }

    #[tokio::test]
    async fn stream_error_before_any_text_sends_single_error_message() {
        // Dispatcher fails before producing any text → one initial
        // message containing the error string. No correction needed
        // because there's nothing to replace.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::Error("subprocess crashed".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(stanzas.len(), 1);
        let msg = unwrap_message(stanzas.into_iter().next().unwrap());
        assert_eq!(body_text(&msg), "Something went sideways: subprocess crashed");
        assert!(msg.id.is_some());
        assert!(replace_payload(&msg).is_none());
    }

    #[tokio::test]
    async fn stream_error_after_chunks_replaces_partial_with_error() {
        // Dispatcher streams some text, then errors out. The user sees
        // the partial reply briefly, then it gets replaced (XEP-0308
        // correction) with the error message. Better than leaving a
        // truncated half-answer dangling next to the failure.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::TextChunk("starting to answ".into())).await.unwrap();
            tx.send(TurnEvent::Error("connection lost".into())).await.unwrap();
            drop(tx);
        })
        .await;

        assert_eq!(stanzas.len(), 2);
        let mut iter = stanzas.into_iter();

        let initial = unwrap_message(iter.next().unwrap());
        assert_eq!(body_text(&initial), "starting to answ");
        assert!(replace_payload(&initial).is_none());
        let initial_id = initial.id.expect("initial id");

        let correction = unwrap_message(iter.next().unwrap());
        assert_eq!(
            body_text(&correction),
            "Something went sideways: connection lost"
        );
        let replace = replace_payload(&correction).expect("Replace payload");
        assert_eq!(replace.id, initial_id);
    }

    #[tokio::test]
    async fn stream_empty_complete_with_no_chunks_sends_nothing() {
        // Edge case: dispatcher closes the stream with an empty Complete
        // and no chunks. Mirror the pre-streaming behavior — log and
        // send nothing.
        let stanzas = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::Complete(String::new())).await.unwrap();
            drop(tx);
        })
        .await;

        assert!(stanzas.is_empty(), "no stanzas should be sent for empty Complete");
    }

    #[tokio::test]
    async fn stream_initial_id_is_unique_per_call() {
        // Two independent streaming calls should use different message
        // ids, so a correction in one turn never accidentally references
        // the wrong message in another turn. UUID v4 makes this trivially
        // true; the test guards against accidental introduction of a
        // shared/static id later.
        let s1 = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::Complete("first".into())).await.unwrap();
            drop(tx);
        })
        .await;
        let s2 = run_stream_test(Duration::ZERO, |tx| async move {
            tx.send(TurnEvent::Complete("second".into())).await.unwrap();
            drop(tx);
        })
        .await;

        let id1 = unwrap_message(s1.into_iter().next().unwrap()).id.unwrap();
        let id2 = unwrap_message(s2.into_iter().next().unwrap()).id.unwrap();
        assert_ne!(id1, id2);
    }
}
