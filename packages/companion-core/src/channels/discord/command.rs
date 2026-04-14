//! Bang commands (`!new`, `!status`, `!help`) for the Discord channel.
//!
//! Same set as xmpp and email. Returns the reply text for the caller to
//! send back to the channel.

use crate::channels::util::format_timestamp;
use crate::dispatcher::Dispatcher;

/// Handle a bang command. Returns the reply text.
pub async fn handle(
    surface_id: &str,
    conversation_id: &str,
    text: &str,
    dispatcher: &Dispatcher,
) -> String {
    let cmd = text
        .trim_start_matches('!')
        .split_whitespace()
        .next()
        .unwrap_or("");

    match cmd {
        "new" => {
            let store = dispatcher.store().await;
            let had_session = store
                .delete_session(surface_id, conversation_id)
                .unwrap_or(false);
            drop(store);

            if had_session {
                "Fine. Whatever we just talked about? Gone. Hope it wasn't important.".into()
            } else {
                "There's nothing to forget. We haven't even started yet.".into()
            }
        }
        "status" => {
            let store = dispatcher.store().await;
            let session = store
                .lookup_session(surface_id, conversation_id)
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
                        "Session active.\nClaude session: {}\nLast active: {}",
                        claude_id,
                        format_timestamp(s.last_active_at),
                    )
                }
                None => "No active session. Send a message to start one.".into(),
            }
        }
        "help" => "\
!new — drop the session, start fresh on the next message
!status — show the session info
!help — this message

Anything else goes straight to the companion."
            .into(),
        _ => "Not a command. Try !help if you're lost.".into(),
    }
}
