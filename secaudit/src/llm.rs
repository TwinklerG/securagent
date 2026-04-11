// LLM 模块：基于 async-openai 的模型调用与响应解析

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequestArgs, FunctionCall as OpenAIFunctionCall, FunctionObjectArgs,
};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::Error;

// ─── 核心类型 ───────────────────────────────────────────────────────────────

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

// ─── 类型转换 ───────────────────────────────────────────────────────────────

/// 将内部 `ChatMessage` 转换为 `async-openai` 请求消息类型。
fn to_request_message(msg: &ChatMessage) -> Result<ChatCompletionRequestMessage, Error> {
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
                        ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                            id: tc.id.clone(),
                            function: OpenAIFunctionCall {
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        })
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

/// 将 `ToolDefinition` 转换为 `async-openai` 工具类型。
fn to_chat_completion_tool(def: &ToolDefinition) -> Result<ChatCompletionTools, Error> {
    let function = FunctionObjectArgs::default()
        .name(&def.name)
        .description(&def.description)
        .parameters(def.parameters.clone())
        .build()
        .map_err(|e| Error::Llm(format!("工具定义构建失败：{e}")))?;

    Ok(ChatCompletionTools::Function(ChatCompletionTool {
        function,
    }))
}

/// 将 `async-openai` 工具调用转换为内部类型。
fn from_tool_calls(calls: &[ChatCompletionMessageToolCalls]) -> Vec<ToolCallResponse> {
    calls
        .iter()
        .map(|tc| match tc {
            ChatCompletionMessageToolCalls::Function(tc) => ToolCallResponse {
                id: tc.id.clone(),
                r#type: "function".into(),
                function: FunctionCall {
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                },
            },
            ChatCompletionMessageToolCalls::Custom(tc) => ToolCallResponse {
                id: tc.id.clone(),
                r#type: "custom".into(),
                function: FunctionCall {
                    name: tc.custom_tool.name.clone(),
                    arguments: tc.custom_tool.input.clone(),
                },
            },
        })
        .collect()
}

// ─── 客户端 ─────────────────────────────────────────────────────────────────

/// LLM 客户端，基于 `async-openai` 封装 `OpenAI` 兼容 API 的交互逻辑。
pub struct LlmClient {
    /// async-openai 客户端
    client: async_openai::Client<OpenAIConfig>,
    /// 模型名称
    model: String,
}

impl LlmClient {
    /// 根据配置创建客户端实例
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let openai_config = OpenAIConfig::new()
            .with_api_base(&config.api_base_url)
            .with_api_key(&config.api_key);

        Self {
            client: async_openai::Client::with_config(openai_config),
            model: config.model.clone(),
        }
    }

    /// 发送对话请求，支持工具定义。
    ///
    /// # Errors
    ///
    /// - 请求构建失败时返回 [`Error::Llm`]
    /// - API 调用失败时返回 [`Error::Llm`]
    /// - 响应中无有效选项时返回 [`Error::Llm`]
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatMessage, Error> {
        // 转换消息列表
        let request_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(to_request_message)
            .collect::<Result<_, _>>()?;

        // 构建请求
        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(&self.model).messages(request_messages);

        if let Some(tool_defs) = tools
            && !tool_defs.is_empty()
        {
            let chat_tools: Vec<ChatCompletionTools> = tool_defs
                .iter()
                .map(to_chat_completion_tool)
                .collect::<Result<_, _>>()?;
            builder.tools(chat_tools);
        }

        let request = builder
            .build()
            .map_err(|e| Error::Llm(format!("请求构建失败：{e}")))?;

        tracing::debug!(model = %self.model, "发送 LLM 请求");
        tracing::debug!(messages_count = messages.len(), "消息数量");

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

        let result = ChatMessage {
            role: Role::Assistant,
            content: message.content,
            tool_calls,
            tool_call_id: None,
        };

        tracing::debug!(
            has_content = result.content.is_some(),
            has_tool_calls = result.tool_calls.is_some(),
            "收到 LLM 响应"
        );

        Ok(result)
    }
}
