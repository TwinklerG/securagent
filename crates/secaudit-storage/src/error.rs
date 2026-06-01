use std::io;
use std::path::PathBuf;
use std::result;

/// 持久化层错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 无法推导默认存储根目录。
    #[error("无法推导默认持久化根目录")]
    MissingHome,

    /// JSON 序列化或解析失败。
    #[error("JSON 处理失败：{0}")]
    Json(#[from] serde_json::Error),

    /// IO 失败。
    #[error("IO 错误：{0}")]
    Io(#[from] io::Error),

    /// 已存在同名路径，不能安全写入。
    #[error("目标路径已存在且不能安全覆盖：{path}")]
    PathConflict { path: PathBuf },
}

/// crate 内统一 Result 类型。
pub type Result<T> = result::Result<T, Error>;
