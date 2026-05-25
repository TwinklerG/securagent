#![expect(clippy::print_stderr, reason = "TUI 初始化与确认流程需要终端直接输出")]
#![expect(
    clippy::indexing_slicing,
    reason = "输入缓冲区已通过不变式保障索引安全"
)]
#![expect(clippy::integer_division, reason = "布局百分比分割需使用整数运算")]
#![expect(
    clippy::too_many_lines,
    reason = "TUI 主渲染与事件循环集中管理便于维护"
)]

mod event_format;
mod markdown;
mod timestamp;

use std::io::{self, Stdout, Write};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use secaudit_agent::Agent;
use secaudit_agent::state::AgentState;
use secaudit_conversation::{SessionListItem, SessionPreviewRole};
use secaudit_core::Config;
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

use super::commands::{Command, UserInput};
use super::{
    ChatRequest, DisplayMessage, DisplayRole, SessionSnapshot, WorkerCommand, WorkerEvent,
    build_worker_conversation, parse_user_input, start_worker_session,
};
use event_format::{summarize_tool_args, summarize_tool_result};
use markdown::render_markdown_lines;
use timestamp::{format_absolute_timestamp, now_timestamp};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_EVENT_LINES: usize = 500;
const MAX_MESSAGE_ITEMS: usize = 240;
const KEY_CTRL_C: char = 'c';
const KEY_CTRL_D: char = 'd';
const DEFAULT_VIEWPORT_HEIGHT: u16 = 10;
const MAX_INPUT_LINES: usize = 6;

const WELCOME_MSG: &str = "secaudit -- 安全代码审计 Agent（TUI）";
const HELP_HINT: &str = "输入审计指令后回车，命令：/help /new /sessions /session <id|序号> /status /tools /exit，Ctrl+D 退出";

const FOOTER_HINT: &str =
    "F1 帮助  Ctrl+L 事件面板  Ctrl+J/K 事件滚动  Tab 补全  Ctrl+P/N 历史  Enter 发送  Ctrl+D 退出";

const HELP_TEXT: &[&str] = &[
    "快捷键",
    "  Enter                发送输入（单行）",
    "  Shift+Enter          新增输入行（多行）",
    "  Ctrl+C / Ctrl+D / /exit 退出",
    "  F1                   打开/关闭帮助",
    "  Ctrl+L               折叠/展开事件面板",
    "  Ctrl+J / Ctrl+K      滚动事件面板",
    "  Up / Down            浏览对话（输入为空时）",
    "  PageUp / PageDown    按页滚动对话",
    "  Home / End           对话顶部/底部",
    "  Ctrl+P / Ctrl+N      输入历史 上一条/下一条",
    "  Tab                  命令补全（以 / 开头）",
    "",
    "命令",
    "  /help          显示帮助",
    "  /new           新建会话并清空当前视图",
    "  /clear         /new 的兼容别名",
    "  /sessions      列出当前项目会话",
    "  /session <id|序号>  切换到指定会话",
    "  /status        显示当前状态",
    "  /tools         列出工具",
    "  /exit          退出",
];

const HELP_EVENT_SUMMARY: &str = "已显示帮助。";
const STATUS_EVENT_SUMMARY: &str = "已显示当前状态。";
const TOOLS_EVENT_SUMMARY: &str = "已显示可用工具。";
const CLEAR_EVENT_SUMMARY: &str = "已开启新会话，当前视图与事件已清空。";
const SESSIONS_EVENT_SUMMARY: &str = "已显示会话列表。";
const SWITCH_SESSION_EVENT_SUMMARY: &str = "已切换会话。";
const SESSION_PREVIEW_MAX_CHARS: usize = 96;

const COMMAND_CANDIDATES: [&str; 8] = [
    "/help",
    "/new",
    "/clear",
    "/sessions",
    "/session ",
    "/status",
    "/tools",
    "/exit",
];

const EVENT_PANEL_WIDTH_EXPANDED: u16 = 34;
const EVENT_PANEL_WIDTH_COLLAPSED: u16 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageRole {
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
enum EventKind {
    State,
    ToolCall,
    ToolResult,
    System,
    Error,
}

impl EventKind {
    fn badge(self) -> &'static str {
        match self {
            Self::State => "STATE",
            Self::ToolCall => "TOOL",
            Self::ToolResult => "RESULT",
            Self::System => "INFO",
            Self::Error => "ERROR",
        }
    }

    fn badge_style(self) -> Style {
        match self {
            Self::State => Style::default().fg(Color::Black).bg(Color::Magenta),
            Self::ToolCall => Style::default().fg(Color::Black).bg(Color::Blue),
            Self::ToolResult => Style::default().fg(Color::Black).bg(Color::Gray),
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
            Self::System | Self::ToolResult => Style::default().fg(Color::White),
        }
    }
}

struct ChatMessage {
    timestamp: SystemTime,
    role: MessageRole,
    content: String,
}

impl ChatMessage {
    fn header_line(&self) -> Line<'static> {
        let time = format_absolute_timestamp(self.timestamp);
        let role_label = format!(" {:^7} ", self.role.label());

        Line::from(vec![
            Span::styled(time, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(role_label, self.role.label_style()),
        ])
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
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

struct EventEntry {
    timestamp: SystemTime,
    kind: EventKind,
    text: String,
}

impl EventEntry {
    fn render_line(&self) -> Line<'static> {
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

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Default)]
struct InputBuffer {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

impl InputBuffer {
    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        }
    }

    fn ensure_invariants(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len().saturating_sub(1);
        }

        let max_col = self.current_line_len();
        if self.cursor_col > max_col {
            self.cursor_col = max_col;
        }
    }

    fn current_line_len(&self) -> usize {
        self.lines
            .get(self.cursor_line)
            .map_or(0, |line| line.chars().count())
    }

    fn current_line_mut(&mut self) -> &mut String {
        self.ensure_invariants();
        &mut self.lines[self.cursor_line]
    }

    fn is_empty(&self) -> bool {
        self.lines.first().is_some_and(String::is_empty) && self.lines.len() == 1
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.lines.push(String::new());
        self.cursor_line = 0;
        self.cursor_col = 0;
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn set_text(&mut self, text: &str) {
        let mut split: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
        if split.is_empty() {
            split.push(String::new());
        }
        self.lines = split;
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
    }

    fn insert_char(&mut self, ch: char) {
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let byte = char_to_byte_idx(line, col);
        line.insert(byte, ch);
        self.cursor_col += 1;
    }

    fn backspace(&mut self) {
        self.ensure_invariants();

        if self.cursor_col > 0 {
            let cursor = self.cursor_col;
            let line = self.current_line_mut();
            let start = char_to_byte_idx(line, cursor - 1);
            let end = char_to_byte_idx(line, cursor);
            line.replace_range(start..end, "");
            self.cursor_col -= 1;
            return;
        }

        if self.cursor_line > 0 {
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.current_line_len();
            if let Some(prev) = self.lines.get_mut(self.cursor_line) {
                prev.push_str(&current);
            }
            self.cursor_col = prev_len;
        }
    }

    fn newline(&mut self) {
        self.ensure_invariants();
        if self.lines.len() >= MAX_INPUT_LINES {
            return;
        }

        let cursor = self.cursor_col;
        let line = self.current_line_mut();
        let split = char_to_byte_idx(line, cursor);
        let tail = line.split_off(split);
        let insert_at = self.cursor_line + 1;
        self.lines.insert(insert_at, tail);
        self.cursor_line = insert_at;
        self.cursor_col = 0;
    }

    fn move_left(&mut self) {
        self.ensure_invariants();

        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            return;
        }
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    fn move_right(&mut self) {
        self.ensure_invariants();

        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
            return;
        }
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        self.ensure_invariants();

        if self.cursor_line == 0 {
            return;
        }
        self.cursor_line -= 1;
        self.cursor_col = self.cursor_col.min(self.current_line_len());
    }

    fn move_down(&mut self) {
        self.ensure_invariants();

        if self.cursor_line + 1 >= self.lines.len() {
            return;
        }
        self.cursor_line += 1;
        self.cursor_col = self.cursor_col.min(self.current_line_len());
    }

    fn move_line_start(&mut self) {
        self.cursor_col = 0;
    }

    fn move_line_end(&mut self) {
        self.ensure_invariants();
        self.cursor_col = self.current_line_len();
    }

    fn cursor_display_col(&self) -> usize {
        let Some(line) = self.lines.get(self.cursor_line) else {
            return 0;
        };

        let byte_idx = char_to_byte_idx(line, self.cursor_col);
        line.get(..byte_idx).map_or(0, UnicodeWidthStr::width)
    }

    fn visual_lines(&self) -> Vec<Line<'_>> {
        if self.lines.iter().all(String::is_empty) {
            return vec![Line::from(Span::styled(
                "输入审计指令...",
                Style::default().fg(Color::DarkGray),
            ))];
        }

        self.lines
            .iter()
            .map(|line| Line::from(line.as_str()))
            .collect::<Vec<_>>()
    }
}

fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    if char_idx == s.chars().count() {
        return s.len();
    }

    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(idx, _)| idx)
}

#[derive(Default)]
struct History {
    entries: Vec<String>,
    browse_index: Option<usize>,
}

impl History {
    fn push(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        if self.entries.last().is_some_and(|last| last == &entry) {
            self.browse_index = None;
            return;
        }
        self.entries.push(entry);
        self.browse_index = None;
    }

    fn prev(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }

        let next = match self.browse_index {
            None => self.entries.len().saturating_sub(1),
            Some(0) => 0,
            Some(idx) => idx.saturating_sub(1),
        };
        self.browse_index = Some(next);
        self.entries.get(next).cloned()
    }

    fn next(&mut self) -> Option<String> {
        match self.browse_index {
            None => None,
            Some(idx) if idx + 1 >= self.entries.len() => {
                self.browse_index = None;
                Some(String::new())
            }
            Some(idx) => {
                let next = idx + 1;
                self.browse_index = Some(next);
                self.entries.get(next).cloned()
            }
        }
    }

    fn reset_browse(&mut self) {
        self.browse_index = None;
    }
}

struct TuiApp {
    work_dir: PathBuf,
    input: InputBuffer,
    history: History,
    messages: Vec<ChatMessage>,
    events: Vec<EventEntry>,
    should_quit: bool,
    busy: bool,
    show_help: bool,
    tool_names: Vec<String>,
    current_session_id: Option<String>,
    message_count: usize,
    last_state_label: String,
    chat_scroll: u16,
    follow_chat: bool,
    chat_viewport_height: u16,
    chat_viewport_width: u16,
    event_scroll: u16,
    event_viewport_height: u16,
    event_viewport_width: u16,
    event_panel_collapsed: bool,
    /// 当前流式输出的 agent 消息在 `messages` 中的索引。
    /// 为 None 表示尚未开始流式输出；ChatDone 后归零。
    streaming_index: Option<usize>,
}

impl TuiApp {
    fn new(work_dir: PathBuf) -> Self {
        let mut app = Self {
            work_dir,
            input: InputBuffer::new(),
            history: History::default(),
            messages: Vec::new(),
            events: Vec::new(),
            should_quit: false,
            busy: false,
            show_help: false,
            tool_names: Vec::new(),
            current_session_id: None,
            message_count: 0,
            last_state_label: "就绪".to_owned(),
            chat_scroll: 0,
            follow_chat: true,
            chat_viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            chat_viewport_width: DEFAULT_VIEWPORT_HEIGHT,
            event_scroll: 0,
            event_viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            event_viewport_width: DEFAULT_VIEWPORT_HEIGHT,
            event_panel_collapsed: false,
            streaming_index: None,
        };

        app.push_system(
            &[
                WELCOME_MSG,
                &format!("工作目录：{}", app.work_dir.display()),
                HELP_HINT,
            ]
            .join("\n"),
        );
        app.push_event(
            EventKind::System,
            "事件面板默认展开，可按 Ctrl+L 折叠。".to_owned(),
        );
        app
    }

    fn push_message(&mut self, role: MessageRole, text: &str) {
        self.messages.push(ChatMessage {
            timestamp: now_timestamp(),
            role,
            content: text.to_owned(),
        });

        if self.messages.len() > MAX_MESSAGE_ITEMS {
            let overflow = self.messages.len().saturating_sub(MAX_MESSAGE_ITEMS);
            self.messages.drain(0..overflow);
        }

        if self.follow_chat {
            self.scroll_chat_to_bottom();
        }
    }

    fn push_system(&mut self, text: &str) {
        self.push_message(MessageRole::System, text);
        self.push_event(EventKind::System, text.to_owned());
    }

    fn push_system_block(&mut self, text: &str, event_summary: &str) {
        self.push_message(MessageRole::System, text);
        self.push_event(EventKind::System, event_summary.to_owned());
    }

    fn push_state(&mut self, state: &AgentState) {
        state.label().clone_into(&mut self.last_state_label);
        self.push_event(EventKind::State, state.label().to_owned());
    }

    fn push_user(&mut self, text: &str) {
        self.follow_chat = true;
        self.push_message(MessageRole::User, text);
    }

    fn push_agent(&mut self, text: &str) {
        self.push_message(MessageRole::Agent, text);
    }

    /// 把流式 delta 追加到正在输出的 agent 消息。第一次调用会创建一条新消息。
    fn append_streaming_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }

        if self.streaming_index.is_none() {
            self.messages.push(ChatMessage {
                timestamp: now_timestamp(),
                role: MessageRole::Agent,
                content: String::new(),
            });
            if self.messages.len() > MAX_MESSAGE_ITEMS {
                let overflow = self.messages.len().saturating_sub(MAX_MESSAGE_ITEMS);
                self.messages.drain(0..overflow);
            }
            self.streaming_index = Some(self.messages.len().saturating_sub(1));
        }

        if let Some(idx) = self.streaming_index
            && let Some(msg) = self.messages.get_mut(idx)
        {
            msg.content.push_str(delta);
        }

        if self.follow_chat {
            self.scroll_chat_to_bottom();
        }
    }

    /// 流式输出结束：信任已累积的 delta 内容；若一个 delta 都没收到（如错误提前返回），
    /// 退回到 `final_text`；若两者都为空，移除占位消息。
    fn finalize_streaming(&mut self, final_text: &str) {
        match self.streaming_index.take() {
            Some(idx) => {
                let remove = self.messages.get_mut(idx).is_some_and(|msg| {
                    if final_text.is_empty() {
                        msg.content.is_empty()
                    } else {
                        final_text.clone_into(&mut msg.content);
                        false
                    }
                });
                if remove {
                    self.messages.remove(idx);
                }
                if self.follow_chat {
                    self.scroll_chat_to_bottom();
                }
            }
            None => {
                self.push_agent(final_text);
            }
        }
    }

    fn push_error(&mut self, text: &str) {
        self.push_message(MessageRole::Error, text);
        self.push_event(EventKind::Error, text.to_owned());
    }

    fn push_think(&mut self, text: &str) {
        let one_line = first_line(text).to_owned();
        self.push_event(EventKind::System, format!("思考：{one_line}"));
    }

    fn push_tool_call(&mut self, name: &str, args: &str) {
        let args_summary = summarize_tool_args(args);
        self.push_event(EventKind::ToolCall, format!("{name} ({args_summary})"));
    }

    fn push_tool_result(&mut self, name: &str, result: &str) {
        let summary = summarize_tool_result(result);
        self.push_event(EventKind::ToolResult, format!("{name} -> {summary}"));
    }

    fn push_event(&mut self, kind: EventKind, text: String) {
        self.events.push(EventEntry {
            timestamp: now_timestamp(),
            kind,
            text,
        });

        if self.events.len() > MAX_EVENT_LINES {
            let overflow = self.events.len().saturating_sub(MAX_EVENT_LINES);
            self.events.drain(0..overflow);
        }

        self.scroll_event_to_bottom();
    }

    fn clear_messages_and_events(&mut self) {
        self.messages.clear();
        self.events.clear();
        self.chat_scroll = 0;
        self.event_scroll = 0;
        self.follow_chat = true;
        self.streaming_index = None;
        self.message_count = 0;
        self.push_system_block(
            &format!("{CLEAR_EVENT_SUMMARY}\n{HELP_HINT}"),
            CLEAR_EVENT_SUMMARY,
        );
    }

    fn load_session_snapshot(&mut self, snapshot: SessionSnapshot, summary: &str) {
        self.messages.clear();
        self.events.clear();
        self.chat_scroll = 0;
        self.event_scroll = 0;
        self.follow_chat = true;
        self.current_session_id = Some(snapshot.id);
        self.message_count = snapshot.message_count;

        for message in &snapshot.messages {
            self.push_display_message(message);
        }

        self.push_system_block(summary, SWITCH_SESSION_EVENT_SUMMARY);
    }

    fn push_display_message(&mut self, message: &DisplayMessage) {
        match message.role {
            DisplayRole::User => self.push_user(&message.content),
            DisplayRole::Agent => self.push_agent(&message.content),
        }
    }

    fn show_help_text(&mut self) {
        self.push_system_block(&HELP_TEXT.join("\n"), HELP_EVENT_SUMMARY);
    }

    fn show_status(&mut self) {
        let activity = if self.busy {
            "Agent 运行中"
        } else {
            "Agent 空闲"
        };
        let status = format!(
            "工作目录：{}\n当前会话：{}\n当前状态：{}\n对话消息数：{}\n{}",
            self.work_dir.display(),
            self.current_session_id.as_deref().unwrap_or("尚未就绪"),
            self.last_state_label,
            self.message_count,
            activity
        );
        self.push_system_block(&status, STATUS_EVENT_SUMMARY);
    }

    fn show_sessions(&mut self, sessions: &[SessionListItem]) {
        if sessions.is_empty() {
            self.push_system_block("当前项目没有历史会话。", SESSIONS_EVENT_SUMMARY);
            return;
        }

        let mut lines = vec!["当前项目会话：".to_owned()];
        for (index, item) in sessions.iter().enumerate() {
            let session = &item.metadata;
            let current = if self
                .current_session_id
                .as_deref()
                .is_some_and(|id| id == session.session_id)
            {
                "*"
            } else {
                " "
            };
            let display_index = index + 1;
            lines.push(format!(
                "{current} [{display_index}] {}  {}  messages={}  updated={}",
                short_session_id(Some(&session.session_id)),
                session.status,
                session.message_count,
                session.updated_at
            ));
            lines.push(format!("    预览：{}", session_preview_text(item)));
        }
        lines.push("使用 /session <序号> 或 /session <id> 切换会话，/new 新建会话。".to_owned());

        self.push_system_block(&lines.join("\n"), SESSIONS_EVENT_SUMMARY);
    }

    fn show_tools(&mut self) {
        if self.tool_names.is_empty() {
            self.push_system("工具列表尚未就绪。");
            return;
        }

        let tools = self
            .tool_names
            .iter()
            .map(|name| format!("  - {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.push_system_block(&format!("可用工具：\n{tools}"), TOOLS_EVENT_SUMMARY);
    }

    fn toggle_event_panel(&mut self) {
        self.event_panel_collapsed = !self.event_panel_collapsed;
        if self.event_panel_collapsed {
            self.push_event(
                EventKind::System,
                "事件面板已折叠（Ctrl+L 展开）".to_owned(),
            );
        } else {
            self.push_event(
                EventKind::System,
                "事件面板已展开（Ctrl+L 折叠）".to_owned(),
            );
        }
    }

    fn rendered_chat_lines(&self) -> Vec<Line<'static>> {
        if self.messages.is_empty() {
            return vec![Line::from(Span::styled(
                "暂无消息",
                Style::default().fg(Color::DarkGray),
            ))];
        }

        self.messages
            .iter()
            .flat_map(ChatMessage::render_lines)
            .collect::<Vec<_>>()
    }

    fn rendered_event_lines(&self) -> Vec<Line<'static>> {
        if self.events.is_empty() {
            return vec![Line::from(Span::styled(
                "暂无事件",
                Style::default().fg(Color::DarkGray),
            ))];
        }

        self.events
            .iter()
            .map(EventEntry::render_line)
            .collect::<Vec<_>>()
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(4),
                Constraint::Length(4),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let [header_area, center_area, input_area, footer_area] = root.as_ref() else {
            return;
        };

        let header_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(*header_area);

        let [workspace_area, runtime_area] = header_chunks.as_ref() else {
            return;
        };

        let center_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(32),
                Constraint::Length(if self.event_panel_collapsed {
                    EVENT_PANEL_WIDTH_COLLAPSED
                } else {
                    EVENT_PANEL_WIDTH_EXPANDED
                }),
            ])
            .split(*center_area);

        let [chat_area, event_area] = center_chunks.as_ref() else {
            return;
        };

        let header_title = Paragraph::new(vec![
            Line::from(Span::styled(
                "SeCAudit Chat",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("Directory  ", Style::default().fg(Color::Gray)),
                Span::raw(self.work_dir.display().to_string()),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title("Workspace"));

        let activity = if self.busy { "Running" } else { "Idle" };
        let metrics = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Session ", Style::default().fg(Color::Gray)),
                Span::raw(short_session_id(self.current_session_id.as_deref())),
            ]),
            Line::from(vec![
                Span::styled("Agent   ", Style::default().fg(Color::Gray)),
                Span::raw(activity),
            ]),
            Line::from(vec![
                Span::styled("State   ", Style::default().fg(Color::Gray)),
                Span::raw(self.last_state_label.clone()),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title("Runtime"));

        self.chat_viewport_height = chat_area.height.saturating_sub(2);
        self.chat_viewport_width = chat_area.width.saturating_sub(2).max(1);
        let chat_lines = wrap_lines(
            self.rendered_chat_lines(),
            usize::from(self.chat_viewport_width),
        );

        let chat_max_offset = max_scroll_offset(chat_lines.len(), self.chat_viewport_height);
        if self.follow_chat || self.chat_scroll > chat_max_offset {
            self.chat_scroll = chat_max_offset;
        }

        let chat_title = if self.follow_chat {
            "Conversation · Follow"
        } else {
            "Conversation · Browse"
        };

        let chat_panel = Paragraph::new(chat_lines)
            .block(Block::default().borders(Borders::ALL).title(chat_title))
            .scroll((self.chat_scroll, 0));

        frame.render_widget(header_title, *workspace_area);
        frame.render_widget(metrics, *runtime_area);
        frame.render_widget(chat_panel, *chat_area);

        if self.event_panel_collapsed {
            let collapsed = Paragraph::new("E")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::LEFT));
            frame.render_widget(collapsed, *event_area);
        } else {
            self.event_viewport_height = event_area.height.saturating_sub(2);
            self.event_viewport_width = event_area.width.saturating_sub(2).max(1);
            let event_lines = wrap_lines(
                self.rendered_event_lines(),
                usize::from(self.event_viewport_width),
            );
            self.event_scroll = self.event_scroll.min(max_scroll_offset(
                event_lines.len(),
                self.event_viewport_height,
            ));

            let event_panel = Paragraph::new(event_lines)
                .block(Block::default().borders(Borders::ALL).title("Events"))
                .scroll((self.event_scroll, 0));
            frame.render_widget(event_panel, *event_area);
        }

        let input_title = if self.busy {
            "Input · Agent running"
        } else {
            "Input"
        };

        let input_panel = Paragraph::new(self.input.visual_lines())
            .block(Block::default().borders(Borders::ALL).title(input_title))
            .wrap(Wrap { trim: false });

        let footer = Paragraph::new(FOOTER_HINT).style(Style::default().fg(Color::DarkGray));

        frame.render_widget(input_panel, *input_area);
        frame.render_widget(footer, *footer_area);

        if self.show_help {
            draw_help_overlay(frame);
        } else {
            let col_u16 = u16::try_from(self.input.cursor_display_col()).unwrap_or(u16::MAX);
            let line_u16 = u16::try_from(self.input.cursor_line).unwrap_or(u16::MAX);
            let cursor_x = input_area.x.saturating_add(1).saturating_add(col_u16);
            let cursor_y = input_area.y.saturating_add(1).saturating_add(line_u16);
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    fn max_chat_scroll_offset(&self) -> u16 {
        let wrapped = wrap_lines(
            self.rendered_chat_lines(),
            usize::from(self.chat_viewport_width.max(1)),
        );
        max_scroll_offset(wrapped.len(), self.chat_viewport_height)
    }

    fn max_event_scroll_offset(&self) -> u16 {
        let wrapped = wrap_lines(
            self.rendered_event_lines(),
            usize::from(self.event_viewport_width.max(1)),
        );
        max_scroll_offset(wrapped.len(), self.event_viewport_height)
    }

    fn scroll_chat_up(&mut self, amount: u16) {
        self.follow_chat = false;
        self.chat_scroll = self.chat_scroll.saturating_sub(amount);
    }

    fn scroll_chat_down(&mut self, amount: u16) {
        let max_offset = self.max_chat_scroll_offset();
        let next = self.chat_scroll.saturating_add(amount);
        self.chat_scroll = next.min(max_offset);
        if self.chat_scroll >= max_offset {
            self.follow_chat = true;
        }
    }

    fn scroll_chat_to_top(&mut self) {
        self.follow_chat = false;
        self.chat_scroll = 0;
    }

    fn scroll_chat_to_bottom(&mut self) {
        self.chat_scroll = self.max_chat_scroll_offset();
        self.follow_chat = true;
    }

    fn scroll_event_up(&mut self, amount: u16) {
        self.event_scroll = self.event_scroll.saturating_sub(amount);
    }

    fn scroll_event_down(&mut self, amount: u16) {
        let max_offset = self.max_event_scroll_offset();
        let next = self.event_scroll.saturating_add(amount);
        self.event_scroll = next.min(max_offset);
    }

    fn scroll_event_to_bottom(&mut self) {
        self.event_scroll = self.max_event_scroll_offset();
    }

    fn page_scroll_amount(&self) -> u16 {
        self.chat_viewport_height.saturating_sub(1).max(1)
    }

    fn apply_tab_completion(&mut self) {
        let text = self.input.text();
        if !text.starts_with('/') || text.contains('\n') {
            return;
        }

        let current = text.trim();
        let mut candidates = COMMAND_CANDIDATES
            .iter()
            .copied()
            .filter(|cmd| cmd.starts_with(current))
            .collect::<Vec<_>>();

        if candidates.is_empty() {
            return;
        }

        candidates.sort_unstable();
        if let Some(first) = candidates.first().copied() {
            self.input.set_text(first);
        }
    }

    fn history_prev(&mut self) {
        if let Some(prev) = self.history.prev() {
            self.input.set_text(&prev);
        }
    }

    fn history_next(&mut self) {
        if let Some(next) = self.history.next() {
            self.input.set_text(&next);
        }
    }

    fn on_input_mutation(&mut self) {
        self.history.reset_browse();
    }
}

fn draw_help_overlay(frame: &mut Frame<'_>) {
    let area = centered_rect(70, 70, frame.area());
    let help_lines: Vec<Line<'_>> = HELP_TEXT.iter().map(|line| Line::raw(*line)).collect();

    let panel = Paragraph::new(help_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Help")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, outer: Rect) -> Rect {
    let top = (100u16.saturating_sub(percent_y)) / 2;
    let left = (100u16.saturating_sub(percent_x)) / 2;

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(top),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(top),
        ])
        .split(outer);

    let middle = match vertical.as_ref() {
        [_, mid, _] => *mid,
        _ => return outer,
    };

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(left),
        ])
        .split(middle);

    match horizontal.as_ref() {
        [_, mid, _] => *mid,
        _ => outer,
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn short_session_id(session_id: Option<&str>) -> String {
    session_id.map_or_else(
        || "pending".to_owned(),
        |id| {
            id.char_indices()
                .nth(8)
                .map_or_else(|| id.to_owned(), |(idx, _)| id[..idx].to_owned())
        },
    )
}

fn session_preview_text(item: &SessionListItem) -> String {
    let Some(preview) = &item.preview else {
        return "无用户/助手消息".to_owned();
    };
    let role = match preview.role {
        SessionPreviewRole::User => "User",
        SessionPreviewRole::Assistant => "Assistant",
    };
    format!(
        "{role}: {}",
        truncate_chars(&preview.content, SESSION_PREVIEW_MAX_CHARS)
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    const ELLIPSIS: &str = "...";
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}{ELLIPSIS}")
}

fn max_scroll_offset(total_lines: usize, viewport_height: u16) -> u16 {
    let viewport = usize::from(viewport_height.max(1));
    u16::try_from(total_lines.max(1).saturating_sub(viewport)).unwrap_or(u16::MAX)
}

fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut wrapped = Vec::new();

    for line in lines {
        let mut current = Vec::new();
        let mut current_width = 0usize;

        for span in line.spans {
            let style = span.style;
            for ch in span.content.chars() {
                let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if current_width > 0 && current_width.saturating_add(ch_width) > width {
                    wrapped.push(Line::from(mem::take(&mut current)));
                    current_width = 0;
                }

                current.push(Span::styled(ch.to_string(), style));
                current_width = current_width.saturating_add(ch_width);

                if current_width >= width {
                    wrapped.push(Line::from(mem::take(&mut current)));
                    current_width = 0;
                }
            }
        }

        if current.is_empty() {
            wrapped.push(Line::from(String::new()));
        } else {
            wrapped.push(Line::from(current));
        }
    }

    if wrapped.is_empty() {
        vec![Line::from(String::new())]
    } else {
        wrapped
    }
}

fn cli_confirm(
    event_tx: mpsc::UnboundedSender<WorkerEvent>,
) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |prompt: &str| {
        let _ = event_tx.send(WorkerEvent::Think(format!("确认请求：{prompt}")));

        let _ = disable_raw_mode();
        eprint!("\n确认 {prompt} [y/N] ");
        let _ = io::stderr().flush();

        let mut input = String::new();
        let approved = if io::stdin().read_line(&mut input).is_err() {
            false
        } else {
            matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
        };

        let _ = enable_raw_mode();
        approved
    })
}

fn bind_agent_callbacks(agent: &mut Agent, event_tx: &mpsc::UnboundedSender<WorkerEvent>) {
    {
        let tx = event_tx.clone();
        agent.on_state_change(move |state| {
            let _ = tx.send(WorkerEvent::State(state.clone()));
        });
    }
    {
        let tx = event_tx.clone();
        agent.on_think(move |text| {
            let _ = tx.send(WorkerEvent::Think(text.to_owned()));
        });
    }
    {
        let tx = event_tx.clone();
        agent.on_token(move |delta| {
            let _ = tx.send(WorkerEvent::Delta(delta.to_owned()));
        });
    }
    {
        let tx = event_tx.clone();
        agent.on_tool_call(move |name, args| {
            let _ = tx.send(WorkerEvent::ToolCall {
                name: name.to_owned(),
                args: args.to_owned(),
            });
        });
    }
    {
        let tx = event_tx.clone();
        agent.on_tool_result(move |name, result| {
            let _ = tx.send(WorkerEvent::ToolResult {
                name: name.to_owned(),
                result: result.to_owned(),
            });
        });
    }
}

async fn run_worker(
    config: Config,
    work_dir: PathBuf,
    mut command_rx: mpsc::UnboundedReceiver<WorkerCommand>,
    event_tx: mpsc::UnboundedSender<WorkerEvent>,
) {
    let mut agent = Agent::new(config, work_dir.clone(), cli_confirm(event_tx.clone()));
    bind_agent_callbacks(&mut agent, &event_tx);

    let tool_names = agent.tool_names();

    let conversation = match build_worker_conversation() {
        Ok(service) => service,
        Err(error) => {
            let _ = event_tx.send(WorkerEvent::Error(format!("会话服务初始化失败：{error}")));
            return;
        }
    };
    let mut session = match start_worker_session(&conversation, work_dir.as_path()) {
        Ok(session) => session,
        Err(error) => {
            let _ = event_tx.send(WorkerEvent::Error(format!("会话创建失败：{error}")));
            return;
        }
    };

    let _ = event_tx.send(WorkerEvent::Ready {
        tool_names,
        session: SessionSnapshot::from_managed(&session),
    });

    while let Some(command) = command_rx.recv().await {
        match command {
            WorkerCommand::Chat(request) => {
                let response = conversation
                    .chat(&mut agent, &mut session, &request.input)
                    .await
                    .map_err(|e| format!("{e}"));

                let _ = event_tx.send(WorkerEvent::ChatDone {
                    response,
                    message_count: session.session().messages().len(),
                });
            }
            WorkerCommand::NewSession => {
                match start_worker_session(&conversation, work_dir.as_path()) {
                    Ok(new_session) => {
                        session = new_session;
                        let _ = event_tx.send(WorkerEvent::NewSession {
                            session: SessionSnapshot::from_managed(&session),
                        });
                    }
                    Err(error) => {
                        let _ = event_tx.send(WorkerEvent::Error(format!("新建会话失败：{error}")));
                    }
                }
            }
            WorkerCommand::ListSessions => {
                let sessions = conversation
                    .list_sessions_with_preview(work_dir.as_path())
                    .map_err(|e| format!("{e}"));
                let _ = event_tx.send(WorkerEvent::SessionList { sessions });
            }
            WorkerCommand::SwitchSession { selector } => {
                match super::resolve_session_selector(&conversation, work_dir.as_path(), &selector)
                {
                    Ok(session_id) => {
                        match conversation.load_session(work_dir.as_path(), &session_id) {
                            Ok(loaded_session) => {
                                session = loaded_session;
                                let _ = event_tx.send(WorkerEvent::SessionLoaded {
                                    session: SessionSnapshot::from_managed(&session),
                                });
                            }
                            Err(error) => {
                                let _ = event_tx
                                    .send(WorkerEvent::Error(format!("切换会话失败：{error}")));
                            }
                        }
                    }
                    Err(error) => {
                        let _ = event_tx.send(WorkerEvent::Error(format!("切换会话失败：{error}")));
                    }
                }
            }
            WorkerCommand::QueryStatus => {
                let _ = event_tx.send(WorkerEvent::Status {
                    message_count: session.session().messages().len(),
                });
            }
            WorkerCommand::Shutdown => {
                break;
            }
        }
    }
}

fn process_worker_events(app: &mut TuiApp, event_rx: &mut mpsc::UnboundedReceiver<WorkerEvent>) {
    while let Ok(worker_event) = event_rx.try_recv() {
        match worker_event {
            WorkerEvent::Ready {
                tool_names,
                session,
            } => {
                app.tool_names = tool_names;
                app.current_session_id = Some(session.id);
                app.message_count = session.message_count;
                app.push_system("Agent 已就绪。");
            }
            WorkerEvent::State(state) => app.push_state(&state),
            WorkerEvent::Think(text) => app.push_think(&text),
            WorkerEvent::Delta(delta) => app.append_streaming_delta(&delta),
            WorkerEvent::ToolCall { name, args } => app.push_tool_call(&name, &args),
            WorkerEvent::ToolResult { name, result } => app.push_tool_result(&name, &result),
            WorkerEvent::ChatDone {
                response,
                message_count,
            } => {
                app.busy = false;
                app.message_count = message_count;
                match response {
                    Ok(text) => app.finalize_streaming(&text),
                    Err(err) => {
                        app.streaming_index = None;
                        app.push_error(&format!("Agent 错误：{err}"));
                    }
                }
            }
            WorkerEvent::Status { message_count } => {
                app.message_count = message_count;
            }
            WorkerEvent::NewSession { session } => {
                app.clear_messages_and_events();
                app.current_session_id = Some(session.id);
                app.message_count = session.message_count;
            }
            WorkerEvent::SessionLoaded { session } => {
                let session_id = session.id.clone();
                app.load_session_snapshot(session, &format!("已切换到会话：{session_id}"));
            }
            WorkerEvent::SessionList { sessions } => match sessions {
                Ok(sessions) => app.show_sessions(&sessions),
                Err(error) => app.push_error(&format!("会话列表读取失败：{error}")),
            },
            WorkerEvent::Error(message) => {
                app.busy = false;
                app.push_error(&message);
            }
        }
    }
}

fn process_terminal_key(
    key: KeyEvent,
    app: &mut TuiApp,
    command_tx: &mpsc::UnboundedSender<WorkerCommand>,
) {
    if is_quit_key(key.code, key.modifiers) {
        app.should_quit = true;
        return;
    }

    if app.show_help {
        if matches!(key.code, KeyCode::Esc | KeyCode::F(1)) {
            app.show_help = false;
        }
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('l') => {
                app.toggle_event_panel();
                return;
            }
            KeyCode::Char('p') => {
                app.history_prev();
                return;
            }
            KeyCode::Char('n') => {
                app.history_next();
                return;
            }
            KeyCode::Char('k') => {
                app.scroll_event_up(1);
                return;
            }
            KeyCode::Char('j') => {
                app.scroll_event_down(1);
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::F(1) => {
            app.show_help = true;
        }
        KeyCode::Up => {
            if app.input.is_empty() {
                app.scroll_chat_up(1);
            } else {
                app.input.move_up();
            }
        }
        KeyCode::Down => {
            if app.input.is_empty() {
                app.scroll_chat_down(1);
            } else {
                app.input.move_down();
            }
        }
        KeyCode::Left => app.input.move_left(),
        KeyCode::Right => app.input.move_right(),
        KeyCode::PageUp => app.scroll_chat_up(app.page_scroll_amount()),
        KeyCode::PageDown => app.scroll_chat_down(app.page_scroll_amount()),
        KeyCode::Home => {
            if app.input.is_empty() {
                app.scroll_chat_to_top();
            } else {
                app.input.move_line_start();
            }
        }
        KeyCode::End => {
            if app.input.is_empty() {
                app.scroll_chat_to_bottom();
            } else {
                app.input.move_line_end();
            }
        }
        KeyCode::Tab => {
            app.apply_tab_completion();
        }
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.input.newline();
                app.on_input_mutation();
                return;
            }

            if app.busy {
                app.push_system("Agent 正在处理上一条请求，请稍候。");
                return;
            }

            let raw = app.input.text();
            app.input.clear();
            app.history.push(raw.clone());
            match parse_user_input(&raw) {
                UserInput::Empty => {}
                UserInput::Command(command) => {
                    handle_command(command, app, command_tx);
                }
                UserInput::Chat(message) => {
                    app.push_user(&message);
                    let send_result =
                        command_tx.send(WorkerCommand::Chat(ChatRequest { input: message }));

                    if send_result.is_err() {
                        app.push_error("无法发送请求到 Agent 线程。");
                    } else {
                        app.busy = true;
                    }
                }
            }
        }
        KeyCode::Backspace => {
            app.input.backspace();
            app.on_input_mutation();
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.input.insert_char(ch);
            app.on_input_mutation();
        }
        _ => {}
    }
}

fn is_quit_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char(KEY_CTRL_C | KEY_CTRL_D))
}

pub async fn run(config: Config, work_dir: PathBuf) {
    let mut terminal_guard = match TerminalGuard::new() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("初始化 TUI 失败：{e}");
            return;
        }
    };

    let (command_tx, command_rx) = mpsc::unbounded_channel::<WorkerCommand>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<WorkerEvent>();

    let worker = tokio::spawn(run_worker(config, work_dir.clone(), command_rx, event_tx));

    let mut app = TuiApp::new(work_dir);

    loop {
        process_worker_events(&mut app, &mut event_rx);

        let draw_result = terminal_guard.terminal.draw(|frame| app.draw(frame));
        if draw_result.is_err() {
            app.push_error("TUI 绘制失败，正在退出。");
            app.should_quit = true;
        }

        if app.should_quit {
            break;
        }

        if event::poll(POLL_INTERVAL).unwrap_or(false)
            && let Ok(CrosstermEvent::Key(key)) = event::read()
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            process_terminal_key(key, &mut app, &command_tx);
        }
    }

    let _ = command_tx.send(WorkerCommand::Shutdown);
    let _ = worker.await;
}

fn handle_command(
    command: Command,
    app: &mut TuiApp,
    command_tx: &mpsc::UnboundedSender<WorkerCommand>,
) {
    match command {
        Command::Help => app.show_help_text(),
        Command::NewSession => {
            if command_tx.send(WorkerCommand::NewSession).is_err() {
                app.push_error("无法新建 Agent 会话。");
            } else {
                app.busy = false;
            }
        }
        Command::ListSessions => {
            if command_tx.send(WorkerCommand::ListSessions).is_err() {
                app.push_error("无法读取会话列表。");
            }
        }
        Command::SwitchSession { selector } => {
            if command_tx
                .send(WorkerCommand::SwitchSession { selector })
                .is_err()
            {
                app.push_error("无法切换 Agent 会话。");
            }
        }
        Command::Status => {
            if command_tx.send(WorkerCommand::QueryStatus).is_err() {
                app.push_error("无法查询 Agent 状态。");
            }
            app.show_status();
        }
        Command::Tools => app.show_tools(),
        Command::Exit => app.should_quit = true,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyModifiers};

    use super::{
        DisplayMessage, DisplayRole, HELP_EVENT_SUMMARY, HELP_TEXT, InputBuffer, SessionSnapshot,
        TuiApp, is_quit_key,
    };

    #[test]
    fn help_command_uses_single_message() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let message_count = app.messages.len();
        let event_count = app.events.len();

        app.show_help_text();

        let expected_help = HELP_TEXT.join("\n");
        assert_eq!(
            app.messages.len(),
            message_count + 1,
            "帮助命令应只追加一条消息"
        );
        assert_eq!(
            app.events.len(),
            event_count + 1,
            "帮助命令应只追加一条事件"
        );
        assert_eq!(
            app.messages.last().map(|msg| msg.content.as_str()),
            Some(expected_help.as_str()),
            "帮助消息内容应保留完整多行文本"
        );
        assert_eq!(
            app.events.last().map(|event| event.text.as_str()),
            Some(HELP_EVENT_SUMMARY),
            "帮助事件应使用摘要文本"
        );
    }

    #[test]
    fn session_snapshot_rebuilds_chat_messages() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let snapshot = SessionSnapshot {
            id: "session-123".to_owned(),
            message_count: 2,
            messages: vec![
                DisplayMessage {
                    role: DisplayRole::User,
                    content: "hello".to_owned(),
                },
                DisplayMessage {
                    role: DisplayRole::Agent,
                    content: "world".to_owned(),
                },
            ],
        };

        app.load_session_snapshot(snapshot, "已切换到会话：session-123");

        assert_eq!(app.current_session_id.as_deref(), Some("session-123"));
        assert_eq!(app.message_count, 2);
        assert_eq!(app.messages.len(), 3);
        assert_eq!(
            app.messages
                .iter()
                .rev()
                .nth(1)
                .map(|message| message.content.as_str()),
            Some("world")
        );
    }

    #[test]
    fn cursor_display_col_counts_full_width_chars() {
        let mut input = InputBuffer::new();
        input.insert_char('中');
        input.insert_char('a');
        input.insert_char('文');

        assert_eq!(input.cursor_display_col(), 5, "中文字符应按双宽计算");

        input.move_left();

        assert_eq!(
            input.cursor_display_col(),
            3,
            "左移后光标仍应按显示宽度定位"
        );
    }

    #[test]
    fn ctrl_d_is_quit_key() {
        assert!(
            is_quit_key(KeyCode::Char('d'), KeyModifiers::CONTROL),
            "Ctrl+D 应触发退出"
        );
        assert!(
            is_quit_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            "Ctrl+C 应继续触发退出"
        );
        assert!(
            !is_quit_key(KeyCode::Char('d'), KeyModifiers::NONE),
            "普通 d 不应触发退出"
        );
    }
}
