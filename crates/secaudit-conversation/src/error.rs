use std::io;

use secaudit_agent::error::Error as AgentError;
use std::result;

/// 会话持久化与历史管理错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 底层存储错误（`MissingHome` / `PathConflict` 等）。
    #[error(transparent)]
    Storage(#[from] secaudit_storage::Error),

    /// 会话 ID 不合法。
    #[error("会话 ID 不合法：{session_id}")]
    InvalidSessionId { session_id: String },

    /// 会话不存在。
    #[error("会话不存在：{session_id}")]
    SessionNotFound { session_id: String },

    /// 会话状态不允许执行当前操作。
    #[error("会话 {session_id} 当前状态为 {status}，不能执行该操作")]
    InvalidSessionStatus { session_id: String, status: String },

    /// 空会话不应持久化。
    #[error("空会话不会持久化：{session_id}")]
    EmptySession { session_id: String },

    /// JSON 序列化或解析失败。
    #[error("JSON 处理失败：{0}")]
    Json(#[from] serde_json::Error),

    /// Agent 推理失败。
    #[error("Agent 执行失败：{0}")]
    Agent(#[from] AgentError),

    /// IO 失败。
    #[error("IO 错误：{0}")]
    Io(#[from] io::Error),
}

/// crate 内统一 Result 类型。
pub type Result<T> = result::Result<T, Error>;
