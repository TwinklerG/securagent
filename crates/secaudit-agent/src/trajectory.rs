// Trajectory 模块：将审计对话历史转换为评估平台可消费的多轮样本

use std::collections::HashMap;

use serde::Serialize;

use crate::agent::Finding;
use crate::llm::{ChatMessage, Role, TokenUsage, ToolCallResponse};

/// 未知工具名称的默认值
const UNKNOWN_TOOL_NAME: &str = "unknown";

/// 轨迹中的工具调用信息。
#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    /// 工具名称
    pub name: String,
    /// 工具参数
    #[serde(default)]
    pub arguments: HashMap<String, serde_json::Value>,
}

/// 轨迹中的对话消息。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// 系统消息
    System { content: String },
    /// 用户消息
    Human { content: String },
    /// 助手消息，可附带工具调用
    Ai {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    /// 工具执行结果消息
    Tool { name: String, content: String },
}

impl Message {
    #[must_use]
    fn human<S: Into<String>>(content: S) -> Self {
        Self::Human {
            content: content.into(),
        }
    }

    #[must_use]
    fn ai<S: Into<String>>(content: S) -> Self {
        Self::Ai {
            content: content.into(),
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    fn ai_with_tool_calls<S: Into<String>>(content: S, tool_calls: Vec<ToolCall>) -> Self {
        Self::Ai {
            content: content.into(),
            tool_calls,
        }
    }

    #[must_use]
    fn tool<S: Into<String>, T: Into<String>>(name: S, content: T) -> Self {
        Self::Tool {
            name: name.into(),
            content: content.into(),
        }
    }

    #[must_use]
    fn system<S: Into<String>>(content: S) -> Self {
        Self::System {
            content: content.into(),
        }
    }
}

/// 多轮评估样本（Ragas 风格 JSON 结构）。
#[derive(Debug, Clone, Serialize)]
pub struct MultiTurnSample {
    /// 完整对话轨迹
    pub user_input: Vec<Message>,
    /// 参考目标或预期结果
    pub reference: Option<String>,
    /// 参考工具调用序列
    pub reference_tool_calls: Option<Vec<ToolCall>>,
    /// 附加元数据（如 token、时延）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// 将审计对话历史与发现转换为多轮评估样本。
#[must_use]
pub fn to_multi_turn_sample(
    messages: &[ChatMessage],
    findings: &[Finding],
    _target: &str,
) -> MultiTurnSample {
    let sample_messages: Vec<Message> = messages
        .iter()
        .enumerate()
        .map(|(idx, msg)| convert_message(msg, idx, messages))
        .collect();

    // reference 字段序列化 findings 供评估指标使用
    let reference = serde_json::to_string(findings).unwrap_or_default();
    let token_usage = TokenUsage::sum_from_messages(messages);
    let metadata = if token_usage.is_zero() {
        None
    } else {
        let mut map = HashMap::new();
        map.insert(
            "token_usage".to_owned(),
            serde_json::json!({
                "prompt_tokens": token_usage.prompt_tokens,
                "completion_tokens": token_usage.completion_tokens,
                "total_tokens": token_usage.total_tokens,
            }),
        );
        Some(map)
    };

    MultiTurnSample {
        user_input: sample_messages,
        reference: Some(reference),
        reference_tool_calls: None,
        metadata,
    }
}

/// 将单条 `ChatMessage` 转换为评估样本 `Message`。
fn convert_message(msg: &ChatMessage, idx: usize, all: &[ChatMessage]) -> Message {
    match msg.role {
        Role::System => {
            let content = msg.content.as_deref().unwrap_or_default();
            Message::system(content)
        }
        Role::User => {
            let content = msg.content.as_deref().unwrap_or_default();
            Message::human(content)
        }
        Role::Assistant => convert_assistant_message(msg),
        Role::Tool => convert_tool_message(msg, idx, all),
    }
}

fn convert_assistant_message(msg: &ChatMessage) -> Message {
    let content = msg.content.clone().unwrap_or_default();
    let tool_calls = msg
        .tool_calls
        .as_deref()
        .map(convert_tool_calls)
        .unwrap_or_default();

    if tool_calls.is_empty() {
        Message::ai(content)
    } else {
        Message::ai_with_tool_calls(content, tool_calls)
    }
}

fn convert_tool_calls(calls: &[ToolCallResponse]) -> Vec<ToolCall> {
    calls.iter().map(convert_tool_call).collect()
}

fn convert_tool_call(call: &ToolCallResponse) -> ToolCall {
    ToolCall {
        name: call.function.name.clone(),
        arguments: parse_tool_arguments(&call.function.arguments),
    }
}

fn parse_tool_arguments(raw: &str) -> HashMap<String, serde_json::Value> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn convert_tool_message(msg: &ChatMessage, idx: usize, all: &[ChatMessage]) -> Message {
    let content = msg.content.clone().unwrap_or_default();
    let name = resolve_tool_name(msg, idx, all);
    Message::tool(name, content)
}

/// 从 `tool_call_id` 反查工具名称。
///
/// 在前面的 Assistant 消息的 `tool_calls` 中查找匹配的调用 ID，
/// 找不到则返回 `UNKNOWN_TOOL_NAME`。
fn resolve_tool_name(msg: &ChatMessage, idx: usize, all: &[ChatMessage]) -> String {
    let Some(call_id) = msg.tool_call_id.as_deref() else {
        return UNKNOWN_TOOL_NAME.into();
    };

    all.get(..idx)
        .unwrap_or_default()
        .iter()
        .rev()
        .filter(|message| matches!(message.role, Role::Assistant))
        .filter_map(|message| message.tool_calls.as_deref())
        .flatten()
        .find(|call| call.id == call_id)
        .map_or_else(
            || UNKNOWN_TOOL_NAME.into(),
            |call| call.function.name.clone(),
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::to_multi_turn_sample;
    use crate::llm::{ChatMessage, FunctionCall, Role, ToolCallResponse};

    #[test]
    fn converts_assistant_tool_calls_and_tool_results() {
        let messages = vec![
            ChatMessage::user("查找文件"),
            ChatMessage {
                role: Role::Assistant,
                content: Some("我会读取文件".to_owned()),
                tool_calls: Some(vec![ToolCallResponse {
                    id: "call-1".to_owned(),
                    r#type: "function".to_owned(),
                    function: FunctionCall {
                        name: "read_file".to_owned(),
                        arguments: r#"{"path":"src/main.rs"}"#.to_owned(),
                    },
                }]),
                tool_call_id: None,
                usage: None,
            },
            ChatMessage::tool_result("call-1", "内容"),
        ];

        let sample = to_multi_turn_sample(&messages, &[], "src/main.rs");
        let value = serde_json::to_value(sample).ok();

        assert_eq!(
            value
                .as_ref()
                .and_then(|value| value.pointer("/user_input/1")),
            Some(&json!({
                "role": "ai",
                "content": "我会读取文件",
                "tool_calls": [
                    {
                        "name": "read_file",
                        "arguments": {
                            "path": "src/main.rs"
                        }
                    }
                ]
            }))
        );
        assert_eq!(
            value
                .as_ref()
                .and_then(|value| value.pointer("/user_input/2")),
            Some(&json!({
                "role": "tool",
                "name": "read_file",
                "content": "内容"
            }))
        );
    }
}
