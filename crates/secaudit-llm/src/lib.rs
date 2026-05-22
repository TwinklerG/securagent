//! **secaudit-llm** — 通用 LLM 客户端与对话类型，供 workspace 内多个 crate 复用。
//!
//! 基于 `async-openai` 封装，兼容 `OpenAI` Chat Completions API。
//! 提供两种调用模式：
//! - `generate`：简单单轮文本生成
//! - `chat`：多轮对话 + 工具调用（secaudit Agent 使用）

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionStreamOptions, ChatCompletionTool,
    ChatCompletionTools, CreateChatCompletionRequestArgs, CreateChatCompletionStreamResponse,
    FunctionCall as OpenAIFunctionCall, FunctionObjectArgs,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// 把 `other` 累加到当前统计。
    pub fn add_assign(&mut self, other: &Self) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }

    /// 是否为零用量。
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0 && self.total_tokens == 0
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

impl StreamAggregator {
    fn new() -> Self {
        Self::default()
    }

    /// 消费一个 chunk。若 chunk 带新增文本，则返回该增量供调用方触发流式回调。
    fn ingest(&mut self, chunk: CreateChatCompletionStreamResponse) -> Option<String> {
        if let Some(chunk_usage) = chunk.usage.as_ref() {
            self.usage = Some(TokenUsage {
                prompt_tokens: chunk_usage.prompt_tokens.into(),
                completion_tokens: chunk_usage.completion_tokens.into(),
                total_tokens: chunk_usage.total_tokens.into(),
            });
        }

        let choice = chunk.choices.into_iter().next()?;
        let delta = choice.delta;

        if let Some(tc_chunks) = delta.tool_calls {
            for tc in tc_chunks {
                let entry = self.tool_calls.entry(tc.index).or_default();
                if let Some(id) = tc.id {
                    entry.id = Some(id);
                }
                if let Some(func) = tc.function {
                    if let Some(name) = func.name {
                        entry.name = Some(name);
                    }
                    if let Some(args) = func.arguments {
                        entry.arguments.push_str(&args);
                    }
                }
            }
        }

        if let Some(text) = delta.content
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
    pub async fn chat_stream<F>(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
        mut on_delta: F,
    ) -> Result<ChatMessage, Error>
    where
        F: FnMut(&str) + Send,
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
            if let Some(delta) = aggregator.ingest(chunk) {
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
