//! Configuration for the Discord channel adapter.

use std::collections::HashSet;

use tracing::error;

/// How to render streaming responses in Discord.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamMode {
    /// Edit a single message in place as chunks arrive.
    SingleMessage,
    /// Collect full response, split at 2000-char boundaries, send each.
    MultiMessage,
}

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub bot_token: String,
    pub allowed_user_ids: HashSet<u64>,
    pub mention_only: bool,
    pub stream_mode: StreamMode,
}

impl DiscordConfig {
    /// Parse configuration from environment variables. Returns `None` if the
    /// adapter is disabled or required fields are missing.
    pub fn from_env() -> Option<Self> {
        if std::env::var("COMPANION_DISCORD_ENABLE").ok()?.as_str() != "1" {
            return None;
        }

        let token_file = match std::env::var("COMPANION_DISCORD_BOT_TOKEN_FILE") {
            Ok(p) => p,
            Err(_) => {
                error!("COMPANION_DISCORD_BOT_TOKEN_FILE not set");
                return None;
            }
        };
        let bot_token = match std::fs::read_to_string(&token_file) {
            Ok(t) => {
                let trimmed = t.trim().to_string();
                if trimmed.is_empty() {
                    error!(path = %token_file, "Discord bot token file is empty");
                    return None;
                }
                trimmed
            }
            Err(e) => {
                error!(path = %token_file, %e, "failed to read Discord bot token file");
                return None;
            }
        };

        let allowed_user_ids: HashSet<u64> = std::env::var("COMPANION_DISCORD_ALLOWED_USER_IDS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                match s.parse::<u64>() {
                    Ok(id) => Some(id),
                    Err(_) => {
                        error!(value = s, "invalid Discord user ID, skipping");
                        None
                    }
                }
            })
            .collect();

        let mention_only = std::env::var("COMPANION_DISCORD_MENTION_ONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let stream_mode = match std::env::var("COMPANION_DISCORD_STREAM_MODE")
            .unwrap_or_default()
            .as_str()
        {
            "multi_message" | "multi-message" => StreamMode::MultiMessage,
            _ => StreamMode::SingleMessage,
        };

        Some(Self {
            bot_token,
            allowed_user_ids,
            mention_only,
            stream_mode,
        })
    }

    /// Whether the given user ID is in the allowlist (Owner trust).
    pub fn is_allowed(&self, user_id: u64) -> bool {
        self.allowed_user_ids.contains(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_allowed_with_populated_list() {
        let config = DiscordConfig {
            bot_token: "test".into(),
            allowed_user_ids: HashSet::from([123456789, 987654321]),
            mention_only: true,
            stream_mode: StreamMode::SingleMessage,
        };
        assert!(config.is_allowed(123456789));
        assert!(config.is_allowed(987654321));
        assert!(!config.is_allowed(111111111));
    }

    #[test]
    fn is_allowed_empty_list_denies_all() {
        let config = DiscordConfig {
            bot_token: "test".into(),
            allowed_user_ids: HashSet::new(),
            mention_only: true,
            stream_mode: StreamMode::SingleMessage,
        };
        assert!(!config.is_allowed(123456789));
    }
}
