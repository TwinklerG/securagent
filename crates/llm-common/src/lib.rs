//! **llm-common** — 通用 LLM 客户端，供 workspace 内多个 crate 复用。
//!
//! 基于 `async-openai` 封装，兼容 `OpenAI` Chat Completions API。

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
};
use async_trait::async_trait;
use ragrs::{Error, LlmClient};
use serde::Deserialize;

// ── 配置 ─────────────────────────────────────────────────────────────────────

/// LLM 服务配置。
#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    /// API 基础地址（OpenAI 兼容）
    pub api_base_url: String,
    /// API 密钥
    pub api_key: String,
    /// 模型名称
    pub model: String,
}

// ── 常量 ─────────────────────────────────────────────────────────────────────

/// LLM-as-Judge 评估时使用的低温参数
const JUDGE_TEMPERATURE: f32 = 0.0;

// ── 客户端 ───────────────────────────────────────────────────────────────────

/// 基于 `async-openai` 的 LLM 客户端。
///
/// 实现 ragrs 的 [`LlmClient`] trait，可直接用于评估指标的 LLM-as-Judge 调用。
pub struct HttpLlmClient {
    /// async-openai 客户端
    client: async_openai::Client<OpenAIConfig>,
    /// 模型名称
    model: String,
}

impl HttpLlmClient {
    /// 根据配置创建客户端实例。
    #[must_use]
    pub fn new(config: &LlmConfig) -> Self {
        let openai_config = OpenAIConfig::new()
            .with_api_base(&config.api_base_url)
            .with_api_key(&config.api_key);

        Self {
            client: async_openai::Client::with_config(openai_config),
            model: config.model.clone(),
        }
    }
}

#[async_trait]
impl LlmClient for HttpLlmClient {
    async fn generate(&self, prompt: &str) -> Result<String, Error> {
        let message = ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
            .build()
            .map_err(|e| Error::Llm(format!("消息构建失败：{e}")))?;

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .temperature(JUDGE_TEMPERATURE)
            .messages(vec![message.into()])
            .build()
            .map_err(|e| Error::Llm(format!("请求构建失败：{e}")))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| Error::Llm(format!("API 调用失败：{e}")))?;

        response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| Error::Llm("API 返回空 choices".into()))
    }
}
