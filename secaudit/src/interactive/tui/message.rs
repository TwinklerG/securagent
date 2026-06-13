//! TUI 聊天消息与事件条目的渲染模型。

use std::time::SystemTime;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::markdown::render_markdown_lines;
use super::timestamp::{format_absolute_timestamp, now_timestamp};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageRole {
    User,
    Agent,
    System,
    Error,
}

impl MessageRole {
    fn label(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Agent => "AGENT",
            Self::System => "SYSTEM",
            Self::Error => "ERROR",
        }
    }

    fn label_style(self) -> Style {
        match self {
            Self::User => Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            Self::Agent => Style::default()
                .fg(Color::Black)
                .bg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
            Self::System => Style::default()
                .fg(Color::Black)
                .bg(Color::Gray)
                .add_modifier(Modifier::BOLD),
            Self::Error => Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EventKind {
    State,
    ToolCall,
    ToolResult,
    ContextCompaction,
    System,
    Error,
}

impl EventKind {
    fn badge(self) -> &'static str {
        match self {
            Self::State => "STATE",
            Self::ToolCall => "TOOL",
            Self::ToolResult => "RESULT",
            Self::ContextCompaction => "压缩",
            Self::System => "INFO",
            Self::Error => "ERROR",
        }
    }

    fn badge_style(self) -> Style {
        match self {
            Self::State => Style::default().fg(Color::Black).bg(Color::Magenta),
            Self::ToolCall => Style::default().fg(Color::Black).bg(Color::Blue),
            Self::ToolResult => Style::default().fg(Color::Black).bg(Color::Gray),
            Self::ContextCompaction => Style::default().fg(Color::Black).bg(Color::LightCyan),
            Self::System => Style::default().fg(Color::Black).bg(Color::Cyan),
            Self::Error => Style::default().fg(Color::White).bg(Color::Red),
        }
        .add_modifier(Modifier::BOLD)
    }

    fn text_style(self) -> Style {
        match self {
            Self::Error => Style::default().fg(Color::LightRed),
            Self::State => Style::default().fg(Color::LightMagenta),
            Self::ToolCall => Style::default().fg(Color::LightBlue),
            Self::ContextCompaction => Style::default().fg(Color::LightCyan),
            Self::System | Self::ToolResult => Style::default().fg(Color::White),
        }
    }
}

pub(super) struct ChatMessage {
    timestamp: SystemTime,
    role: MessageRole,
    pub(super) content: String,
}

impl ChatMessage {
    pub(super) fn new(role: MessageRole, content: &str) -> Self {
        Self {
            timestamp: now_timestamp(),
            role,
            content: content.to_owned(),
        }
    }

    pub(super) fn empty_agent() -> Self {
        Self::new(MessageRole::Agent, "")
    }

    fn header_line(&self) -> Line<'static> {
        let time = format_absolute_timestamp(self.timestamp);
        let role_label = format!(" {:^7} ", self.role.label());

        Line::from(vec![
            Span::styled(time, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(role_label, self.role.label_style()),
        ])
    }

    pub(super) fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![self.header_line()];
        let mut body = render_markdown_lines(self.content.as_str());
        if body.is_empty() {
            body.push(Line::from(String::new()));
        }
        lines.extend(body);
        lines.push(Line::from(String::new()));
        lines
    }
}

pub(super) struct EventEntry {
    timestamp: SystemTime,
    kind: EventKind,
    pub(super) text: String,
}

impl EventEntry {
    pub(super) fn new(kind: EventKind, text: String) -> Self {
        Self {
            timestamp: now_timestamp(),
            kind,
            text,
        }
    }

    #[cfg(test)]
    pub(super) fn kind_badge(&self) -> &'static str {
        self.kind.badge()
    }

    pub(super) fn render_line(&self) -> Line<'static> {
        let time = format_absolute_timestamp(self.timestamp);
        let badge = format!(" {:^6} ", self.kind.badge());

        Line::from(vec![
            Span::styled(time, Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(badge, self.kind.badge_style()),
            Span::raw(" "),
            Span::styled(self.text.clone(), self.kind.text_style()),
        ])
    }
}
