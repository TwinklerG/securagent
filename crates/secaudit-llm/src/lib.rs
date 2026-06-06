//! **secaudit-llm** — 通用 LLM 客户端与对话类型，供 workspace 内多个 crate 复用。
//!
//! 基于 `async-openai` 封装，兼容 `OpenAI` Chat Completions API。
//! 提供两种调用模式：
//! - `generate`：简单单轮文本生成
//! - `chat`：多轮对话 + 工具调用（secaudit Agent 使用）

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCallChunk,
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
    ChatCompletionStreamOptions, ChatCompletionStreamResponseDelta, ChatCompletionTool,
    ChatCompletionTools, CreateChatCompletionRequestArgs, CreateChatCompletionStreamResponse,
    FunctionCall as OpenAIFunctionCall, FunctionObjectArgs,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::iter::Sum;
use std::ops::AddAssign;
use std::time::Duration;

/// secaudit-llm 错误类型。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// LLM 调用相关错误（网络、API 响应异常等）
    #[error("LLM 错误：{0}")]
    Llm(String),
}

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

/// 拉取模型元数据的请求超时。
const MODEL_METADATA_TIMEOUT: Duration = Duration::from_secs(5);
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// 不同 `OpenAI` 兼容服务商暴露上下文窗口所用的字段名（按优先级）。
const CONTEXT_WINDOW_FIELDS: [&str; 6] = [
    "context_length",
    "context_window",
    "max_context_length",
    "max_context_window_tokens",
    "max_input_tokens",
    "max_tokens",
];

/// 运行时查询当前模型的上下文窗口 token 数。
pub async fn fetch_model_context_window(config: &LlmConfig) -> Option<u64> {
    let client = reqwest::Client::builder()
        .timeout(MODEL_METADATA_TIMEOUT)
        .user_agent("Go-http-client/1.1")
        .build()
        .ok()?;

    let value = fetch_model_metadata(&client, OPENROUTER_MODELS_URL).await?;
    extract_context_window(&value, &config.model)
}

async fn fetch_model_metadata(client: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// 从 `/models` 响应中解析指定模型的上下文窗口。纯函数，便于单测。
fn extract_context_window(value: &serde_json::Value, model: &str) -> Option<u64> {
    // 列表结构：优先取 id == model 的条目；只有一条时直接采用。
    if let Some(items) = value.get("data").and_then(serde_json::Value::as_array) {
        let entry = find_model_entry(items, model).or_else(|| {
            if items.len() == 1 {
                items.first()
            } else {
                None
            }
        })?;
        return window_from_object(entry);
    }
    // 单对象结构（/models/{id}）。
    window_from_object(value)
}

fn find_model_entry<'a>(
    items: &'a [serde_json::Value],
    model: &str,
) -> Option<&'a serde_json::Value> {
    items.iter().find(|item| model_entry_matches(item, model))
}

fn model_entry_matches(item: &serde_json::Value, model: &str) -> bool {
    ["id", "canonical_slug"].iter().any(|field| {
        item.get(*field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|candidate| model_name_matches(candidate, model))
    })
}

fn model_name_matches(candidate: &str, model: &str) -> bool {
    let candidate = candidate.trim();
    let model = model.trim();
    if candidate.eq_ignore_ascii_case(model) {
        return true;
    }
    let candidate_slug = candidate.rsplit('/').next().unwrap_or(candidate);
    let model_slug = model.rsplit('/').next().unwrap_or(model);
    candidate_slug.eq_ignore_ascii_case(model_slug)
}

/// 从单个模型对象里取窗口大小。OpenRouter 等会嵌套在 `top_provider.context_length`。
fn window_from_object(object: &serde_json::Value) -> Option<u64> {
    let nested = object
        .get("top_provider")
        .and_then(|provider| provider.get("context_length"))
        .and_then(value_as_u64);
    if nested.is_some() {
        return nested;
    }
    CONTEXT_WINDOW_FIELDS
        .iter()
        .find_map(|field| object.get(*field).and_then(value_as_u64))
}

/// 字段可能是数字或数字字符串，统一解析为正的 `u64`。
fn value_as_u64(value: &serde_json::Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return (number > 0).then_some(number);
    }
    value
        .as_str()
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|number| *number > 0)
}

#[cfg(test)]
mod context_window_tests {
    use super::extract_context_window;

    #[test]
    fn prefers_nested_top_provider_window() {
        let body = serde_json::json!({
            "data": [
                {"id": "other", "context_length": 8_000},
                {"id": "target", "context_length": 64_000,
                 "top_provider": {"context_length": 128_000}}
            ]
        });
        assert_eq!(extract_context_window(&body, "target"), Some(128_000));
    }

    #[test]
    fn parses_flat_context_length_field() {
        let body = serde_json::json!({"data": [{"id": "m", "context_length": 45_000}]});
        assert_eq!(extract_context_window(&body, "m"), Some(45_000));
    }

    #[test]
    fn parses_single_object_endpoint() {
        let body = serde_json::json!({"id": "m", "max_input_tokens": "200000"});
        assert_eq!(extract_context_window(&body, "m"), Some(200_000));
    }

    #[test]
    fn matches_openrouter_slug_suffix() {
        let body = serde_json::json!({
            "data": [{
                "id": "deepseek/deepseek-v3.2",
                "canonical_slug": "deepseek/deepseek-v3.2",
                "context_length": 1_000_000
            }]
        });

        assert_eq!(
            extract_context_window(&body, "deepseek-v3.2"),
            Some(1_000_000)
        );
    }

    #[test]
    fn matches_openrouter_slug_when_provider_prefix_differs() {
        let body = serde_json::json!({
            "data": [{
                "id": "moonshotai/kimi-k2.6",
                "canonical_slug": "moonshotai/kimi-k2.6",
                "context_length": 262_000
            }]
        });

        assert_eq!(
            extract_context_window(&body, "kimi/kimi-k2.6"),
            Some(262_000)
        );
    }

    #[test]
    fn returns_none_when_provider_omits_window() {
        // DeepSeek / OpenAI 形态：只有 id/object/owned_by。
        let body = serde_json::json!({
            "data": [{"id": "deepseek-chat", "object": "model", "owned_by": "deepseek"}]
        });
        assert_eq!(extract_context_window(&body, "deepseek-chat"), None);
    }
}

/// Token 用量统计。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    /// 提示词 token 数
    pub prompt_tokens: u64,
    /// 生成 token 数
    pub completion_tokens: u64,
    /// 总 token 数
    pub total_tokens: u64,
}

impl TokenUsage {
    /// 是否为零用量。
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0 && self.total_tokens == 0
    }

    /// 累加切片中所有消息的 token 用量。通常只有 assistant 消息携带 `usage`。
    #[must_use]
    pub fn sum_from_messages(messages: &[ChatMessage]) -> Self {
        messages.iter().filter_map(|message| message.usage).fold(
            Self::default(),
            |mut acc, usage| {
                acc.add_assign(usage);
                acc
            },
        )
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.prompt_tokens += rhs.prompt_tokens;
        self.completion_tokens += rhs.completion_tokens;
        self.total_tokens += rhs.total_tokens;
    }
}

impl Sum for TokenUsage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |mut acc, usage| {
            acc += usage;
            acc
        })
    }
}

// ── 常量 ─────────────────────────────────────────────────────────────────────

/// LLM-as-Judge 评估时使用的低温参数
const JUDGE_TEMPERATURE: f32 = 0.0;

// ── 核心对话类型 ─────────────────────────────────────────────────────────────

/// LLM 对话消息角色
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 系统指令
    System,
    /// 用户输入
    User,
    /// 模型回复
    Assistant,
    /// 工具执行结果
    Tool,
}

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息角色
    pub role: Role,
    /// 消息文本内容
    pub content: Option<String>,
    /// 模型返回的工具调用列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallResponse>>,
    /// 工具调用结果对应的调用 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 本条消息对应的 token 用量（通常仅 assistant 消息有值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

impl ChatMessage {
    /// 创建系统消息
    #[must_use]
    pub fn system<S: Into<String>>(content: S) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        }
    }

    /// 创建用户消息
    #[must_use]
    pub fn user<S: Into<String>>(content: S) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        }
    }

    /// 创建工具结果消息
    #[must_use]
    pub fn tool_result<S: Into<String>>(tool_call_id: S, content: S) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            usage: None,
        }
    }
}

/// LLM 返回的工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResponse {
    /// 调用唯一标识
    pub id: String,
    /// 调用类型（通常为 `"function"`）
    pub r#type: String,
    /// 函数调用详情
    pub function: FunctionCall,
}

/// 函数调用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// 函数名称
    pub name: String,
    /// 参数 JSON 字符串
    pub arguments: String,
}

/// 工具定义（发送给 LLM 的工具描述）
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    /// 函数名称
    pub name: String,
    /// 函数描述
    pub description: String,
    /// 参数 JSON Schema
    pub parameters: serde_json::Value,
}

// ── 流式响应聚合器 ──────────────────────────────────────────────────────────

/// 流式 `tool_call` 分片聚合器（按 `index` 拼接多个 `chunk`）。
#[derive(Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAccumulator {
    fn ingest_chunk(&mut self, chunk: ChatCompletionMessageToolCallChunk) {
        if let Some(id) = chunk.id {
            self.id = Some(id);
        }
        if let Some(function) = chunk.function {
            if let Some(name) = function.name {
                self.name = Some(name);
            }
            if let Some(arguments) = function.arguments {
                self.arguments.push_str(&arguments);
            }
        }
    }

    fn into_response(self) -> ToolCallResponse {
        ToolCallResponse {
            id: self.id.unwrap_or_default(),
            r#type: "function".into(),
            function: FunctionCall {
                name: self.name.unwrap_or_default(),
                arguments: self.arguments,
            },
        }
    }
}

/// 把 `OpenAI` 流式 chunk 序列装配回 `chat` 等价的字段。
///
/// 单一职责：消费 `CreateChatCompletionStreamResponse` 序列，按 `index` 拼接
/// `tool_call` 分片、累加文本与 `usage`，对外仅暴露 `ingest`/`finish`。
#[derive(Default)]
struct StreamAggregator {
    content: String,
    tool_calls: BTreeMap<u32, ToolCallAccumulator>,
    usage: Option<TokenUsage>,
}

struct StreamUpdate {
    delta: Option<String>,
    usage: Option<TokenUsage>,
}

impl StreamAggregator {
    fn new() -> Self {
        Self::default()
    }

    /// 消费一个 chunk，返回文本增量和真实 usage（若服务商在本 chunk 提供）。
    fn ingest(&mut self, chunk: CreateChatCompletionStreamResponse) -> StreamUpdate {
        let usage = self.ingest_usage(&chunk);
        let delta = chunk
            .choices
            .into_iter()
            .next()
            .and_then(|choice| self.ingest_delta(choice.delta));

        StreamUpdate { delta, usage }
    }

    fn ingest_usage(&mut self, chunk: &CreateChatCompletionStreamResponse) -> Option<TokenUsage> {
        let usage = chunk.usage.as_ref().map(|chunk_usage| TokenUsage {
            prompt_tokens: chunk_usage.prompt_tokens.into(),
            completion_tokens: chunk_usage.completion_tokens.into(),
            total_tokens: chunk_usage.total_tokens.into(),
        });
        if let Some(usage) = usage {
            self.usage = Some(usage);
        }
        usage
    }

    fn ingest_delta(&mut self, delta: ChatCompletionStreamResponseDelta) -> Option<String> {
        if let Some(tool_call_chunks) = delta.tool_calls {
            self.ingest_tool_call_chunks(tool_call_chunks);
        }

        self.ingest_text_delta(delta.content)
    }

    fn ingest_tool_call_chunks(&mut self, chunks: Vec<ChatCompletionMessageToolCallChunk>) {
        for chunk in chunks {
            self.tool_calls
                .entry(chunk.index)
                .or_default()
                .ingest_chunk(chunk);
        }
    }

    fn ingest_text_delta(&mut self, text: Option<String>) -> Option<String> {
        if let Some(text) = text
            && !text.is_empty()
        {
            self.content.push_str(&text);
            return Some(text);
        }

        None
    }

    /// 流结束后产出装配好的 assistant `ChatMessage`。
    fn finish(self) -> ChatMessage {
        let content = if self.content.is_empty() {
            None
        } else {
            Some(self.content)
        };
        let tool_calls = if self.tool_calls.is_empty() {
            None
        } else {
            Some(
                self.tool_calls
                    .into_values()
                    .map(ToolCallAccumulator::into_response)
                    .collect(),
            )
        };
        ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            usage: self.usage,
        }
    }
}

// ── 类型转换（内部） ────────────────────────────────────────────────────────

/// 将内部 `ChatMessage` 转换为 `async-openai` 请求消息类型。
impl TryFrom<&ChatMessage> for ChatCompletionRequestMessage {
    type Error = Error;

    fn try_from(msg: &ChatMessage) -> Result<Self, Self::Error> {
        match msg.role {
            Role::System => {
                let content = msg.content.as_deref().unwrap_or_default();
                let m = ChatCompletionRequestSystemMessageArgs::default()
                    .content(content)
                    .build()
                    .map_err(|e| Error::Llm(format!("系统消息构建失败：{e}")))?;
                Ok(m.into())
            }
            Role::User => {
                let content = msg.content.as_deref().unwrap_or_default();
                let m = ChatCompletionRequestUserMessageArgs::default()
                    .content(content)
                    .build()
                    .map_err(|e| Error::Llm(format!("用户消息构建失败：{e}")))?;
                Ok(m.into())
            }
            Role::Assistant => {
                let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
                if let Some(content) = &msg.content {
                    builder.content(content.as_str());
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    let calls: Vec<ChatCompletionMessageToolCalls> = tool_calls
                        .iter()
                        .map(|tc| {
                            ChatCompletionMessageToolCalls::Function(
                                ChatCompletionMessageToolCall {
                                    id: tc.id.clone(),
                                    function: OpenAIFunctionCall {
                                        name: tc.function.name.clone(),
                                        arguments: tc.function.arguments.clone(),
                                    },
                                },
                            )
                        })
                        .collect();
                    builder.tool_calls(calls);
                }
                let m = builder
                    .build()
                    .map_err(|e| Error::Llm(format!("助手消息构建失败：{e}")))?;
                Ok(m.into())
            }
            Role::Tool => {
                let content = msg.content.as_deref().unwrap_or_default();
                let tool_call_id = msg.tool_call_id.as_deref().unwrap_or_default();
                let m = ChatCompletionRequestToolMessageArgs::default()
                    .content(content)
                    .tool_call_id(tool_call_id)
                    .build()
                    .map_err(|e| Error::Llm(format!("工具消息构建失败：{e}")))?;
                Ok(m.into())
            }
        }
    }
}

/// 将 `ToolDefinition` 转换为 `async-openai` 工具类型。
impl TryFrom<&ToolDefinition> for ChatCompletionTools {
    type Error = Error;

    fn try_from(def: &ToolDefinition) -> Result<Self, Self::Error> {
        let function = FunctionObjectArgs::default()
            .name(&def.name)
            .description(&def.description)
            .parameters(def.parameters.clone())
            .build()
            .map_err(|e| Error::Llm(format!("工具定义构建失败：{e}")))?;

        Ok(Self::Function(ChatCompletionTool { function }))
    }
}

impl From<&ChatCompletionMessageToolCalls> for ToolCallResponse {
    fn from(tc: &ChatCompletionMessageToolCalls) -> Self {
        match tc {
            ChatCompletionMessageToolCalls::Function(tc) => Self {
                id: tc.id.clone(),
                r#type: "function".into(),
                function: FunctionCall {
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                },
            },
            ChatCompletionMessageToolCalls::Custom(tc) => Self {
                id: tc.id.clone(),
                r#type: "custom".into(),
                function: FunctionCall {
                    name: tc.custom_tool.name.clone(),
                    arguments: tc.custom_tool.input.clone(),
                },
            },
        }
    }
}

/// 将 `async-openai` 工具调用转换为内部类型。
fn from_tool_calls(calls: &[ChatCompletionMessageToolCalls]) -> Vec<ToolCallResponse> {
    calls.iter().map(ToolCallResponse::from).collect()
}

// ── 客户端 ───────────────────────────────────────────────────────────────────

/// 基于 `async-openai` 的 LLM 客户端。
///
/// 同时支持：
/// - `generate`（单轮文本生成）
/// - 多轮对话 + 工具调用（`chat`，用于 Agent 交互）
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

        let openai_config = openai_config
            .clone()
            .with_header("user-agent", "Go-http-client/1.1")
            .unwrap_or(openai_config);

        Self {
            client: async_openai::Client::with_config(openai_config),
            model: config.model.clone(),
        }
    }

    /// 发送多轮对话请求，支持工具定义。
    ///
    /// # Errors
    ///
    /// 请求构建、API 调用或响应解析失败时返回 [`Error::Llm`]。
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatMessage, Error> {
        // 转换消息列表
        let request_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(ChatCompletionRequestMessage::try_from)
            .collect::<Result<_, _>>()?;

        // 构建请求
        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(&self.model).messages(request_messages);

        if let Some(tool_defs) = tools
            && !tool_defs.is_empty()
        {
            let chat_tools: Vec<ChatCompletionTools> = tool_defs
                .iter()
                .map(ChatCompletionTools::try_from)
                .collect::<Result<_, _>>()?;
            builder.tools(chat_tools);
        }

        let request = builder
            .build()
            .map_err(|e| Error::Llm(format!("请求构建失败：{e}")))?;

        // 发送请求
        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| Error::Llm(format!("API 调用失败：{e}")))?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::Llm("响应中无有效选项".into()))?;

        let message = choice.message;

        // 转换为内部类型
        let tool_calls = message.tool_calls.as_ref().map(|tc| from_tool_calls(tc));
        let usage = response.usage.as_ref().map(|usage| TokenUsage {
            prompt_tokens: usage.prompt_tokens.into(),
            completion_tokens: usage.completion_tokens.into(),
            total_tokens: usage.total_tokens.into(),
        });

        Ok(ChatMessage {
            role: Role::Assistant,
            content: message.content,
            tool_calls,
            tool_call_id: None,
            usage,
        })
    }

    /// 流式发送多轮对话请求。
    ///
    /// 收到每个文本增量片段时调用 `on_delta` 回调（仅 `content` 增量，不包括 `tool_calls`）。
    /// 流结束后聚合 `content` / `tool_calls` / `usage` 组装出与 [`Self::chat`] 等价的
    /// `ChatMessage` 返回，便于上层 `ReAct` 循环复用现有逻辑。
    ///
    /// # Errors
    ///
    /// 请求构建、API 调用或响应解析失败时返回 [`Error::Llm`]。
    pub async fn chat_stream<F, U>(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
        mut on_delta: F,
        mut on_usage: U,
    ) -> Result<ChatMessage, Error>
    where
        F: FnMut(&str) + Send,
        U: FnMut(TokenUsage) + Send,
    {
        let request_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(ChatCompletionRequestMessage::try_from)
            .collect::<Result<_, _>>()?;

        // 兼容 0.34+ 多字段结构（include_usage / include_obfuscation），
        // 用 JSON 反序列化避免直接字面量在不同版本下字段数差异引发编译/lint 问题。
        let stream_opts: ChatCompletionStreamOptions =
            serde_json::from_str(r#"{"include_usage":true}"#)
                .map_err(|e| Error::Llm(format!("stream_options 构建失败：{e}")))?;

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder
            .model(&self.model)
            .messages(request_messages)
            .stream(true)
            .stream_options(stream_opts);

        if let Some(tool_defs) = tools
            && !tool_defs.is_empty()
        {
            let chat_tools: Vec<ChatCompletionTools> = tool_defs
                .iter()
                .map(ChatCompletionTools::try_from)
                .collect::<Result<_, _>>()?;
            builder.tools(chat_tools);
        }

        let request = builder
            .build()
            .map_err(|e| Error::Llm(format!("请求构建失败：{e}")))?;

        let mut stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| Error::Llm(format!("API 调用失败：{e}")))?;

        let mut aggregator = StreamAggregator::new();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| Error::Llm(format!("流式响应读取失败：{e}")))?;
            let update = aggregator.ingest(chunk);
            if let Some(usage) = update.usage {
                on_usage(usage);
            }
            if let Some(delta) = update.delta {
                on_delta(&delta);
            }
        }

        Ok(aggregator.finish())
    }

    /// 发送单轮生成请求，返回模型输出文本。
    ///
    /// # Errors
    ///
    /// 请求构建、API 调用或响应解析失败时返回 [`Error::Llm`]。
    pub async fn generate(&self, prompt: &str) -> Result<String, Error> {
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

#[cfg(test)]
mod tests {
    use async_openai::types::chat::CreateChatCompletionStreamResponse;
    use serde_json::json;

    use super::{StreamAggregator, TokenUsage};

    #[test]
    fn token_usage_uses_standard_add_assign_and_sum() {
        let first = TokenUsage {
            prompt_tokens: 2,
            completion_tokens: 3,
            total_tokens: 5,
        };
        let second = TokenUsage {
            prompt_tokens: 7,
            completion_tokens: 11,
            total_tokens: 18,
        };

        let mut usage = first;
        usage += second;

        assert_eq!(
            usage,
            TokenUsage {
                prompt_tokens: 9,
                completion_tokens: 14,
                total_tokens: 23,
            }
        );

        let total: TokenUsage = [first, second].into_iter().sum();
        assert_eq!(total, usage);
    }

    #[test]
    fn stream_aggregator_collects_text_tool_calls_and_usage() {
        let mut aggregator = StreamAggregator::new();
        let first_tool_call =
            tool_call_chunk(&json!("call-1"), &json!("read_file"), &json!("{\"path\""));
        let second_tool_call = tool_call_chunk(
            &serde_json::Value::Null,
            &serde_json::Value::Null,
            &json!(":\"src/main.rs\"}"),
        );

        let first_update =
            aggregator.ingest(choice_chunk(&text_delta("hel", &serde_json::Value::Null)));
        assert_eq!(first_update.delta.as_deref(), Some("hel"));
        assert_eq!(first_update.usage, None);

        let second_update = aggregator.ingest(choice_chunk(&text_delta("lo", &first_tool_call)));
        assert_eq!(second_update.delta.as_deref(), Some("lo"));
        assert_eq!(second_update.usage, None);

        let tool_only_update = aggregator.ingest(choice_chunk(&null_text_delta(&second_tool_call)));
        assert_eq!(tool_only_update.delta, None);
        assert_eq!(tool_only_update.usage, None);

        let usage_update = aggregator.ingest(usage_chunk());
        assert_eq!(usage_update.delta, None);
        assert_eq!(
            usage_update.usage,
            Some(TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            })
        );

        let message = aggregator.finish();
        assert_eq!(message.content.as_deref(), Some("hello"));
        assert_eq!(
            message.usage,
            Some(TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            })
        );
        let tool_call = message.tool_calls.as_ref().and_then(|calls| calls.first());
        assert_eq!(tool_call.map(|call| call.id.as_str()), Some("call-1"));
        assert_eq!(
            tool_call.map(|call| call.function.name.as_str()),
            Some("read_file")
        );
        assert_eq!(
            tool_call.map(|call| call.function.arguments.as_str()),
            Some(r#"{"path":"src/main.rs"}"#)
        );
    }

    fn choice_chunk(delta: &serde_json::Value) -> CreateChatCompletionStreamResponse {
        stream_chunk(json!({
            "choices": [
                {
                    "index": 0,
                    "delta": delta,
                    "finish_reason": null,
                    "logprobs": null
                }
            ],
            "usage": null
        }))
    }

    fn text_delta(content: &str, tool_call: &serde_json::Value) -> serde_json::Value {
        json!({
            "content": content,
            "function_call": null,
            "tool_calls": optional_tool_calls(tool_call),
            "role": null,
            "refusal": null
        })
    }

    fn null_text_delta(tool_call: &serde_json::Value) -> serde_json::Value {
        json!({
            "content": null,
            "function_call": null,
            "tool_calls": optional_tool_calls(tool_call),
            "role": null,
            "refusal": null
        })
    }

    fn optional_tool_calls(tool_call: &serde_json::Value) -> serde_json::Value {
        if tool_call.is_null() {
            serde_json::Value::Null
        } else {
            json!([tool_call.clone()])
        }
    }

    fn tool_call_chunk(
        id: &serde_json::Value,
        name: &serde_json::Value,
        arguments: &serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "index": 0,
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments
            }
        })
    }

    fn usage_chunk() -> CreateChatCompletionStreamResponse {
        stream_chunk(json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 2,
                "completion_tokens": 3,
                "total_tokens": 5,
                "prompt_tokens_details": null,
                "completion_tokens_details": null
            }
        }))
    }

    fn stream_chunk(mut value: serde_json::Value) -> CreateChatCompletionStreamResponse {
        let object = value.as_object_mut().expect("chunk JSON must be an object");
        object.insert("id".to_owned(), json!("chatcmpl-test"));
        object.insert("created".to_owned(), json!(0));
        object.insert("model".to_owned(), json!("test-model"));
        object.insert("service_tier".to_owned(), serde_json::Value::Null);
        object.insert("system_fingerprint".to_owned(), serde_json::Value::Null);
        object.insert("object".to_owned(), json!("chat.completion.chunk"));

        serde_json::from_value(value).expect("valid stream chunk")
    }
}
