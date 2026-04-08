//! Shared utilities for channel adapters.
//!
//! Anything that more than one channel adapter needs lives here. Right now
//! that's just the message-splitting algorithm — telegram caps at 4096 chars
//! by API rule, xmpp caps around 3000 by client comfort. Same algorithm,
//! different cap.

/// Split a long message into chunks that fit within `max_chars`. Tries to
/// break at paragraph boundaries first, then line boundaries, then sentence
/// boundaries, then word boundaries — never mid-word if it can be avoided.
/// Falls back to a hard cut at `max_chars` only if no whitespace exists in
/// the window.
pub fn split_message(text: &str, max_chars: usize) -> Vec<String> {
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_chars {
            chunks.push(remaining.to_string());
            break;
        }

        let slice = &remaining[..max_chars];

        // Try paragraph break.
        let split_at = slice.rfind("\n\n")
            // Try line break.
            .or_else(|| slice.rfind('\n'))
            // Try sentence end.
            .or_else(|| slice.rfind(". ").map(|i| i + 1))
            // Try word boundary.
            .or_else(|| slice.rfind(' '))
            // Last resort: hard cut.
            .unwrap_or(max_chars);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start();
    }

    chunks
}

/// Format a unix timestamp as a human-readable "time since" — "just now",
/// "5m ago", "3h ago", "2d ago". Used by `/status` slash commands across
/// every channel.
pub fn format_timestamp(ts: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let dt = UNIX_EPOCH + Duration::from_secs(ts as u64);
    let elapsed = dt.elapsed().unwrap_or_default();
    let mins = elapsed.as_secs() / 60;
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if mins < 1440 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / 1440)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_message_returns_single_chunk() {
        let chunks = split_message("Hello, world!", 4096);
        assert_eq!(chunks, vec!["Hello, world!"]);
    }

    #[test]
    fn respects_custom_cap() {
        // Cap at 10 — every chunk must fit.
        let chunks = split_message("one two three four five six seven", 10);
        for chunk in &chunks {
            assert!(
                chunk.len() <= 10,
                "chunk {chunk:?} exceeds cap of 10"
            );
        }
        assert!(chunks.len() >= 3);
    }

    #[test]
    fn breaks_at_paragraph_first() {
        let mut text = String::new();
        text.push_str(&"a".repeat(80));
        text.push_str("\n\n");
        text.push_str(&"b".repeat(80));
        // Cap of 100 forces a split — paragraph break is the cleanest spot.
        let chunks = split_message(&text, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a".repeat(80));
        assert_eq!(chunks[1], "b".repeat(80));
    }

    #[test]
    fn breaks_at_word_boundary_when_no_paragraph() {
        let word = "hello ";
        let count = 4096 / word.len() + 100;
        let text: String = word.repeat(count);
        let chunks = split_message(&text, 4096);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 4096);
        }
    }

    #[test]
    fn hard_cut_when_no_whitespace_exists() {
        let text = "x".repeat(8192);
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4096);
        assert_eq!(chunks[1].len(), 4096);
    }

    #[test]
    fn xmpp_size_cap_works() {
        // The XMPP adapter will call this with 3000.
        let text = "a paragraph. ".repeat(500); // ~6500 chars
        let chunks = split_message(&text, 3000);
        for chunk in &chunks {
            assert!(chunk.len() <= 3000);
        }
        assert!(chunks.len() >= 2);
    }
}
