use std::io;
use std::result;

/// 记忆存储错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// IO 失败。
    #[error("IO 错误：{0}")]
    Io(#[from] io::Error),

    /// JSON 序列化或解析失败。
    #[error("JSON 处理失败：{0}")]
    Json(#[from] serde_json::Error),
}

/// crate 内统一 Result 类型。
pub type Result<T> = result::Result<T, Error>;
