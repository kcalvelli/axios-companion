//! Rendering logic for companion-tui.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Render the entire UI.
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // main area
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    draw_main(f, app, chunks[0]);
    draw_status_bar(f, app, chunks[1]);

    if app.show_help {
        draw_help_overlay(f, f.area());
    }
}

/// Main area: sessions panel (left) + conversation panel (right).
fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    if !app.connected {
        draw_disconnected(f, area);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // sessions
            Constraint::Percentage(70), // conversation
        ])
        .split(area);

    draw_sessions_panel(f, app, cols[0]);
    draw_conversation_panel(f, app, cols[1]);
}

/// Disconnected state — full area message.
fn draw_disconnected(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" companion-tui ")
        .border_style(Style::default().fg(Color::DarkGray));

    let text = Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  daemon not running",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  waiting for org.axios.Companion on D-Bus...",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  start with: systemctl --user start companion-core",
            Style::default().fg(Color::DarkGray),
        )),
    ]);

    let para = Paragraph::new(text).block(block);
    f.render_widget(para, area);
}

/// Sessions panel — left side table.
fn draw_sessions_panel(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Sessions;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" sessions ")
        .border_style(border_style);

    if app.sessions.is_empty() {
        let text = Paragraph::new(Span::styled(
            "  no active sessions",
            Style::default().fg(Color::DarkGray),
        ))
        .block(block);
        f.render_widget(text, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("SURFACE"),
        Cell::from("CONVERSATION"),
        Cell::from("STATUS"),
        Cell::from("LAST ACTIVE"),
    ])
    .style(Style::default().fg(Color::DarkGray));

    let rows: Vec<Row> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == app.selected_session {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(s.surface.clone()),
                Cell::from(truncate(&s.conversation_id, 12)),
                Cell::from(s.status.clone()),
                Cell::from(relative_time(s.last_active_at)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(block);

    f.render_widget(table, area);
}

/// Conversation panel — right side streaming view.
fn draw_conversation_panel(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Conversation;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let (surface, conv_id) = match app.selected_session_key() {
        Some(key) => key,
        None => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" conversation ")
                .border_style(border_style);
            let text = Paragraph::new(Span::styled(
                "  select a session",
                Style::default().fg(Color::DarkGray),
            ))
            .block(block);
            f.render_widget(text, area);
            return;
        }
    };

    let buf = app
        .conversations
        .iter()
        .find(|c| c.surface == surface && c.conversation_id == conv_id);

    let content = match buf {
        Some(b) if !b.text.is_empty() => b.text.as_str(),
        _ => "(no output yet)",
    };

    let title = format!(" {} | {} ", surface, truncate(conv_id, 12));
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style);

    let para = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.conversation_scroll, 0));

    f.render_widget(para, area);
}

/// Bottom status bar.
fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let status = &app.daemon_status;

    let left = if app.connected {
        format!(
            " companion v{} | up {} | {} sessions | {} in-flight",
            status.version,
            format_uptime(status.uptime_seconds),
            status.active_sessions,
            status.in_flight_turns,
        )
    } else {
        " disconnected".to_string()
    };

    let right = if let Some(ref msg) = app.flash_message {
        format!("{} ", msg)
    } else {
        " ?=help q=quit ".to_string()
    };

    let left_len = left.len() as u16;
    let right_len = right.len() as u16;
    let gap = area.width.saturating_sub(left_len + right_len);

    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::Cyan)),
        Span::raw(" ".repeat(gap as usize)),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);

    let bar = Paragraph::new(line)
        .style(Style::default().bg(Color::Black));
    f.render_widget(bar, area);
}

/// Help overlay.
fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let popup_area = centered_rect(50, 60, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" keybindings ")
        .border_style(Style::default().fg(Color::Yellow));

    let help_text = vec![
        Line::from(""),
        Line::from("  j/k       navigate sessions"),
        Line::from("  Tab       switch panel focus"),
        Line::from("  1         focus sessions"),
        Line::from("  2         focus conversation"),
        Line::from(""),
        Line::from("  In conversation panel:"),
        Line::from("  j/k       scroll up/down"),
        Line::from("  g         scroll to top"),
        Line::from("  G         scroll to bottom"),
        Line::from(""),
        Line::from("  ?         toggle this help"),
        Line::from("  q         quit"),
    ];

    let para = Paragraph::new(help_text).block(block);
    f.render_widget(para, popup_area);
}

/// Center a rect of given percentage within the parent.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn format_uptime(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

fn relative_time(unix_ts: u32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    let delta = now.saturating_sub(unix_ts);

    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}
