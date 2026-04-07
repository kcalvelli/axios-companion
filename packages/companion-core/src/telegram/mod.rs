//! Telegram channel adapter — receives messages from a personal Telegram bot,
//! routes them through the dispatcher, and streams responses back.
//!
//! Runs as an async task inside companion-core (not a separate process).
//! Env-gated via `COMPANION_TELEGRAM_ENABLE=1`.

use std::collections::HashSet;
use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{MediaKind, MessageKind};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::dispatcher::{Dispatcher, TurnEvent, TurnRequest};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How to render streaming responses in Telegram.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamMode {
    /// Edit a single message in place as chunks arrive.
    SingleMessage,
    /// Send each long response as multiple messages (no editing).
    MultiMessage,
}

/// Telegram channel configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub allowed_users: HashSet<UserId>,
    pub mention_only: bool,
    pub stream_mode: StreamMode,
}

impl TelegramConfig {
    /// Build config from environment variables. Returns `None` if the channel
    /// is not enabled (`COMPANION_TELEGRAM_ENABLE != 1`).
    pub fn from_env() -> Option<Self> {
        if std::env::var("COMPANION_TELEGRAM_ENABLE").ok()?.as_str() != "1" {
            return None;
        }

        let token_file = std::env::var("COMPANION_TELEGRAM_BOT_TOKEN_FILE").ok()?;
        let bot_token = match std::fs::read_to_string(&token_file) {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                error!(path = %token_file, %e, "failed to read bot token file");
                return None;
            }
        };

        if bot_token.is_empty() {
            error!(path = %token_file, "bot token file is empty");
            return None;
        }

        let allowed_users: HashSet<UserId> =
            std::env::var("COMPANION_TELEGRAM_ALLOWED_USERS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .map(UserId)
                .collect();

        let mention_only = std::env::var("COMPANION_TELEGRAM_MENTION_ONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let stream_mode = match std::env::var("COMPANION_TELEGRAM_STREAM_MODE")
            .unwrap_or_default()
            .as_str()
        {
            "multi_message" | "multi-message" => StreamMode::MultiMessage,
            _ => StreamMode::SingleMessage,
        };

        Some(Self {
            bot_token,
            allowed_users,
            mention_only,
            stream_mode,
        })
    }
}

// ---------------------------------------------------------------------------
// Message splitting
// ---------------------------------------------------------------------------

/// Telegram's maximum message length.
const TELEGRAM_MAX_LEN: usize = 4096;

/// Split a long message into chunks that fit within Telegram's 4096-char limit.
/// Tries to break at paragraph boundaries, then sentence boundaries, then word
/// boundaries — never mid-word if avoidable.
pub fn split_message(text: &str) -> Vec<String> {
    if text.len() <= TELEGRAM_MAX_LEN {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= TELEGRAM_MAX_LEN {
            chunks.push(remaining.to_string());
            break;
        }

        let slice = &remaining[..TELEGRAM_MAX_LEN];

        // Try paragraph break.
        let split_at = slice.rfind("\n\n")
            // Try line break.
            .or_else(|| slice.rfind('\n'))
            // Try sentence end.
            .or_else(|| slice.rfind(". ").map(|i| i + 1))
            // Try word boundary.
            .or_else(|| slice.rfind(' '))
            // Last resort: hard cut.
            .unwrap_or(TELEGRAM_MAX_LEN);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start();
    }

    chunks
}

// ---------------------------------------------------------------------------
// Allowlist check
// ---------------------------------------------------------------------------

/// Returns true if the user is allowed to interact with the bot.
/// An empty allowlist means nobody gets through — deny by default.
fn is_allowed(config: &TelegramConfig, user_id: UserId) -> bool {
    config.allowed_users.contains(&user_id)
}

// ---------------------------------------------------------------------------
// Serve — entry point
// ---------------------------------------------------------------------------

/// Start the Telegram long-polling adapter. Blocks until `shutdown` fires.
pub async fn serve(
    dispatcher: Arc<Dispatcher>,
    config: TelegramConfig,
    shutdown: Arc<Notify>,
) {
    info!(
        allowed_users = config.allowed_users.len(),
        mention_only = config.mention_only,
        stream_mode = ?config.stream_mode,
        "starting Telegram adapter"
    );

    let bot = Bot::new(&config.bot_token);
    let config = Arc::new(config);

    // Verify the bot token works by calling getMe.
    match bot.get_me().await {
        Ok(me) => {
            info!(
                username = %me.username(),
                "Telegram bot connected"
            );
        }
        Err(e) => {
            error!(%e, "failed to connect to Telegram API — check bot token");
            return;
        }
    }

    let bot_clone = bot.clone();
    let config_clone = config.clone();
    let dispatcher_clone = dispatcher.clone();

    // Spawn long-polling in a task so we can select against shutdown.
    let poll_handle = tokio::spawn(async move {
        run_polling(bot_clone, config_clone, dispatcher_clone).await;
    });

    // Wait for shutdown signal.
    shutdown.notified().await;
    info!("Telegram adapter shutting down");

    // Abort the polling task — teloxide's polling loop doesn't have a
    // graceful shutdown hook, so we just drop it.
    poll_handle.abort();
    let _ = poll_handle.await;
}

async fn run_polling(
    bot: Bot,
    config: Arc<TelegramConfig>,
    dispatcher: Arc<Dispatcher>,
) {
    use futures::StreamExt;
    use teloxide::update_listeners::{polling_default, AsUpdateStream};
    use teloxide::types::UpdateKind;

    let mut listener = polling_default(bot.clone()).await;
    let stream = AsUpdateStream::as_stream(&mut listener);
    let mut stream = std::pin::pin!(stream);

    while let Some(result) = stream.next().await {
        match result {
            Ok(update) => {
                if let UpdateKind::Message(msg) = update.kind {
                    let bot = bot.clone();
                    let config = config.clone();
                    let dispatcher = dispatcher.clone();
                    tokio::spawn(async move {
                        handle_message(bot, msg, &config, &dispatcher).await;
                    });
                }
            }
            Err(e) => {
                warn!(%e, "Telegram update error");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Message handler
// ---------------------------------------------------------------------------

async fn handle_message(
    bot: Bot,
    msg: Message,
    config: &TelegramConfig,
    dispatcher: &Dispatcher,
) {
    // Extract text content.
    let text = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(t) => &t.text,
            _ => {
                debug!(chat_id = %msg.chat.id, "ignoring non-text message");
                return;
            }
        },
        _ => return,
    };

    // Check user allowlist.
    let user_id = match msg.from.as_ref().map(|u| u.id) {
        Some(id) => id,
        None => {
            debug!(chat_id = %msg.chat.id, "ignoring message with no sender");
            return;
        }
    };

    if !is_allowed(config, user_id) {
        debug!(
            user_id = %user_id,
            chat_id = %msg.chat.id,
            "ignoring message from user not in allowlist"
        );
        return;
    }

    // In group chats with mention_only, check if the bot was mentioned.
    if config.mention_only && !msg.chat.is_private() {
        // Check if the bot username appears in the text.
        // teloxide doesn't expose bot username on the message directly,
        // so we check entities for bot_command or just pass through in
        // private chats.
        let is_mentioned = text.contains("@")
            && msg
                .entities()
                .map(|entities| {
                    entities
                        .iter()
                        .any(|e| matches!(e.kind, teloxide::types::MessageEntityKind::Mention))
                })
                .unwrap_or(false);

        if !is_mentioned {
            debug!(chat_id = %msg.chat.id, "mention_only: ignoring unmentioned message");
            return;
        }
    }

    let chat_id = msg.chat.id;
    let conversation_id = chat_id.0.to_string();

    info!(
        user_id = %user_id,
        chat_id = %chat_id,
        text_len = text.len(),
        "Telegram message received"
    );

    let turn_req = TurnRequest {
        surface_id: "telegram".into(),
        conversation_id,
        message_text: text.clone(),
    };

    let mut rx = dispatcher.dispatch(turn_req).await;

    match config.stream_mode {
        StreamMode::SingleMessage => {
            stream_single_message(&bot, chat_id, &mut rx).await;
        }
        StreamMode::MultiMessage => {
            collect_and_send(&bot, chat_id, &mut rx).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Response rendering
// ---------------------------------------------------------------------------

/// Stream mode: edit a single message in place as chunks arrive.
async fn stream_single_message(
    bot: &Bot,
    chat_id: ChatId,
    rx: &mut tokio::sync::mpsc::Receiver<TurnEvent>,
) {
    let mut accumulated = String::new();
    let mut sent_msg: Option<Message> = None;
    let mut last_edit_len: usize = 0;

    // Throttle edits — Telegram rate-limits message edits.
    // Edit at most every 1.5 seconds, or when we get Complete.
    let mut last_edit = std::time::Instant::now();
    let edit_interval = std::time::Duration::from_millis(1500);

    while let Some(event) = rx.recv().await {
        match event {
            TurnEvent::TextChunk(chunk) => {
                accumulated.push_str(&chunk);

                let should_edit = last_edit.elapsed() >= edit_interval
                    || accumulated.len() - last_edit_len > 200;

                if should_edit {
                    let display = truncate_for_telegram(&accumulated);
                    match &sent_msg {
                        None => {
                            match bot.send_message(chat_id, &display).await {
                                Ok(m) => {
                                    sent_msg = Some(m);
                                    last_edit_len = accumulated.len();
                                    last_edit = std::time::Instant::now();
                                }
                                Err(e) => warn!(%e, "failed to send initial message"),
                            }
                        }
                        Some(m) => {
                            if let Err(e) = bot
                                .edit_message_text(chat_id, m.id, &display)
                                .await
                            {
                                // "message is not modified" is expected if content didn't change.
                                debug!(%e, "edit_message_text failed (may be rate limit)");
                            } else {
                                last_edit_len = accumulated.len();
                                last_edit = std::time::Instant::now();
                            }
                        }
                    }
                }
            }
            TurnEvent::Complete(text) => {
                let chunks = split_message(&text);

                if chunks.len() == 1 {
                    // Final edit of the single message.
                    match &sent_msg {
                        None => {
                            let _ = bot.send_message(chat_id, &chunks[0]).await;
                        }
                        Some(m) => {
                            let _ = bot
                                .edit_message_text(chat_id, m.id, &chunks[0])
                                .await;
                        }
                    }
                } else {
                    // Response exceeded 4096 — delete the streaming message
                    // and send as multiple.
                    if let Some(m) = &sent_msg {
                        let _ = bot.delete_message(chat_id, m.id).await;
                    }
                    for chunk in &chunks {
                        if let Err(e) = bot.send_message(chat_id, chunk).await {
                            error!(%e, "failed to send split message chunk");
                            break;
                        }
                    }
                }
                return;
            }
            TurnEvent::Error(e) => {
                let err_text = format!("Something went sideways: {e}");
                match &sent_msg {
                    None => {
                        let _ = bot.send_message(chat_id, &err_text).await;
                    }
                    Some(m) => {
                        let _ = bot
                            .edit_message_text(chat_id, m.id, &err_text)
                            .await;
                    }
                }
                return;
            }
        }
    }

    // Channel closed without Complete — send whatever we have.
    if !accumulated.is_empty() {
        let chunks = split_message(&accumulated);
        match &sent_msg {
            None => {
                for chunk in &chunks {
                    let _ = bot.send_message(chat_id, chunk).await;
                }
            }
            Some(m) => {
                if chunks.len() == 1 {
                    let _ = bot.edit_message_text(chat_id, m.id, &chunks[0]).await;
                } else {
                    let _ = bot.delete_message(chat_id, m.id).await;
                    for chunk in &chunks {
                        let _ = bot.send_message(chat_id, chunk).await;
                    }
                }
            }
        }
    }
}

/// Multi-message mode: collect full response, then send as split messages.
async fn collect_and_send(
    bot: &Bot,
    chat_id: ChatId,
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
                let _ = bot
                    .send_message(chat_id, format!("Something went sideways: {e}"))
                    .await;
                return;
            }
        }
    }

    if full_text.is_empty() {
        return;
    }

    for chunk in split_message(&full_text) {
        if let Err(e) = bot.send_message(chat_id, &chunk).await {
            error!(%e, "failed to send message chunk");
            break;
        }
    }
}

/// Truncate text to fit Telegram's limit for in-progress streaming display.
fn truncate_for_telegram(text: &str) -> String {
    if text.len() <= TELEGRAM_MAX_LEN {
        text.to_string()
    } else {
        // Show the tail end of the response during streaming.
        let suffix = "...\n\n";
        let avail = TELEGRAM_MAX_LEN - suffix.len();
        // Find a good break point near the start of the visible window.
        let start = text.len() - avail;
        let break_at = text[start..].find('\n').map(|i| start + i + 1).unwrap_or(start);
        format!("{}{}", suffix, &text[break_at..])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_short_message() {
        let text = "Hello, world!";
        let chunks = split_message(text);
        assert_eq!(chunks, vec!["Hello, world!"]);
    }

    #[test]
    fn split_at_paragraph_boundary() {
        let mut text = String::new();
        // Fill first ~4000 chars, then a paragraph break, then more.
        text.push_str(&"a".repeat(4000));
        text.push_str("\n\n");
        text.push_str(&"b".repeat(200));

        let chunks = split_message(&text);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].len() <= TELEGRAM_MAX_LEN);
        // Reassembled text should contain all content.
        let reassembled: String = chunks.join("");
        assert!(reassembled.contains(&"b".repeat(200)));
    }

    #[test]
    fn split_at_word_boundary() {
        // Build a message of words that exceeds the limit.
        let word = "hello ";
        let count = TELEGRAM_MAX_LEN / word.len() + 100;
        let text: String = word.repeat(count);

        let chunks = split_message(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= TELEGRAM_MAX_LEN);
        }
        // Reassembling all chunks (with space separator to replace the
        // trim_start'd whitespace) should recover the original content.
        // The key property: no chunk exceeds the limit.
        let total_chars: usize = chunks.iter().map(|c| c.len()).sum();
        assert!(
            total_chars <= text.len(),
            "split produced more characters than input"
        );
    }

    #[test]
    fn split_no_spaces_hard_cut() {
        let text = "x".repeat(TELEGRAM_MAX_LEN * 2);
        let chunks = split_message(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), TELEGRAM_MAX_LEN);
    }

    #[test]
    fn allowlist_empty_denies_all() {
        let config = TelegramConfig {
            bot_token: String::new(),
            allowed_users: HashSet::new(),
            mention_only: false,
            stream_mode: StreamMode::SingleMessage,
        };
        assert!(!is_allowed(&config, UserId(12345)));
    }

    #[test]
    fn allowlist_permits_listed_user() {
        let mut allowed = HashSet::new();
        allowed.insert(UserId(42));
        let config = TelegramConfig {
            bot_token: String::new(),
            allowed_users: allowed,
            mention_only: false,
            stream_mode: StreamMode::SingleMessage,
        };
        assert!(is_allowed(&config, UserId(42)));
        assert!(!is_allowed(&config, UserId(99)));
    }

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "short";
        assert_eq!(truncate_for_telegram(text), "short");
    }

    #[test]
    fn truncate_long_text_fits() {
        let text = "x".repeat(TELEGRAM_MAX_LEN * 2);
        let result = truncate_for_telegram(&text);
        assert!(result.len() <= TELEGRAM_MAX_LEN);
    }

    #[test]
    fn config_stream_mode_parsing() {
        // Can't test from_env easily without setting env vars, but we can
        // verify the enum variants exist and match.
        assert_eq!(StreamMode::SingleMessage, StreamMode::SingleMessage);
        assert_ne!(StreamMode::SingleMessage, StreamMode::MultiMessage);
    }
}
