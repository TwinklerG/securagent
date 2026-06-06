//! headless 模式输出响应模型。

use secaudit_agent::{ChatMessage, Session, TokenUsage};
use secaudit_conversation::SessionManagementInfo;
use serde::Serialize;

use super::trace::TraceSnapshot;

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

/// 会话运行统计。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionMetrics {
    /// 聚合 token 用量（基于 assistant 消息 usage 汇总）。
    pub token_usage: TokenUsage,
}

/// 从会话消息聚合 token 用量。
#[must_use]
pub fn collect_session_metrics(messages: &[ChatMessage]) -> SessionMetrics {
    let token_usage = TokenUsage::sum_from_messages(messages);

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

#[cfg(test)]
mod tests {
    use secaudit_agent::TokenUsage;

    use super::{
        HeadlessResponse, HeadlessResponseContext, SessionMetrics, SessionSnapshot, TurnRecord,
    };
    use crate::headless::trace::{ToolCallRecord, TraceSnapshot};

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
}
