// LLM 模块：重导出 secaudit-llm 统一类型，提供 secaudit 专用的客户端构造

pub use secaudit_llm::{
    ChatMessage, HttpLlmClient, LlmConfig, Role, ToolCallResponse, ToolDefinition,
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
