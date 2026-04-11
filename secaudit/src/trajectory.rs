// Trajectory 模块：将审计对话历史转换为 ragrs 评估样本

use std::collections::HashMap;

use ragrs::{Message, MultiTurnSample, ToolCall};

use crate::agent::Finding;
use crate::llm::{ChatMessage, Role};

/// 未知工具名称的默认值
const UNKNOWN_TOOL_NAME: &str = "unknown";

/// 将审计对话历史与发现转换为 ragrs 多轮评估样本。
pub fn to_multi_turn_sample(
    messages: &[ChatMessage],
    findings: &[Finding],
    _target: &str,
) -> MultiTurnSample {
    let ragrs_messages: Vec<Message> = messages
        .iter()
        .enumerate()
        .map(|(idx, msg)| convert_message(msg, idx, messages))
        .collect();

    // reference 字段序列化 findings 供评估指标使用
    let reference = serde_json::to_string(findings).unwrap_or_default();

    MultiTurnSample::new(ragrs_messages).with_reference(reference)
}

/// 将单条 `ChatMessage` 转换为 ragrs `Message`。
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
