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

mod completion;
mod event_format;
mod input;
mod layout;
mod markdown;
mod message;
mod overlay;
mod terminal;
mod timestamp;

use std::mem;
use std::path::PathBuf;
use std::sync::{Arc, mpsc as std_mpsc};
use std::time::Duration;

use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use secaudit_agent::Agent;
use secaudit_agent::TokenUsage;
use secaudit_agent::llm::fetch_context_window;
use secaudit_agent::state::AgentState;
use secaudit_conversation::{ContextUsage, SessionListItem};
use secaudit_core::Config;
use tokio::sync::mpsc;

use super::commands::{Command, UserInput};
use super::{
    ChatRequest, DisplayMessage, DisplayRole, SessionSnapshot, WorkerCommand, WorkerEvent,
    build_worker_conversation, parse_user_input, start_worker_session,
};
use completion::complete_command_input;
use event_format::{summarize_tool_args, summarize_tool_result};
use input::{History, InputBuffer};
use layout::split_tui_areas;
use message::{ChatMessage, EventEntry, EventKind, MessageRole};
use overlay::{centered_rect, draw_confirmation_overlay};
use terminal::TerminalGuard;

const POLL_INTERVAL: Duration = Duration::from_millis(16);
const PENDING_USAGE_STEP: u64 = 16;
const PENDING_USAGE_LIMIT: u64 = 1_200;
const MAX_EVENT_LINES: usize = 500;
const MAX_WORKER_EVENTS_PER_TICK: usize = 128;
const MAX_TERMINAL_EVENTS_PER_TICK: usize = 64;
const MAX_MESSAGE_ITEMS: usize = 240;
const KEY_CTRL_C: char = 'c';
const KEY_CTRL_D: char = 'd';
const DEFAULT_VIEWPORT_HEIGHT: u16 = 10;

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
    "  /usage         显示 Token 用量",
    "  /context       显示上下文占用",
    "  /tools         列出工具",
    "  /skills        列出可用 Skills",
    "  /exit          退出",
];

const HELP_EVENT_SUMMARY: &str = "已显示帮助。";
const TOOLS_EVENT_SUMMARY: &str = "已显示可用工具。";
const SKILLS_EVENT_SUMMARY: &str = "已显示可用 Skills。";
const CLEAR_EVENT_SUMMARY: &str = "已开启新会话，当前视图与事件已清空。";
const SESSIONS_EVENT_SUMMARY: &str = "已显示会话列表。";
const SWITCH_SESSION_EVENT_SUMMARY: &str = "已切换会话。";
const SESSION_PREVIEW_MAX_CHARS: usize = 96;

struct PendingConfirmation {
    prompt: String,
    response_tx: std_mpsc::Sender<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusTab {
    Settings,
    Status,
    Config,
    Usage,
    Stats,
}

impl StatusTab {
    const ALL: [Self; 5] = [
        Self::Settings,
        Self::Status,
        Self::Config,
        Self::Usage,
        Self::Stats,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Status => "Status",
            Self::Config => "Config",
            Self::Usage => "Usage",
            Self::Stats => "Stats",
        }
    }

    fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        let next_idx = (idx + 1) % Self::ALL.len();
        Self::ALL.get(next_idx).copied().unwrap_or(self)
    }

    fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        let prev_idx = if idx == 0 {
            Self::ALL.len().saturating_sub(1)
        } else {
            idx - 1
        };
        Self::ALL.get(prev_idx).copied().unwrap_or(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Status(StatusTab),
    Usage,
    Context,
}

struct TuiApp {
    work_dir: PathBuf,
    input: InputBuffer,
    history: History,
    messages: Vec<ChatMessage>,
    events: Vec<EventEntry>,
    should_quit: bool,
    busy: bool,
    overlay: Overlay,
    tool_names: Vec<String>,
    skill_list: Vec<(String, String)>,
    current_session_id: Option<String>,
    message_count: usize,
    last_state_label: String,
    model: String,
    api_base_url: String,
    max_iterations: u32,
    reasoning_strategy: String,
    cumulative_usage: TokenUsage,
    last_turn_usage: TokenUsage,
    displayed_cumulative_usage: TokenUsage,
    displayed_last_turn_usage: TokenUsage,
    usage_animation_base: TokenUsage,
    turn_usage_resolved: bool,
    context_usage: ContextUsage,
    chat_scroll: u16,
    follow_chat: bool,
    chat_viewport_height: u16,
    chat_viewport_width: u16,
    event_scroll: u16,
    event_viewport_height: u16,
    event_viewport_width: u16,
    event_panel_collapsed: bool,
    pending_confirmation: Option<PendingConfirmation>,
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
            overlay: Overlay::None,
            tool_names: Vec::new(),
            skill_list: Vec::new(),
            current_session_id: None,
            message_count: 0,
            last_state_label: "就绪".to_owned(),
            model: String::new(),
            api_base_url: String::new(),
            max_iterations: 0,
            reasoning_strategy: String::new(),
            cumulative_usage: TokenUsage::default(),
            last_turn_usage: TokenUsage::default(),
            displayed_cumulative_usage: TokenUsage::default(),
            displayed_last_turn_usage: TokenUsage::default(),
            usage_animation_base: TokenUsage::default(),
            turn_usage_resolved: true,
            context_usage: ContextUsage::default(),
            chat_scroll: 0,
            follow_chat: true,
            chat_viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            chat_viewport_width: DEFAULT_VIEWPORT_HEIGHT,
            event_scroll: 0,
            event_viewport_height: DEFAULT_VIEWPORT_HEIGHT,
            event_viewport_width: DEFAULT_VIEWPORT_HEIGHT,
            event_panel_collapsed: false,
            pending_confirmation: None,
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
        self.messages.push(ChatMessage::new(role, text));

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
            self.messages.push(ChatMessage::empty_agent());
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
        self.events.push(EventEntry::new(kind, text));

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
        self.cumulative_usage = TokenUsage::default();
        self.last_turn_usage = TokenUsage::default();
        self.displayed_cumulative_usage = TokenUsage::default();
        self.displayed_last_turn_usage = TokenUsage::default();
        self.usage_animation_base = TokenUsage::default();
        self.turn_usage_resolved = true;
        self.context_usage = ContextUsage::default();
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
        self.cumulative_usage = TokenUsage::default();
        self.last_turn_usage = TokenUsage::default();
        self.displayed_cumulative_usage = TokenUsage::default();
        self.displayed_last_turn_usage = TokenUsage::default();
        self.usage_animation_base = TokenUsage::default();
        self.turn_usage_resolved = true;
        self.context_usage = ContextUsage::default();

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

    fn open_status_overlay(&mut self, tab: StatusTab) {
        self.overlay = Overlay::Status(tab);
        self.push_event(EventKind::System, "Opened status view".to_owned());
    }

    fn open_usage_overlay(&mut self) {
        self.overlay = Overlay::Usage;
        self.push_event(EventKind::System, "Opened usage view".to_owned());
    }

    fn reset_turn_usage_animation(&mut self) {
        self.last_turn_usage = TokenUsage::default();
        self.displayed_last_turn_usage = TokenUsage::default();
        self.usage_animation_base = self.cumulative_usage;
        self.displayed_cumulative_usage = self.cumulative_usage;
        self.turn_usage_resolved = false;
    }

    fn sync_usage_display(&mut self) {
        self.displayed_cumulative_usage = self.cumulative_usage;
        self.displayed_last_turn_usage = self.last_turn_usage;
        self.usage_animation_base = self.cumulative_usage;
        self.turn_usage_resolved = true;
    }

    fn animate_usage_display(&mut self) {
        if self.busy && !self.turn_usage_resolved {
            self.displayed_last_turn_usage = advance_pending_usage(self.displayed_last_turn_usage);
            self.displayed_cumulative_usage =
                add_usage(self.usage_animation_base, self.displayed_last_turn_usage);
        }
    }

    fn open_context_overlay(&mut self) {
        self.overlay = Overlay::Context;
        self.push_event(EventKind::System, "Opened context view".to_owned());
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
            lines.push(format!(
                "    预览：{}",
                item.preview_text(SESSION_PREVIEW_MAX_CHARS)
            ));
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

    fn show_skills(&mut self) {
        let text = if self.skill_list.is_empty() {
            "未找到可用 Skills。\n\
            Skills 存放在项目 .secaudit/skills/<skill-name>/SKILL.md 或 ~/.secaudit/skills/<skill-name>/SKILL.md 中。\n\
            使用方式: /skill-name [arguments]"
                .to_owned()
        } else {
            let mut out = format!("可用 Skills（{}个）：\n\n", self.skill_list.len());
            for (name, desc) in &self.skill_list {
                use std::fmt::Write as _;
                let _ = writeln!(out, "  /{name} — {desc}");
            }
            out.push_str("\n使用方式: /skill-name [arguments]");
            out
        };
        self.push_system_block(&text, SKILLS_EVENT_SUMMARY);
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

    fn request_confirmation(&mut self, prompt: String, response_tx: std_mpsc::Sender<bool>) {
        if let Some(pending) = self.pending_confirmation.take() {
            let _ = pending.response_tx.send(false);
        }

        self.push_event(EventKind::System, format!("确认请求：{prompt}"));
        self.pending_confirmation = Some(PendingConfirmation {
            prompt,
            response_tx,
        });
    }

    fn resolve_confirmation(&mut self, approved: bool) {
        if let Some(pending) = self.pending_confirmation.take() {
            let _ = pending.response_tx.send(approved);
            let result = if approved { "已允许" } else { "已拒绝" };
            self.push_event(EventKind::System, format!("确认结果：{result}"));
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
        let Some(areas) = split_tui_areas(frame.area(), self.event_panel_collapsed) else {
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
        let model_display = if self.model.is_empty() {
            "-".to_owned()
        } else {
            self.model.clone()
        };
        let tokens_display = format!(
            "{}",
            self.displayed_cumulative_usage
                .prompt_tokens
                .saturating_add(self.displayed_cumulative_usage.completion_tokens),
        );
        let context_display = format!("{} messages", self.message_count);
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
            Line::from(vec![
                Span::styled("Model   ", Style::default().fg(Color::Gray)),
                Span::raw(model_display),
            ]),
            Line::from(vec![
                Span::styled("Tokens  ", Style::default().fg(Color::Gray)),
                Span::raw(tokens_display),
            ]),
            Line::from(vec![
                Span::styled("Context ", Style::default().fg(Color::Gray)),
                Span::raw(context_display),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title("Runtime"));

        self.chat_viewport_height = areas.chat.height.saturating_sub(2);
        self.chat_viewport_width = areas.chat.width.saturating_sub(2).max(1);
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

        frame.render_widget(header_title, areas.workspace);
        frame.render_widget(metrics, areas.runtime);
        frame.render_widget(chat_panel, areas.chat);

        if self.event_panel_collapsed {
            let collapsed = Paragraph::new("E")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::LEFT));
            frame.render_widget(collapsed, areas.event);
        } else {
            self.event_viewport_height = areas.event.height.saturating_sub(2);
            self.event_viewport_width = areas.event.width.saturating_sub(2).max(1);
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
            frame.render_widget(event_panel, areas.event);
        }

        let input_title = if self.busy {
            "Input · Agent running"
        } else {
            "Input"
        };

        let input_panel = Paragraph::new(self.input.visual_lines())
            .block(Block::default().borders(Borders::ALL).title(input_title))
            .wrap(Wrap { trim: false });

        let footer = Paragraph::new(format!("{FOOTER_HINT}  /usage /context"))
            .style(Style::default().fg(Color::DarkGray));

        frame.render_widget(input_panel, areas.input);
        frame.render_widget(footer, areas.footer);

        if let Some(pending) = &self.pending_confirmation {
            draw_confirmation_overlay(frame, &pending.prompt);
        } else if self.overlay != Overlay::None {
            draw_overlay(frame, self);
        } else {
            let col_u16 = u16::try_from(self.input.cursor_display_col()).unwrap_or(u16::MAX);
            let line_u16 = u16::try_from(self.input.cursor_line).unwrap_or(u16::MAX);
            let cursor_x = areas.input.x.saturating_add(1).saturating_add(col_u16);
            let cursor_y = areas.input.y.saturating_add(1).saturating_add(line_u16);
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
        if let Some(completed) = complete_command_input(&text) {
            self.input.set_text(completed);
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

fn draw_overlay(frame: &mut Frame<'_>, app: &TuiApp) {
    match app.overlay {
        Overlay::None => {}
        Overlay::Status(tab) => draw_status_overlay(frame, app, tab),
        Overlay::Usage => draw_usage_overlay(frame, app),
        Overlay::Context => draw_context_overlay(frame, app),
    }
}

fn draw_status_overlay(frame: &mut Frame<'_>, app: &TuiApp, active_tab: StatusTab) {
    let area = centered_rect(78, 74, frame.area());
    let mut lines = Vec::new();

    lines.push(render_status_tabs(active_tab));
    lines.push(Line::from(String::new()));
    lines.extend(match active_tab {
        StatusTab::Settings => render_settings_lines(app),
        StatusTab::Status => render_status_lines(app),
        StatusTab::Config => render_config_lines(app),
        StatusTab::Usage => render_usage_lines(app),
        StatusTab::Stats => render_stats_lines(app),
    });
    lines.push(Line::from(String::new()));
    lines.push(Line::from(Span::styled(
        "Esc close | Left/Right switch tabs | /usage and /context open focused views",
        Style::default().fg(Color::DarkGray),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Status")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
}

fn draw_usage_overlay(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(72, 58, frame.area());
    let mut lines = vec![Line::from(Span::styled(
        "Token Usage",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(String::new()));
    lines.extend(render_usage_lines(app));
    lines.push(Line::from(String::new()));
    lines.push(Line::from(Span::styled(
        "Command: /usage | Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Usage")
                .border_style(Style::default().fg(Color::LightBlue)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
}

fn draw_context_overlay(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(82, 72, frame.area());
    let context = &app.context_usage;
    let mut lines = vec![
        Line::from(Span::styled(
            "Context",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Estimated current prompt: {} / {} tokens ({}%)",
            context.used_tokens,
            context.window_tokens,
            context.used_percent()
        )),
        Line::from(format!("Estimator: {}", context.token_estimator.label())),
        Line::from(String::new()),
        render_context_bar(context.used_percent()),
        Line::from(String::new()),
    ];
    lines.extend(render_context_grid(context.used_percent()));
    lines.push(Line::from(String::new()));
    lines.extend(render_context_breakdown(context));
    lines.push(Line::from(String::new()));
    lines.push(Line::from(Span::styled(
        "Command: /context | Esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Context")
                .border_style(Style::default().fg(Color::LightMagenta)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
}

fn render_status_tabs(active_tab: StatusTab) -> Line<'static> {
    let mut spans = Vec::new();
    for tab in StatusTab::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }

        let label = format!(" {} ", tab.label());
        let style = if tab == active_tab {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
    }
    Line::from(spans)
}

fn render_settings_lines(app: &TuiApp) -> Vec<Line<'static>> {
    vec![
        kv_line("Working dir", app.work_dir.display().to_string()),
        kv_line(
            "Session",
            app.current_session_id.as_deref().unwrap_or("pending"),
        ),
        kv_line("Model", display_or_dash(&app.model)),
        kv_line("Strategy", display_or_dash(&app.reasoning_strategy)),
        kv_line("Max iterations", display_u32(app.max_iterations)),
    ]
}

fn render_status_lines(app: &TuiApp) -> Vec<Line<'static>> {
    vec![
        kv_line("Agent", if app.busy { "Running" } else { "Idle" }),
        kv_line("State", &app.last_state_label),
        kv_line("Messages", app.message_count.to_string()),
        kv_line("Tools", app.tool_names.len().to_string()),
        kv_line(
            "Event panel",
            if app.event_panel_collapsed {
                "Collapsed"
            } else {
                "Expanded"
            },
        ),
    ]
}

fn render_config_lines(app: &TuiApp) -> Vec<Line<'static>> {
    vec![
        kv_line("API base URL", display_or_dash(&app.api_base_url)),
        kv_line("Model", display_or_dash(&app.model)),
        kv_line("Strategy", display_or_dash(&app.reasoning_strategy)),
        kv_line("Max iterations", display_u32(app.max_iterations)),
    ]
}

fn render_usage_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let cumulative_total = usage_total(app.displayed_cumulative_usage);
    let last_total = usage_total(app.displayed_last_turn_usage);
    vec![
        kv_line("Total tokens", cumulative_total.to_string()),
        kv_line(
            "Prompt tokens",
            app.displayed_cumulative_usage.prompt_tokens.to_string(),
        ),
        kv_line(
            "Completion tokens",
            app.displayed_cumulative_usage.completion_tokens.to_string(),
        ),
        kv_line("Last turn total", last_total.to_string()),
        kv_line(
            "Last turn prompt",
            app.displayed_last_turn_usage.prompt_tokens.to_string(),
        ),
        kv_line(
            "Last turn completion",
            app.displayed_last_turn_usage.completion_tokens.to_string(),
        ),
    ]
}

fn render_stats_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let context = &app.context_usage;
    vec![
        kv_line("Messages", app.message_count.to_string()),
        kv_line("Events", app.events.len().to_string()),
        kv_line("Tools", app.tool_names.len().to_string()),
        kv_line("Context used", format!("{}%", context.used_percent())),
        kv_line(
            "Cumulative tokens",
            usage_total(app.displayed_cumulative_usage).to_string(),
        ),
    ]
}

fn advance_pending_usage(current: TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: advance_pending_counter(current.prompt_tokens),
        completion_tokens: advance_pending_counter(current.completion_tokens),
        total_tokens: advance_pending_counter(current.total_tokens),
    }
}

fn advance_pending_counter(current: u64) -> u64 {
    current
        .saturating_add(PENDING_USAGE_STEP)
        .min(PENDING_USAGE_LIMIT)
}

fn add_usage(left: TokenUsage, right: TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: left.prompt_tokens.saturating_add(right.prompt_tokens),
        completion_tokens: left
            .completion_tokens
            .saturating_add(right.completion_tokens),
        total_tokens: left.total_tokens.saturating_add(right.total_tokens),
    }
}

fn kv_line(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<18}"), Style::default().fg(Color::Gray)),
        Span::raw(value.into()),
    ])
}

fn display_or_dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_owned()
    } else {
        value.to_owned()
    }
}

fn display_u32(value: u32) -> String {
    if value == 0 {
        "-".to_owned()
    } else {
        value.to_string()
    }
}

fn usage_total(usage: TokenUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens.saturating_add(usage.completion_tokens)
    }
}

fn render_context_bar(used_percent: u64) -> Line<'static> {
    const WIDTH: usize = 48;
    let filled = usize::try_from(used_percent)
        .unwrap_or(0)
        .saturating_mul(WIDTH)
        / 100;
    let mut spans = Vec::with_capacity(WIDTH);
    for idx in 0..WIDTH {
        let style = if idx < filled {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled("█", style));
    }
    Line::from(spans)
}

fn render_context_grid(used_percent: u64) -> Vec<Line<'static>> {
    const COLS: usize = 25;
    const ROWS: usize = 4;
    let used_cells = usize::try_from(used_percent)
        .unwrap_or(0)
        .min(100)
        .saturating_mul(COLS * ROWS)
        / 100;
    let mut lines = Vec::with_capacity(ROWS);

    for row in 0..ROWS {
        let mut spans = Vec::with_capacity(COLS * 2);
        for col in 0..COLS {
            let idx = row * COLS + col;
            let style = if idx < used_cells {
                Style::default().fg(Color::LightGreen)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled("■", style));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }

    lines
}

fn render_context_breakdown(context: &ContextUsage) -> Vec<Line<'static>> {
    vec![
        context_part_line(
            "System prompt",
            context.system_tokens,
            context.window_tokens,
        ),
        context_part_line("Tools", context.tool_tokens, context.window_tokens),
        context_part_line("Messages", context.message_tokens, context.window_tokens),
        context_part_line("Free space", context.free_tokens, context.window_tokens),
    ]
}

fn context_part_line(label: &str, tokens: u64, total_tokens: u64) -> Line<'static> {
    let tenths = percent_tenths(tokens, total_tokens);
    kv_line(
        label,
        format!("{}.{:01}%  {} tokens", tenths / 10, tenths % 10, tokens),
    )
}

fn percent_tenths(value: u64, total: u64) -> u64 {
    value.saturating_mul(1_000).checked_div(total).unwrap_or(0)
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
        let (response_tx, response_rx) = std_mpsc::channel();
        if event_tx
            .send(WorkerEvent::ConfirmRequest {
                prompt: prompt.to_owned(),
                response_tx,
            })
            .is_err()
        {
            return false;
        }

        response_rx.recv().unwrap_or(false)
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
        agent.on_usage(move |usage| {
            let _ = tx.send(WorkerEvent::Usage(usage));
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
    let model = config.model.clone();
    let api_base_url = config.api_base_url.clone();
    let max_iterations = config.max_iterations;
    let reasoning_strategy = config.reasoning_strategy.clone();
    let context_window_tokens = if config.has_context_window_override() {
        config.context_window_tokens
    } else {
        fetch_context_window(&config)
            .await
            .unwrap_or(config.context_window_tokens)
    };
    let mut agent = Agent::new(config, work_dir.clone(), cli_confirm(event_tx.clone()));
    bind_agent_callbacks(&mut agent, &event_tx);

    let tool_names = agent.tool_names();
    let skill_list = agent.skill_list();

    let conversation = match build_worker_conversation(context_window_tokens, &model) {
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
        skill_list,
        session: SessionSnapshot::from_managed(&session),
        model,
        api_base_url,
        max_iterations,
        reasoning_strategy,
        context_usage: conversation.active_context_usage(&session),
    });

    while let Some(command) = command_rx.recv().await {
        match command {
            WorkerCommand::Chat(request) => {
                let previous_usage = TokenUsage::sum_from_messages(session.session().messages());
                let response = conversation
                    .chat(&mut agent, &mut session, &request.input)
                    .await
                    .map_err(|e| format!("{e}"));

                let usage = TokenUsage::sum_from_messages(session.session().messages());
                let last_turn_usage = TokenUsage {
                    prompt_tokens: usage
                        .prompt_tokens
                        .saturating_sub(previous_usage.prompt_tokens),
                    completion_tokens: usage
                        .completion_tokens
                        .saturating_sub(previous_usage.completion_tokens),
                    total_tokens: usage
                        .total_tokens
                        .saturating_sub(previous_usage.total_tokens),
                };
                let _ = event_tx.send(WorkerEvent::ChatDone {
                    response,
                    message_count: session.session().messages().len(),
                    usage,
                    last_turn_usage,
                    context_usage: conversation.active_context_usage(&session),
                });
            }
            WorkerCommand::NewSession => {
                match start_worker_session(&conversation, work_dir.as_path()) {
                    Ok(new_session) => {
                        session = new_session;
                        let _ = event_tx.send(WorkerEvent::NewSession {
                            session: SessionSnapshot::from_managed(&session),
                            context_usage: conversation.active_context_usage(&session),
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
                                    usage: TokenUsage::sum_from_messages(
                                        session.session().messages(),
                                    ),
                                    context_usage: conversation.active_context_usage(&session),
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
                    usage: TokenUsage::sum_from_messages(session.session().messages()),
                    context_usage: conversation.active_context_usage(&session),
                });
            }
            WorkerCommand::Shutdown => {
                break;
            }
        }
    }
}

fn process_worker_events(app: &mut TuiApp, event_rx: &mut mpsc::UnboundedReceiver<WorkerEvent>) {
    for _ in 0..MAX_WORKER_EVENTS_PER_TICK {
        let Ok(worker_event) = event_rx.try_recv() else {
            break;
        };

        match worker_event {
            WorkerEvent::Ready {
                tool_names,
                skill_list,
                session,
                model,
                api_base_url,
                max_iterations,
                reasoning_strategy,
                context_usage,
            } => {
                app.tool_names = tool_names;
                app.skill_list = skill_list;
                app.current_session_id = Some(session.id);
                app.message_count = session.message_count;
                app.model = model;
                app.api_base_url = api_base_url;
                app.max_iterations = max_iterations;
                app.reasoning_strategy = reasoning_strategy;
                app.context_usage = context_usage;
                app.push_system("Agent 已就绪。");
            }
            WorkerEvent::State(state) => app.push_state(&state),
            WorkerEvent::Think(text) => app.push_think(&text),
            WorkerEvent::Delta(delta) => app.append_streaming_delta(&delta),
            WorkerEvent::Usage(usage) => {
                app.last_turn_usage += usage;
                app.cumulative_usage += usage;
                app.sync_usage_display();
            }
            WorkerEvent::ToolCall { name, args } => app.push_tool_call(&name, &args),
            WorkerEvent::ToolResult { name, result } => app.push_tool_result(&name, &result),
            WorkerEvent::ChatDone {
                response,
                message_count,
                usage,
                last_turn_usage,
                context_usage,
            } => {
                app.busy = false;
                app.message_count = message_count;
                app.last_turn_usage = last_turn_usage;
                app.cumulative_usage = usage;
                app.sync_usage_display();
                app.context_usage = context_usage;
                match response {
                    Ok(text) => app.finalize_streaming(&text),
                    Err(err) => {
                        app.streaming_index = None;
                        app.push_error(&format!("Agent 错误：{err}"));
                    }
                }
            }
            WorkerEvent::Status {
                message_count,
                usage,
                context_usage,
            } => {
                app.message_count = message_count;
                app.cumulative_usage = usage;
                app.sync_usage_display();
                app.context_usage = context_usage;
            }
            WorkerEvent::NewSession {
                session,
                context_usage,
            } => {
                app.clear_messages_and_events();
                app.current_session_id = Some(session.id);
                app.message_count = session.message_count;
                app.context_usage = context_usage;
            }
            WorkerEvent::SessionLoaded {
                session,
                usage,
                context_usage,
            } => {
                let session_id = session.id.clone();
                app.load_session_snapshot(session, &format!("已切换到会话：{session_id}"));
                app.cumulative_usage = usage;
                app.last_turn_usage = TokenUsage::default();
                app.sync_usage_display();
                app.context_usage = context_usage;
            }
            WorkerEvent::SessionList { sessions } => match sessions {
                Ok(sessions) => app.show_sessions(&sessions),
                Err(error) => app.push_error(&format!("会话列表读取失败：{error}")),
            },
            WorkerEvent::ConfirmRequest {
                prompt,
                response_tx,
            } => {
                app.request_confirmation(prompt, response_tx);
            }
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
        app.resolve_confirmation(false);
        app.should_quit = true;
        return;
    }

    if app.pending_confirmation.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => app.resolve_confirmation(true),
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                app.resolve_confirmation(false);
            }
            _ => {}
        }
        return;
    }

    if app.overlay != Overlay::None {
        match key.code {
            KeyCode::Esc => app.overlay = Overlay::None,
            KeyCode::F(1) => app.show_help_text(),
            KeyCode::Left => {
                if let Overlay::Status(tab) = app.overlay {
                    app.overlay = Overlay::Status(tab.prev());
                }
            }
            KeyCode::Right | KeyCode::Tab => {
                if let Overlay::Status(tab) = app.overlay {
                    app.overlay = Overlay::Status(tab.next());
                }
            }
            _ => {}
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
        KeyCode::F(1) => app.show_help_text(),
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
                        app.reset_turn_usage_animation();
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

fn process_terminal_events(
    app: &mut TuiApp,
    command_tx: &mpsc::UnboundedSender<WorkerCommand>,
    wait: Duration,
) {
    if !event::poll(wait).unwrap_or(false) {
        return;
    }

    for _ in 0..MAX_TERMINAL_EVENTS_PER_TICK {
        let Ok(event) = event::read() else {
            break;
        };

        if let CrosstermEvent::Key(key) = event
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            process_terminal_key(key, app, command_tx);
        }

        if app.should_quit || !event::poll(Duration::ZERO).unwrap_or(false) {
            break;
        }
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
        process_terminal_events(&mut app, &command_tx, Duration::ZERO);
        if app.should_quit {
            break;
        }

        process_worker_events(&mut app, &mut event_rx);
        process_terminal_events(&mut app, &command_tx, Duration::ZERO);
        if app.should_quit {
            break;
        }

        app.animate_usage_display();
        let draw_result = terminal_guard.terminal.draw(|frame| app.draw(frame));
        if draw_result.is_err() {
            app.push_error("TUI 绘制失败，正在退出。");
            app.should_quit = true;
        }

        if app.should_quit {
            break;
        }

        process_terminal_events(&mut app, &command_tx, POLL_INTERVAL);
    }

    let _ = command_tx.send(WorkerCommand::Shutdown);
    app.resolve_confirmation(false);
    if app.busy {
        worker.abort();
    }
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
            app.open_status_overlay(StatusTab::Status);
        }
        Command::Usage => app.open_usage_overlay(),
        Command::Context => app.open_context_overlay(),
        Command::Tools => app.show_tools(),
        Command::Skills => app.show_skills(),
        Command::Exit => app.should_quit = true,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use secaudit_agent::TokenUsage;
    use tokio::sync::mpsc::unbounded_channel;

    use super::{
        DisplayMessage, DisplayRole, HELP_EVENT_SUMMARY, HELP_TEXT, InputBuffer,
        MAX_WORKER_EVENTS_PER_TICK, PENDING_USAGE_LIMIT, PENDING_USAGE_STEP, SessionSnapshot,
        TuiApp, WorkerCommand, WorkerEvent, advance_pending_counter, is_quit_key,
        process_terminal_key, process_worker_events, usage_total,
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
    fn pending_usage_counter_keeps_increasing() {
        let next = advance_pending_counter(1_000);

        assert!(next > 1_000);
        assert!(next <= PENDING_USAGE_LIMIT);
    }

    #[test]
    fn pending_usage_adds_to_confirmed_base() {
        let mut app = TuiApp::new(PathBuf::from("."));
        app.cumulative_usage = TokenUsage {
            prompt_tokens: 800_000,
            completion_tokens: 800_000,
            total_tokens: 1_600_000,
        };
        app.sync_usage_display();
        app.busy = true;
        app.reset_turn_usage_animation();
        app.animate_usage_display();

        assert_eq!(
            usage_total(app.displayed_cumulative_usage),
            1_600_000 + PENDING_USAGE_STEP
        );
    }

    #[test]
    fn real_usage_syncs_display_immediately() {
        let mut app = TuiApp::new(PathBuf::from("."));
        app.busy = true;
        app.reset_turn_usage_animation();
        app.animate_usage_display();

        app.cumulative_usage = TokenUsage {
            prompt_tokens: 2_000,
            completion_tokens: 2_000,
            total_tokens: 4_000,
        };
        app.last_turn_usage = app.cumulative_usage;
        app.sync_usage_display();

        assert_eq!(usage_total(app.displayed_cumulative_usage), 4_000);
        assert!(app.turn_usage_resolved);
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

    #[test]
    fn worker_event_processing_is_bounded_per_tick() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let initial_event_count = app.events.len();
        let (tx, mut rx) = unbounded_channel();

        for idx in 0..(MAX_WORKER_EVENTS_PER_TICK + 10) {
            tx.send(WorkerEvent::Think(format!("event {idx}")))
                .expect("send worker event");
        }

        process_worker_events(&mut app, &mut rx);

        assert_eq!(
            app.events.len(),
            initial_event_count + MAX_WORKER_EVENTS_PER_TICK,
            "单帧事件处理应有上限，避免运行中键盘输入被饿死"
        );

        process_worker_events(&mut app, &mut rx);

        assert_eq!(
            app.events.len(),
            initial_event_count + MAX_WORKER_EVENTS_PER_TICK + 10
        );
    }

    #[test]
    fn quit_key_works_while_agent_is_busy() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let (tx, _rx) = unbounded_channel::<WorkerCommand>();

        app.busy = true;
        process_terminal_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &mut app,
            &tx,
        );

        assert!(app.should_quit, "Ctrl+D 运行中也应立即触发退出");
    }

    #[test]
    fn confirmation_request_is_handled_inside_tui() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let (event_tx, mut event_rx) = unbounded_channel();
        let (response_tx, response_rx) = channel();

        event_tx
            .send(WorkerEvent::ConfirmRequest {
                prompt: "允许执行 pwd 吗？".to_owned(),
                response_tx,
            })
            .expect("send confirm request");

        process_worker_events(&mut app, &mut event_rx);

        assert!(app.pending_confirmation.is_some());
        let _ = response_rx.try_recv().unwrap_err();
    }

    #[test]
    fn confirmation_keys_do_not_mutate_input() {
        let mut app = TuiApp::new(PathBuf::from("."));
        let (command_tx, _command_rx) = unbounded_channel::<WorkerCommand>();
        let (response_tx, response_rx) = channel();

        app.request_confirmation("允许执行 pwd 吗？".to_owned(), response_tx);
        process_terminal_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
            &command_tx,
        );

        assert!(app.input.is_empty());
        let _ = response_rx.try_recv().unwrap_err();

        process_terminal_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
            &command_tx,
        );

        assert!(response_rx.recv().expect("confirmation response"));
        assert!(app.pending_confirmation.is_none());
        assert!(app.input.is_empty());
    }
}
