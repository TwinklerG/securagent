// Trajectory 模块：将审计对话历史转换为评估平台可消费的多轮样本

use std::collections::HashMap;

use serde::Serialize;

use crate::agent::Finding;
use crate::llm::{ChatMessage, Role};

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

    MultiTurnSample {
        user_input: sample_messages,
        reference: Some(reference),
        reference_tool_calls: None,
        metadata: None,
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
        Role::Assistant => {
            let content = msg.content.clone().unwrap_or_default();
            let tool_calls = msg.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|tc| {
                        let arguments = serde_json::from_str::<HashMap<String, serde_json::Value>>(
                            &tc.function.arguments,
                        )
                        .unwrap_or_default();
                        ToolCall {
                            name: tc.function.name.clone(),
                            arguments,
                        }
                    })
                    .collect::<Vec<_>>()
            });

            match tool_calls {
                Some(calls) if !calls.is_empty() => Message::ai_with_tool_calls(content, calls),
                _ => Message::ai(content),
            }
        }
        Role::Tool => {
            let content = msg.content.clone().unwrap_or_default();
            let name = resolve_tool_name(msg, idx, all);
            Message::tool(name, content)
        }
    }
}

/// 从 `tool_call_id` 反查工具名称。
///
/// 在前面的 Assistant 消息的 `tool_calls` 中查找匹配的调用 ID，
/// 找不到则返回 `UNKNOWN_TOOL_NAME`。
fn resolve_tool_name(msg: &ChatMessage, idx: usize, all: &[ChatMessage]) -> String {
    let Some(call_id) = msg.tool_call_id.as_deref() else {
        return UNKNOWN_TOOL_NAME.into();
    };

    // 向前搜索最近的 Assistant 消息
    for prev in all.get(..idx).unwrap_or_default().iter().rev() {
        if !matches!(prev.role, Role::Assistant) {
            continue;
        }
        if let Some(calls) = &prev.tool_calls {
            for tc in calls {
                if tc.id == call_id {
                    return tc.function.name.clone();
                }
            }
        }
    }

    UNKNOWN_TOOL_NAME.into()
}
