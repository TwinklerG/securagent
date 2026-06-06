//! 非交互调试模式：记录轨迹并输出结构化 JSON。

use std::fmt::Display;
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};

use colored::Colorize;
use secaudit_agent::{Agent, ChatMessage, Session, TokenUsage};
use secaudit_conversation::{
    ConversationService, ManagedSession, SessionManagementInfo, SessionMetadata,
};
use serde::Serialize;

use crate::output;
use crate::{ConfirmMode, OutputFormat};

#[derive(Serialize)]
struct SessionListOutput<'a> {
    status: &'static str,
    sessions: &'a [SessionMetadata],
}

#[derive(Serialize)]
struct ArchiveSessionOutput<'a> {
    status: &'static str,
    session: &'a SessionMetadata,
}

const DEFAULT_CHAT_MESSAGE: &str = "请审计当前工作目录的安全风险，并给出高优先级问题清单。";
const MSG_USER_DENIED: &str = "用户拒绝执行该命令";
const CONFIRM_SOURCE_AUTO_ALLOW: &str = "auto_allow";
const CONFIRM_SOURCE_AUTO_DENY: &str = "auto_deny";
const CONFIRM_SOURCE_STDIN_PROMPT: &str = "stdin_prompt";

/// 工具调用记录。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// 工具名称。
    pub name: String,
    /// 工具参数。
    pub args: String,
    /// 工具结果。
    pub result: String,
}

/// 用户确认事件。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfirmEvent {
    /// 确认提示文案。
    pub prompt: String,
    /// 用户是否批准。
    pub approved: bool,
    /// 确认模式。
    pub mode: String,
    /// 决策来源（如 `auto_allow`、`stdin_prompt`）。
    pub source: String,
}

/// 轨迹快照。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TraceSnapshot {
    /// 工具调用记录。
    pub tool_calls: Vec<ToolCallRecord>,
    /// 状态变迁历史。
    pub state_history: Vec<String>,
    /// 思考事件列表。
    pub think_events: Vec<String>,
    /// 用户确认事件列表。
    pub confirm_events: Vec<ConfirmEvent>,
}

/// 轨迹记录器。
#[derive(Clone, Default)]
pub struct TraceRecorder {
    inner: Arc<Mutex<TraceSnapshot>>,
}

impl TraceRecorder {
    /// 创建记录器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 绑定 Agent 事件回调。
    pub fn attach(&self, agent: &mut Agent) {
        let state_recorder = self.clone();
        agent.on_state_change(move |state| {
            state_recorder.record_state_label(state.label());
        });

        let think_recorder = self.clone();
        agent.on_think(move |text| {
            think_recorder.record_think(text);
        });

        let call_recorder = self.clone();
        agent.on_tool_call(move |name, args| {
            call_recorder.record_tool_call(name, args);
        });

        let result_recorder = self.clone();
        agent.on_tool_result(move |name, result| {
            result_recorder.record_tool_result(name, result);
        });
    }

    /// 记录状态标签。
    pub fn record_state_label(&self, label: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.state_history.push(label.to_owned());
        }
    }

    /// 记录思考事件。
    pub fn record_think(&self, think: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.think_events.push(think.to_owned());
        }
    }

    /// 记录工具调用。
    pub fn record_tool_call(&self, name: &str, args: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.tool_calls.push(ToolCallRecord {
                name: name.to_owned(),
                args: args.to_owned(),
                result: String::new(),
            });
        }
    }

    /// 填充最近一次同名工具调用的执行结果。
    pub fn record_tool_result(&self, name: &str, result: &str) {
        if let Ok(mut trace) = self.inner.lock()
            && let Some(record) = trace
                .tool_calls
                .iter_mut()
                .rev()
                .find(|record| record.name == name)
        {
            record.result.clone_from(&result.to_owned());
        }
    }

    /// 记录用户确认事件。
    pub fn record_confirm(&self, prompt: &str, approved: bool, mode: &str, source: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.confirm_events.push(ConfirmEvent {
                prompt: prompt.to_owned(),
                approved,
                mode: mode.to_owned(),
                source: source.to_owned(),
            });
        }
    }

    /// 导出轨迹快照。
    #[must_use]
    pub fn snapshot(&self) -> TraceSnapshot {
        self.inner
            .lock()
            .map_or_else(|_| TraceSnapshot::default(), |trace| trace.clone())
    }
}

/// 单轮对话记录。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TurnRecord {
    /// 对话轮次（从 1 开始）。
    pub turn_index: usize,
    /// 用户输入。
    pub user_message: String,
    /// 助手回复（失败时为空）。
    pub assistant_message: String,
    /// 错误信息（成功时为 `None`）。
    pub error: Option<String>,
    /// 该轮耗时（毫秒）。
    pub duration_ms: u64,
}

impl TurnRecord {
    /// 构造成功轮次记录。
    #[must_use]
    pub fn success(
        turn_index: usize,
        user_message: String,
        assistant_message: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            turn_index,
            user_message,
            assistant_message,
            error: None,
            duration_ms,
        }
    }

    /// 构造失败轮次记录。
    #[must_use]
    pub fn error(turn_index: usize, user_message: String, error: String, duration_ms: u64) -> Self {
        Self {
            turn_index,
            user_message,
            assistant_message: String::new(),
            error: Some(error),
            duration_ms,
        }
    }
}

/// 会话快照。
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    /// 会话 ID。
    pub id: String,
    /// 会话创建时间。
    pub created_at: String,
    /// 完整消息历史（可直接用于评估）。
    pub messages: Vec<ChatMessage>,
}

/// 会话运行统计。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionMetrics {
    /// 聚合 token 用量（基于 assistant 消息 usage 汇总）。
    pub token_usage: TokenUsage,
}

impl SessionSnapshot {
    /// 从 Session 生成快照。
    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            created_at: session.created_at.clone(),
            messages: session.messages().to_vec(),
        }
    }
}

/// 从会话消息聚合 token 用量。
#[must_use]
pub fn collect_session_metrics(messages: &[ChatMessage]) -> SessionMetrics {
    let token_usage = messages.iter().filter_map(|message| message.usage).sum();

    SessionMetrics { token_usage }
}

/// 确认模式稳定字符串。
#[must_use]
pub(crate) const fn confirm_mode_name(mode: ConfirmMode) -> &'static str {
    match mode {
        ConfirmMode::Deny => "deny",
        ConfirmMode::Allow => "allow",
        ConfirmMode::Ask => "ask",
    }
}

/// 构造 headless 模式的确认回调。
pub(crate) fn build_confirm_callback(
    mode: ConfirmMode,
    recorder: TraceRecorder,
) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |prompt: &str| {
        let (approved, source) = match mode {
            ConfirmMode::Allow => (true, CONFIRM_SOURCE_AUTO_ALLOW),
            ConfirmMode::Deny => (false, CONFIRM_SOURCE_AUTO_DENY),
            ConfirmMode::Ask => ask_user_confirm(prompt),
        };

        recorder.record_confirm(prompt, approved, confirm_mode_name(mode), source);
        approved
    })
}

/// 判断错误是否来自用户拒绝确认。
#[must_use]
pub(crate) fn is_user_denied_error(error: &str) -> bool {
    error.contains(MSG_USER_DENIED)
}

/// 打开默认会话服务；失败时按 CLI 约定输出错误并退出。
pub(crate) fn open_conversation_service_or_exit() -> ConversationService {
    match ConversationService::with_default_storage() {
        Ok(service) => service,
        Err(error) => exit_session_error(error),
    }
}

/// 处理 headless 会话管理请求。
///
/// 返回 `true` 表示已处理管理请求，调用方无需继续执行 chat。
pub(crate) fn handle_session_management_request(
    list_sessions: bool,
    archive_session: Option<&str>,
    output_format: OutputFormat,
    work_dir: &Path,
) -> bool {
    if !list_sessions && archive_session.is_none() {
        return false;
    }

    let service = open_conversation_service_or_exit();

    if list_sessions {
        match service.list_sessions(work_dir) {
            Ok(sessions) => print_session_list(output_format, &sessions),
            Err(error) => exit_session_error(error),
        }
    }

    if let Some(session_id) = archive_session {
        match service.archive_session(work_dir, session_id) {
            Ok(metadata) => print_archived_session(output_format, &metadata),
            Err(error) => exit_session_error(error),
        }
    }

    true
}

/// 按可选 session id 加载既有会话；未指定时创建新会话。
pub(crate) fn load_or_start_session(
    service: &ConversationService,
    work_dir: &Path,
    session_id: Option<&str>,
) -> secaudit_conversation::Result<ManagedSession> {
    if let Some(session_id) = session_id {
        service.load_session(work_dir, session_id)
    } else {
        service.start_session(work_dir)
    }
}

fn ask_user_confirm(prompt: &str) -> (bool, &'static str) {
    eprint!("{} {} [y/N] ", "[确认]".yellow().bold(), prompt);
    let _ = Write::flush(&mut io::stderr());

    let mut input = String::new();
    let approved = match io::stdin().read_line(&mut input) {
        Ok(_) => matches!(input.trim().to_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    };
    (approved, CONFIRM_SOURCE_STDIN_PROMPT)
}

fn exit_session_error(error: impl Display) -> ! {
    eprintln!("{}: {error}", "会话错误".red().bold());
    process::exit(1);
}

/// 解析 headless chat 输入消息。
///
/// 优先级：`--messages-json` > `--message` > stdin > 默认审计请求。
pub(crate) fn resolve_chat_messages<F>(
    messages_json: Option<&str>,
    message: Option<&str>,
    read_stdin: F,
) -> Vec<String>
where
    F: FnOnce() -> Option<String>,
{
    if let Some(messages_json) = messages_json
        && let Some(messages) = parse_message_list(messages_json)
    {
        return messages;
    }

    if let Some(message) = message {
        return vec![message.to_owned()];
    }

    let Some(stdin) = read_stdin() else {
        return default_chat_messages();
    };
    let trimmed = stdin.trim();
    if trimmed.is_empty() {
        return default_chat_messages();
    }

    parse_message_list(trimmed).unwrap_or_else(|| vec![trimmed.to_owned()])
}

fn parse_message_list(raw: &str) -> Option<Vec<String>> {
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(messages) => non_empty_messages(messages),
        Err(_) => Some(vec![raw.trim().to_owned()]),
    }
}

fn non_empty_messages(messages: Vec<String>) -> Option<Vec<String>> {
    let filtered = messages
        .into_iter()
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>();

    (!filtered.is_empty()).then_some(filtered)
}

fn default_chat_messages() -> Vec<String> {
    vec![DEFAULT_CHAT_MESSAGE.to_owned()]
}

/// 非交互模式输出。
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HeadlessResponse {
    /// 执行成功。
    Success {
        /// Agent 最终回复。
        final_message: String,
        /// 成功与失败响应共享的上下文。
        #[serde(flatten)]
        context: HeadlessResponseContext,
    },
    /// 执行失败。
    Error {
        /// 错误信息。
        error: String,
        /// 成功与失败响应共享的上下文。
        #[serde(flatten)]
        context: HeadlessResponseContext,
    },
}

/// 非交互响应的公共上下文。
#[derive(Debug, Serialize)]
pub struct HeadlessResponseContext {
    /// 用户输入与输出轮次。
    pub turns: Vec<TurnRecord>,
    /// 轨迹信息（状态、思考、工具、确认）。
    pub trace: TraceSnapshot,
    /// 会话快照。
    pub session: SessionSnapshot,
    /// 会话统计。
    pub metrics: SessionMetrics,
    /// 总耗时（毫秒）。
    pub duration_ms: u64,
    /// 工作目录。
    pub work_dir: String,
    /// 确认模式。
    pub confirm_mode: String,
    /// 会话管理信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_management: Option<SessionManagementInfo>,
}

impl HeadlessResponse {
    /// 构造成功响应。
    #[must_use]
    pub fn success(final_message: String, context: HeadlessResponseContext) -> Self {
        Self::Success {
            final_message,
            context,
        }
    }

    /// 构造失败响应。
    #[must_use]
    pub fn error(error: String, context: HeadlessResponseContext) -> Self {
        Self::Error { error, context }
    }
}

/// 按 CLI 输出格式打印会话列表。
pub(crate) fn print_session_list(output_format: OutputFormat, sessions: &[SessionMetadata]) {
    match output_format {
        OutputFormat::Json => {
            let output = SessionListOutput {
                status: "success",
                sessions,
            };
            print_json_or_exit(&output);
        }
        OutputFormat::Text => {
            println!("{}", "secaudit chat 会话列表".green().bold());
            if sessions.is_empty() {
                println!("当前项目没有历史会话。");
                return;
            }
            for session in sessions {
                println!(
                    "{}  {}  {}  messages={}  {}",
                    session.session_id,
                    session.status,
                    session.updated_at,
                    session.message_count,
                    session.title
                );
            }
        }
    }
}

/// 按 CLI 输出格式打印归档结果。
pub(crate) fn print_archived_session(output_format: OutputFormat, session: &SessionMetadata) {
    match output_format {
        OutputFormat::Json => {
            let output = ArchiveSessionOutput {
                status: "success",
                session,
            };
            print_json_or_exit(&output);
        }
        OutputFormat::Text => {
            println!("{}", "secaudit chat 会话已归档".green().bold());
            println!("会话：{}", session.session_id);
            println!("状态：{}", session.status);
            println!("更新时间：{}", session.updated_at);
        }
    }
}

/// 按 CLI 输出格式打印 headless chat 响应。
pub(crate) fn print_response(output_format: OutputFormat, response: &HeadlessResponse) {
    match output_format {
        OutputFormat::Json => match serde_json::to_string_pretty(response) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("{}: 非交互结果序列化失败：{e}", "错误".red().bold());
                process::exit(1);
            }
        },
        OutputFormat::Text => match response {
            HeadlessResponse::Success {
                final_message,
                context,
            } => {
                println!("{}", "secaudit chat 执行成功".green().bold());
                println!("工作目录：{}", context.work_dir);
                println!("确认模式：{}", context.confirm_mode);
                println!("轮次：{}", context.turns.len());
                println!("耗时：{} ms", context.duration_ms);
                if let Some(info) = &context.session_management {
                    println!("会话：{} ({})", info.project_key, info.status);
                    println!("会话文件：{}", info.session_path);
                }
                output::cli::print_separator();
                println!("{final_message}");
            }
            HeadlessResponse::Error { error, context } => {
                eprintln!("{}", "secaudit chat 执行失败".red().bold());
                eprintln!("工作目录：{}", context.work_dir);
                eprintln!("确认模式：{}", context.confirm_mode);
                eprintln!("轮次：{}", context.turns.len());
                eprintln!("耗时：{} ms", context.duration_ms);
                if let Some(info) = &context.session_management {
                    eprintln!("会话：{} ({})", info.project_key, info.status);
                    eprintln!("会话文件：{}", info.session_path);
                }
                eprintln!("错误：{error}");
            }
        },
    }
}

fn print_json_or_exit<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("{}: JSON 序列化失败：{error}", "错误".red().bold());
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HeadlessResponse, HeadlessResponseContext, SessionMetrics, SessionSnapshot, ToolCallRecord,
        TraceRecorder, TraceSnapshot, TurnRecord, resolve_chat_messages,
    };
    use secaudit_agent::TokenUsage;

    #[test]
    fn recorder_tracks_state_and_tool_trace() {
        let recorder = TraceRecorder::new();

        recorder.record_state_label("执行中");
        recorder.record_think("准备调用工具");
        recorder.record_tool_call("read_file", "{\"path\":\"src/main.rs\"}");
        recorder.record_tool_result("read_file", "ok");
        recorder.record_confirm("允许执行命令吗", true, "ask", "stdin_prompt");

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.state_history, vec!["执行中"]);
        assert_eq!(snapshot.think_events, vec!["准备调用工具"]);
        assert_eq!(snapshot.tool_calls.len(), 1);
        assert_eq!(snapshot.tool_calls[0].name, "read_file");
        assert_eq!(snapshot.tool_calls[0].result, "ok");
        assert_eq!(snapshot.confirm_events.len(), 1);
        assert!(snapshot.confirm_events[0].approved);
    }

    #[test]
    fn recorder_ignores_result_without_call() {
        let recorder = TraceRecorder::new();
        recorder.record_tool_result("missing", "ignored");

        let snapshot = recorder.snapshot();
        assert!(snapshot.tool_calls.is_empty());
    }

    #[test]
    fn response_serialization_contains_status_tag() {
        let response = HeadlessResponse::success(
            "done".to_owned(),
            HeadlessResponseContext {
                turns: vec![TurnRecord::success(1, "hi".to_owned(), "ok".to_owned(), 10)],
                trace: TraceSnapshot {
                    tool_calls: vec![ToolCallRecord {
                        name: "list_directory".to_owned(),
                        args: "{}".to_owned(),
                        result: "[]".to_owned(),
                    }],
                    state_history: vec!["规划".to_owned()],
                    think_events: vec!["思考".to_owned()],
                    confirm_events: Vec::new(),
                },
                session: SessionSnapshot {
                    id: "session-id".to_owned(),
                    created_at: "2026-04-26T00:00:00Z".to_owned(),
                    messages: Vec::new(),
                },
                metrics: SessionMetrics {
                    token_usage: TokenUsage::default(),
                },
                duration_ms: 10,
                work_dir: "/tmp/project".to_owned(),
                confirm_mode: "deny".to_owned(),
                session_management: None,
            },
        );

        let json = serde_json::to_value(response).ok();
        assert!(json.is_some());
        if let Some(value) = json {
            assert_eq!(
                value.get("status").and_then(serde_json::Value::as_str),
                Some("success")
            );
            assert_eq!(
                value
                    .get("final_message")
                    .and_then(serde_json::Value::as_str),
                Some("done")
            );
            assert!(value.get("turns").is_some());
            assert!(value.get("trace").is_some());
            assert!(value.get("session").is_some());
            assert!(value.get("metrics").is_some());
            assert!(value.get("session_management").is_none());
        }
    }

    #[test]
    fn resolve_messages_prefers_json_list() {
        let messages =
            resolve_chat_messages(Some("[\"a\",\"b\"]"), None, || Some("ignored".to_owned()));

        assert_eq!(messages, vec!["a", "b"]);
    }

    #[test]
    fn resolve_messages_falls_back_to_raw_invalid_json_arg() {
        let messages = resolve_chat_messages(Some("not json"), None, || None);

        assert_eq!(messages, vec!["not json"]);
    }

    #[test]
    fn resolve_messages_reads_stdin_json_when_no_arg_exists() {
        let messages = resolve_chat_messages(None, None, || {
            Some("[\" first \",\"\", \"second\"]".to_owned())
        });

        assert_eq!(messages, vec!["first", "second"]);
    }
}
