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
use xmpp_parsers::message::Lang;
use xmpp_parsers::presence::{Presence, Show as PresenceShow, Type as PresenceType};

use crate::dispatcher::Dispatcher;

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

// ---------------------------------------------------------------------------
// Serve — entry point. Phase 2 lands the connect/auth/presence path and the
// reconnect loop. DM/MUC message handling are Phase 3+ and live downstream.
// ---------------------------------------------------------------------------

/// Start the XMPP adapter. Blocks until `shutdown` fires. On any connection
/// error the loop reconnects with exponential backoff so the bot survives
/// Prosody restarts during nixos-rebuild.
pub async fn serve(
    _dispatcher: Arc<Dispatcher>,
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
        let session = run_session(cfg);

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
async fn run_session(config: Arc<XmppConfig>) -> Result<(), tokio_xmpp::Error> {
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
                    // TODO(channel-xmpp Phase 5): MUC join goes here.
                    if !config.muc_rooms.is_empty() {
                        debug!(
                            count = config.muc_rooms.len(),
                            "MUC join deferred to Phase 5"
                        );
                    }
                }
            }
            Event::Disconnected(err) => {
                warn!(%err, "XMPP disconnected");
                return Err(err);
            }
            Event::Stanza(stanza) => {
                // TODO(channel-xmpp Phase 3): dispatch via _dispatcher
                debug!(?stanza, "XMPP stanza received (handler not yet wired)");
            }
        }
    }

    Ok(())
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
}
