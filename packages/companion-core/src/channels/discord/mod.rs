//! Discord channel adapter — receives messages from a Discord bot via the
//! Gateway WebSocket, routes them through the dispatcher, and streams
//! responses back.
//!
//! Runs as an async task inside companion-core (not a separate process).
//! Env-gated via `COMPANION_DISCORD_ENABLE=1`.

pub mod command;
pub mod config;

pub use config::DiscordConfig;

use std::sync::Arc;
use std::time::{Duration, Instant};

use serenity::all::{
    ChannelId, Context, EventHandler, GatewayIntents, Message, Ready, UserId,
};
use serenity::async_trait;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::channels::util::split_message;
use crate::dispatcher::{Dispatcher, TrustLevel, TurnEvent, TurnRequest};

use config::StreamMode;

const DISCORD_MAX_LEN: usize = 2000;

/// Throttle between message edits during streaming (same as telegram).
const EDIT_INTERVAL: Duration = Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// Event handler
// ---------------------------------------------------------------------------

struct Handler {
    dispatcher: Arc<Dispatcher>,
    config: Arc<DiscordConfig>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        let bot_id = ready.user.id;
        info!(
            bot_user = %ready.user.name,
            bot_id = bot_id.get(),
            guilds = ready.guilds.len(),
            "Discord gateway ready"
        );

        // Stash the bot's own user ID in the cache for loop prevention.
        // serenity's cache feature does this automatically on Ready.
        let _ = ctx;
        let _ = bot_id;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Loop prevention: drop our own messages and other bots.
        if msg.author.bot {
            return;
        }

        let is_dm = msg.guild_id.is_none();

        if is_dm {
            self.handle_dm(&ctx, &msg).await;
        } else {
            self.handle_guild_message(&ctx, &msg).await;
        }
    }
}

impl Handler {
    async fn handle_dm(&self, ctx: &Context, msg: &Message) {
        let author_id = msg.author.id.get();
        let trust = if self.config.is_allowed(author_id) {
            TrustLevel::Owner
        } else {
            TrustLevel::Anonymous
        };

        let conversation_id = author_id.to_string();
        let text = msg.content.clone();

        if text.is_empty() {
            return;
        }

        info!(
            user = %msg.author.name,
            user_id = author_id,
            ?trust,
            "discord DM"
        );

        // Bang commands.
        if text.starts_with('!') {
            let reply = command::handle("discord", &conversation_id, &text, &self.dispatcher).await;
            send_text(ctx, msg.channel_id, &reply).await;
            return;
        }

        self.dispatch_and_respond(ctx, msg.channel_id, &conversation_id, &text, trust)
            .await;
    }

    async fn handle_guild_message(&self, ctx: &Context, msg: &Message) {
        let bot_id = match ctx.cache.current_user().id.get() {
            0 => return, // cache not ready
            id => UserId::new(id),
        };

        // Check if the bot was mentioned.
        let mentioned = msg.mentions.iter().any(|u| u.id == bot_id);

        if self.config.mention_only && !mentioned {
            return;
        }

        // Strip the bot mention from the message body.
        let bot_mention = format!("<@{}>", bot_id.get());
        let bot_mention_nick = format!("<@!{}>", bot_id.get());
        let text = msg
            .content
            .replace(&bot_mention, "")
            .replace(&bot_mention_nick, "")
            .trim()
            .to_string();

        if text.is_empty() {
            return; // bare ping, nothing to dispatch
        }

        // Guild messages are always Anonymous — room membership is not identity.
        let conversation_id = msg.channel_id.get().to_string();

        info!(
            user = %msg.author.name,
            channel = msg.channel_id.get(),
            guild = ?msg.guild_id.map(|g| g.get()),
            "discord guild message"
        );

        // Bang commands.
        if text.starts_with('!') {
            let reply =
                command::handle("discord", &conversation_id, &text, &self.dispatcher).await;
            send_text(ctx, msg.channel_id, &reply).await;
            return;
        }

        self.dispatch_and_respond(
            ctx,
            msg.channel_id,
            &conversation_id,
            &text,
            TrustLevel::Anonymous,
        )
        .await;
    }

    async fn dispatch_and_respond(
        &self,
        ctx: &Context,
        channel_id: ChannelId,
        conversation_id: &str,
        text: &str,
        trust: TrustLevel,
    ) {
        let turn_req = TurnRequest {
            surface_id: "discord".into(),
            conversation_id: conversation_id.to_string(),
            message_text: text.to_string(),
            trust,
        };

        let mut rx = self.dispatcher.dispatch(turn_req).await;

        // Show typing indicator while processing.
        let typing = ctx.http.start_typing(channel_id);
        let _ = typing;

        match self.config.stream_mode {
            StreamMode::SingleMessage => {
                stream_single_message(ctx, channel_id, &mut rx).await;
            }
            StreamMode::MultiMessage => {
                collect_and_send(ctx, channel_id, &mut rx).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Response rendering
// ---------------------------------------------------------------------------

/// Edit-in-place streaming: send the first chunk, edit as more arrive.
async fn stream_single_message(
    ctx: &Context,
    channel_id: ChannelId,
    rx: &mut tokio::sync::mpsc::Receiver<TurnEvent>,
) {
    let mut accumulated = String::new();
    let mut sent_msg: Option<Message> = None;
    let mut last_edit_len: usize = 0;
    let mut last_edit = Instant::now();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => {
                accumulated.push_str(&chunk);

                let should_edit = last_edit.elapsed() >= EDIT_INTERVAL
                    || accumulated.len() - last_edit_len > 200;

                if should_edit {
                    let display = truncate_for_discord(&accumulated);
                    match &sent_msg {
                        None => match send_text(ctx, channel_id, &display).await {
                            Some(m) => {
                                sent_msg = Some(m);
                                last_edit_len = accumulated.len();
                                last_edit = Instant::now();
                            }
                            None => warn!("failed to send initial discord message"),
                        },
                        Some(m) => {
                            if let Err(e) = channel_id
                                .edit_message(&ctx.http, m.id, serenity::builder::EditMessage::new().content(&display))
                                .await
                            {
                                debug!(%e, "edit_message failed");
                            } else {
                                last_edit_len = accumulated.len();
                                last_edit = Instant::now();
                            }
                        }
                    }
                }
            }
            TurnEvent::Complete(text) => {
                let chunks = split_message(&text, DISCORD_MAX_LEN);

                if chunks.len() == 1 {
                    match &sent_msg {
                        None => {
                            send_text(ctx, channel_id, &chunks[0]).await;
                        }
                        Some(m) => {
                            let _ = channel_id
                                .edit_message(&ctx.http, m.id, serenity::builder::EditMessage::new().content(&chunks[0]))
                                .await;
                        }
                    }
                } else {
                    // Exceeded 2000 chars — delete streaming msg, send as multiple.
                    if let Some(m) = &sent_msg {
                        let _ = channel_id.delete_message(&ctx.http, m.id).await;
                    }
                    for chunk in &chunks {
                        send_text(ctx, channel_id, chunk).await;
                    }
                }
                return;
            }
            TurnEvent::Error(e) => {
                let err_text = format!("Something went sideways: {e}");
                match &sent_msg {
                    None => {
                        send_text(ctx, channel_id, &err_text).await;
                    }
                    Some(m) => {
                        let _ = channel_id
                            .edit_message(&ctx.http, m.id, serenity::builder::EditMessage::new().content(&err_text))
                            .await;
                    }
                }
                return;
            }
        }
    }

    // Channel closed without Complete — send whatever we have.
    if !accumulated.is_empty() {
        let chunks = split_message(&accumulated, DISCORD_MAX_LEN);
        match &sent_msg {
            None => {
                for chunk in &chunks {
                    send_text(ctx, channel_id, chunk).await;
                }
            }
            Some(m) => {
                if chunks.len() == 1 {
                    let _ = channel_id
                        .edit_message(&ctx.http, m.id, serenity::builder::EditMessage::new().content(&chunks[0]))
                        .await;
                } else {
                    let _ = channel_id.delete_message(&ctx.http, m.id).await;
                    for chunk in &chunks {
                        send_text(ctx, channel_id, chunk).await;
                    }
                }
            }
        }
    }
}

/// Multi-message mode: collect full response, split, send.
async fn collect_and_send(
    ctx: &Context,
    channel_id: ChannelId,
    rx: &mut tokio::sync::mpsc::Receiver<TurnEvent>,
) {
    let mut full_text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => full_text.push_str(&chunk),
            TurnEvent::Complete(text) => {
                full_text = text;
                break;
            }
            TurnEvent::Error(e) => {
                send_text(ctx, channel_id, &format!("Something went sideways: {e}")).await;
                return;
            }
        }
    }

    if full_text.is_empty() {
        return;
    }

    for chunk in split_message(&full_text, DISCORD_MAX_LEN) {
        send_text(ctx, channel_id, &chunk).await;
    }
}

/// Send a text message to a Discord channel. Returns the sent message on success.
async fn send_text(ctx: &Context, channel_id: ChannelId, text: &str) -> Option<Message> {
    match channel_id
        .send_message(&ctx.http, serenity::builder::CreateMessage::new().content(text))
        .await
    {
        Ok(m) => Some(m),
        Err(e) => {
            error!(%e, "failed to send Discord message");
            None
        }
    }
}

/// Truncate text to fit Discord's 2000-char limit during streaming.
fn truncate_for_discord(text: &str) -> String {
    if text.len() <= DISCORD_MAX_LEN {
        text.to_string()
    } else {
        let suffix = "...\n\n";
        let avail = DISCORD_MAX_LEN - suffix.len();
        let start = text.len() - avail;
        let break_at = text[start..]
            .find('\n')
            .map(|i| start + i + 1)
            .unwrap_or(start);
        format!("{}{}", suffix, &text[break_at..])
    }
}

// ---------------------------------------------------------------------------
// Serve loop
// ---------------------------------------------------------------------------

/// Run the Discord adapter. Blocks until shutdown is signaled.
pub async fn serve(
    dispatcher: Arc<Dispatcher>,
    config: DiscordConfig,
    shutdown: Arc<Notify>,
) {
    let config = Arc::new(config);
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        let handler = Handler {
            dispatcher: dispatcher.clone(),
            config: config.clone(),
        };

        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let client = serenity::Client::builder(&config.bot_token, intents)
            .event_handler(handler)
            .await;

        let mut client = match client {
            Ok(c) => c,
            Err(e) => {
                error!(%e, ?backoff, "failed to build Discord client — retrying after backoff");
                tokio::select! {
                    biased;
                    _ = shutdown.notified() => {
                        info!("Discord adapter shutting down during backoff");
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        // Spawn the gateway connection. serenity handles heartbeat,
        // reconnect, and session resume internally.
        let shard_manager = client.shard_manager.clone();
        let shutdown_clone = shutdown.clone();

        // Task that waits for the shutdown signal and kills the shards.
        tokio::spawn(async move {
            shutdown_clone.notified().await;
            shard_manager.shutdown_all().await;
        });

        match client.start().await {
            Ok(()) => {
                // Clean exit — either shutdown was signaled or the gateway
                // closed gracefully.
                info!("Discord client exited cleanly");
                return;
            }
            Err(e) => {
                error!(%e, ?backoff, "Discord client error — reconnecting after backoff");
                tokio::select! {
                    biased;
                    _ = shutdown.notified() => {
                        info!("Discord adapter shutting down during backoff");
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_message() {
        let text = "Hello, world!";
        assert_eq!(truncate_for_discord(text), text);
    }

    #[test]
    fn truncate_long_message_fits_limit() {
        let text = "x".repeat(3000);
        let truncated = truncate_for_discord(&text);
        assert!(truncated.len() <= DISCORD_MAX_LEN);
    }

    #[test]
    fn split_respects_discord_limit() {
        let text = "hello ".repeat(500); // ~3000 chars
        let chunks = split_message(&text, DISCORD_MAX_LEN);
        for chunk in &chunks {
            assert!(chunk.len() <= DISCORD_MAX_LEN);
        }
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn mention_strip() {
        let bot_id = 123456789u64;
        let bot_mention = format!("<@{}>", bot_id);
        let bot_mention_nick = format!("<@!{}>", bot_id);

        let text = format!("{} hello world", bot_mention);
        let stripped = text
            .replace(&bot_mention, "")
            .replace(&bot_mention_nick, "")
            .trim()
            .to_string();
        assert_eq!(stripped, "hello world");

        let text2 = format!("{} hello world", bot_mention_nick);
        let stripped2 = text2
            .replace(&bot_mention, "")
            .replace(&bot_mention_nick, "")
            .trim()
            .to_string();
        assert_eq!(stripped2, "hello world");
    }
}
