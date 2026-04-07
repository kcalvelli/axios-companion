//! Application state and update logic for companion-tui.

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Conversation,
}

/// Per-session conversation buffer.
#[derive(Debug, Clone)]
pub struct ConversationBuffer {
    pub surface: String,
    pub conversation_id: String,
    /// Accumulated text from chunks.
    pub text: String,
    /// Whether the current turn has completed.
    pub turn_complete: bool,
}

/// Daemon status snapshot from get_status().
#[derive(Debug, Clone, Default)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_seconds: u32,
    pub active_sessions: u32,
    pub in_flight_turns: u32,
}

/// A session row from list_sessions().
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub surface: String,
    pub conversation_id: String,
    pub claude_session_id: String,
    pub status: String,
    pub last_active_at: u32,
}

/// Top-level application state.
pub struct App {
    pub running: bool,
    pub connected: bool,
    pub focus: Focus,
    pub show_help: bool,

    // Sessions panel state.
    pub sessions: Vec<SessionRow>,
    pub selected_session: usize,

    // Status bar state.
    pub daemon_status: DaemonStatus,

    // Conversation panel state — keyed by (surface, conversation_id).
    pub conversations: Vec<ConversationBuffer>,
    /// Scroll offset from the bottom of the conversation view.
    pub conversation_scroll: u16,

    /// Errors or status messages shown briefly.
    pub flash_message: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            connected: false,
            focus: Focus::Sessions,
            show_help: false,
            sessions: Vec::new(),
            selected_session: 0,
            daemon_status: DaemonStatus::default(),
            conversations: Vec::new(),
            conversation_scroll: 0,
            flash_message: None,
        }
    }

    /// The currently selected session, if any.
    pub fn selected_session_key(&self) -> Option<(&str, &str)> {
        self.sessions
            .get(self.selected_session)
            .map(|s| (s.surface.as_str(), s.conversation_id.as_str()))
    }

    /// Get or create a conversation buffer for the given session.
    pub fn conversation_buf_mut(
        &mut self,
        surface: &str,
        conversation_id: &str,
    ) -> &mut ConversationBuffer {
        let pos = self
            .conversations
            .iter()
            .position(|c| c.surface == surface && c.conversation_id == conversation_id);

        match pos {
            Some(i) => &mut self.conversations[i],
            None => {
                self.conversations.push(ConversationBuffer {
                    surface: surface.to_string(),
                    conversation_id: conversation_id.to_string(),
                    text: String::new(),
                    turn_complete: false,
                });
                self.conversations.last_mut().unwrap()
            }
        }
    }

    /// Move session selection up.
    pub fn select_prev(&mut self) {
        if self.selected_session > 0 {
            self.selected_session -= 1;
            self.conversation_scroll = 0;
        }
    }

    /// Move session selection down.
    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() && self.selected_session < self.sessions.len() - 1 {
            self.selected_session += 1;
            self.conversation_scroll = 0;
        }
    }

    /// Scroll conversation up (towards older text).
    pub fn scroll_up(&mut self) {
        self.conversation_scroll = self.conversation_scroll.saturating_add(1);
    }

    /// Scroll conversation down (towards newer text).
    pub fn scroll_down(&mut self) {
        self.conversation_scroll = self.conversation_scroll.saturating_sub(1);
    }

    /// Jump to bottom of conversation.
    pub fn scroll_bottom(&mut self) {
        self.conversation_scroll = 0;
    }

    /// Toggle focus between panels.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sessions => Focus::Conversation,
            Focus::Conversation => Focus::Sessions,
        };
    }

    /// Update sessions list from daemon. Preserves selection if possible.
    pub fn update_sessions(&mut self, rows: Vec<SessionRow>) {
        let prev_key = self.selected_session_key().map(|(s, c)| (s.to_string(), c.to_string()));

        self.sessions = rows;

        // Try to re-select the previously selected session.
        if let Some((prev_surface, prev_conv)) = prev_key {
            if let Some(idx) = self
                .sessions
                .iter()
                .position(|s| s.surface == prev_surface && s.conversation_id == prev_conv)
            {
                self.selected_session = idx;
            } else {
                self.selected_session = self.selected_session.min(
                    self.sessions.len().saturating_sub(1),
                );
            }
        }
    }
}
