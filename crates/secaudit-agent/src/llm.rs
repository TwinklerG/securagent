// LLM 模块：重导出 secaudit-llm 统一类型，提供 secaudit 专用的客户端构造

pub use secaudit_llm::{
    ChatMessage, HttpLlmClient, LlmConfig, Role, TokenUsage, ToolCallResponse, ToolDefinition,
};

use crate::config::Config;

/// 从 secaudit 应用配置创建 LLM 客户端实例。
#[must_use]
pub fn create_client(config: &Config) -> HttpLlmClient {
    HttpLlmClient::new(&LlmConfig {
        api_base_url: config.api_base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
    })
}

/// 运行时向服务商查询当前模型的上下文窗口 token 数。
///
/// 委托给 [`secaudit_llm::fetch_model_context_window`]；服务商未暴露该字段或
/// 请求失败时返回 `None`，调用方应回退到配置/默认窗口。
pub async fn fetch_context_window(config: &Config) -> Option<u64> {
    secaudit_llm::fetch_model_context_window(&LlmConfig {
        api_base_url: config.api_base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
    })
    .await
}
