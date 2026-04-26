//! 非交互调试模式：记录轨迹并输出结构化 JSON。

use std::sync::{Arc, Mutex};

use secaudit_agent::{Agent, ChatMessage, Session, TokenUsage};
use serde::Serialize;

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
    let token_usage = messages.iter().filter_map(|message| message.usage).fold(
        TokenUsage::default(),
        |mut acc, usage| {
            acc.add_assign(&usage);
            acc
        },
    );

    SessionMetrics { token_usage }
}

/// 非交互模式输出。
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HeadlessResponse {
    /// 执行成功。
    Success {
        /// Agent 最终回复。
        final_message: String,
        /// 用户输入与输出轮次。
        turns: Vec<TurnRecord>,
        /// 轨迹信息（状态、思考、工具、确认）。
        trace: TraceSnapshot,
        /// 会话快照。
        session: SessionSnapshot,
        /// 会话统计。
        metrics: SessionMetrics,
        /// 总耗时（毫秒）。
        duration_ms: u64,
        /// 工作目录。
        work_dir: String,
        /// 确认模式。
        confirm_mode: String,
    },
    /// 执行失败。
    Error {
        /// 错误信息。
        error: String,
        /// 已执行轮次。
        turns: Vec<TurnRecord>,
        /// 轨迹信息（状态、思考、工具、确认）。
        trace: TraceSnapshot,
        /// 会话快照。
        session: SessionSnapshot,
        /// 会话统计。
        metrics: SessionMetrics,
        /// 总耗时（毫秒）。
        duration_ms: u64,
        /// 工作目录。
        work_dir: String,
        /// 确认模式。
        confirm_mode: String,
    },
}

/// 非交互响应的公共上下文。
#[derive(Debug)]
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
}

impl HeadlessResponse {
    /// 构造成功响应。
    #[must_use]
    pub fn success(final_message: String, context: HeadlessResponseContext) -> Self {
        let HeadlessResponseContext {
            turns,
            trace,
            session,
            metrics,
            duration_ms,
            work_dir,
            confirm_mode,
        } = context;
        Self::Success {
            final_message,
            turns,
            trace,
            session,
            metrics,
            duration_ms,
            work_dir,
            confirm_mode,
        }
    }

    /// 构造失败响应。
    #[must_use]
    pub fn error(error: String, context: HeadlessResponseContext) -> Self {
        let HeadlessResponseContext {
            turns,
            trace,
            session,
            metrics,
            duration_ms,
            work_dir,
            confirm_mode,
        } = context;
        Self::Error {
            error,
            turns,
            trace,
            session,
            metrics,
            duration_ms,
            work_dir,
            confirm_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HeadlessResponse, HeadlessResponseContext, SessionMetrics, SessionSnapshot, ToolCallRecord,
        TraceRecorder, TraceSnapshot, TurnRecord,
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
            },
        );

        let json = serde_json::to_value(response);
        assert!(json.is_ok());
        if let Ok(value) = json {
            assert_eq!(
                value.get("status").and_then(serde_json::Value::as_str),
                Some("success")
            );
        }
    }
}
