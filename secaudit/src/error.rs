use std::io;

/// 统一错误类型，覆盖应用各层可能出现的错误场景。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 配置相关错误（缺失字段、格式不合法等）
    #[error("配置错误：{0}")]
    Config(String),

    /// LLM 调用相关错误（网络、API 响应异常等）
    #[error("LLM 错误：{0}")]
    Llm(String),

    /// 工具执行相关错误
    #[error("工具错误：{0}")]
    Tool(String),

    /// 解析相关错误（JSON、正则等）
    #[error("解析错误：{0}")]
    Parse(String),

    /// IO 相关错误
    #[error("IO 错误：{0}")]
    Io(#[from] io::Error),
}

impl From<llm_common::Error> for Error {
    fn from(e: llm_common::Error) -> Self {
        Self::Llm(e.to_string())
    }
}
