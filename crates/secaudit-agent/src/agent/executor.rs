// ReAct 循环执行器

use crate::error::Error;
use crate::llm::{ChatMessage, HttpLlmClient, TokenUsage, ToolCallResponse, ToolDefinition};
use crate::tools::Tool;

const MAX_TOOL_RESULT_CHARS: usize = 20_000;
const TOOL_RESULT_TRUNCATED: &str =
    "\n\n[工具结果已截断，输出过长。请缩小路径、降低递归深度或使用更精确的搜索。]";

/// `ReAct` 单步执行结果
pub enum StepResult {
    /// LLM 请求了工具调用
    ToolCalls(Vec<ToolCallResponse>),
    /// LLM 给出了文本回复（无工具调用），表示思考完成
    TextResponse(String),
}

fn step_result_from_response(response: &ChatMessage) -> StepResult {
    if let Some(tool_calls) = &response.tool_calls
        && !tool_calls.is_empty()
    {
        return StepResult::ToolCalls(tool_calls.clone());
    }

    StepResult::TextResponse(response.content.clone().unwrap_or_default())
}

/// `ReAct` 循环执行器，管理对话历史并协调 LLM 与工具交互。
pub struct ReActExecutor<'a> {
    /// LLM 客户端引用
    llm: &'a HttpLlmClient,
    /// 可用工具列表
    tools: &'a [Box<dyn Tool>],
    /// 工具定义（发送给 LLM 的描述信息）
    tool_defs: Vec<ToolDefinition>,
    /// 对话历史
    messages: Vec<ChatMessage>,
}

impl<'a> ReActExecutor<'a> {
    /// 创建执行器，自动从 `Tool` trait 构建工具定义列表。
    #[must_use]
    pub fn new(llm: &'a HttpLlmClient, tools: &'a [Box<dyn Tool>]) -> Self {
        let tool_defs = tool_definitions_from_tools(tools);

        Self {
            llm,
            tools,
            tool_defs,
            messages: Vec::new(),
        }
    }

    /// 添加消息到对话历史
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// 执行一步 ReAct：发送当前对话给 LLM，返回步骤结果。
    ///
    /// 将 LLM 回复自动加入对话历史。
    ///
    /// # Errors
    ///
    /// LLM 调用失败时返回 [`Error::Llm`]。
    pub async fn step(&mut self) -> Result<StepResult, Error> {
        let response = self.llm.chat(&self.messages, Some(&self.tool_defs)).await?;

        // 将 assistant 消息加入历史
        self.messages.push(response.clone());

        Ok(step_result_from_response(&response))
    }

    /// 流式执行一步 ReAct：与 [`Self::step`] 等价，但通过 `on_delta` 回调
    /// 实时回吐 assistant 文本增量片段。
    ///
    /// # Errors
    ///
    /// LLM 调用失败时返回 [`Error::Llm`]。
    pub async fn step_stream<F, U>(&mut self, on_delta: F, on_usage: U) -> Result<StepResult, Error>
    where
        F: FnMut(&str) + Send,
        U: FnMut(TokenUsage) + Send,
    {
        let response = self
            .llm
            .chat_stream(&self.messages, Some(&self.tool_defs), on_delta, on_usage)
            .await?;

        self.messages.push(response.clone());

        Ok(step_result_from_response(&response))
    }

    /// 执行工具调用并将结果加入对话历史。
    ///
    /// 工具执行失败时不传播错误，而是将错误信息作为结果返回给 LLM，
    /// 让 LLM 自行决定如何处理（自我修正）。
    ///
    /// # Errors
    ///
    /// 仅当工具参数 JSON 解析失败时返回 [`Error::Parse`]。
    pub async fn execute_tool_calls(
        &mut self,
        tool_calls: &[ToolCallResponse],
    ) -> Result<Vec<(String, String)>, Error> {
        let mut results = Vec::new();

        for call in tool_calls {
            let tool = self.tools.iter().find(|t| t.name() == call.function.name);

            let result = match tool {
                Some(t) => {
                    let params: serde_json::Value = serde_json::from_str(&call.function.arguments)
                        .map_err(|e| Error::Parse(format!("工具参数解析失败：{e}")))?;
                    // 捕获工具执行错误，返回错误信息并附加重试建议
                    match t.execute(&params).await {
                        Ok(output) => output,
                        Err(e) => format!(
                            "工具执行失败：{e}\n\n建议：请检查参数是否正确，或尝试使用其他工具完成同样的目标。"
                        ),
                    }
                }
                None => format!(
                    "未知工具：{}。可用工具：{}",
                    call.function.name,
                    self.tools
                        .iter()
                        .map(|t| t.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };

            let result = truncate_tool_result(&result);

            self.messages
                .push(ChatMessage::tool_result(&call.id, &result));
            results.push((call.function.name.clone(), result));
        }

        Ok(results)
    }

    /// 获取当前对话历史
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }
}

fn truncate_tool_result(result: &str) -> String {
    if result.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return result.to_owned();
    }

    let mut truncated = result
        .chars()
        .take(MAX_TOOL_RESULT_CHARS)
        .collect::<String>();
    truncated.push_str(TOOL_RESULT_TRUNCATED);
    truncated
}

pub(crate) fn tool_definitions_from_tools(tools: &[Box<dyn Tool>]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|tool| ToolDefinition {
            name: tool.name().into_owned(),
            description: tool.description().into_owned(),
            parameters: tool.parameters_schema(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::llm::{ChatMessage, FunctionCall, Role, ToolCallResponse};

    use super::{
        MAX_TOOL_RESULT_CHARS, StepResult, TOOL_RESULT_TRUNCATED, step_result_from_response,
        truncate_tool_result,
    };

    #[test]
    fn response_with_tool_calls_becomes_tool_step() {
        let response = ChatMessage {
            role: Role::Assistant,
            content: Some("ignored when tool call exists".to_owned()),
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
        };

        match step_result_from_response(&response) {
            StepResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    calls.first().map(|call| call.function.name.as_str()),
                    Some("read_file")
                );
            }
            StepResult::TextResponse(text) => {
                panic!("expected tool calls, got text response: {text}");
            }
        }
    }

    #[test]
    fn response_without_tool_calls_becomes_text_step() {
        let response = ChatMessage {
            role: Role::Assistant,
            content: Some("done".to_owned()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        };

        match step_result_from_response(&response) {
            StepResult::TextResponse(text) => assert_eq!(text, "done"),
            StepResult::ToolCalls(calls) => {
                panic!("expected text response, got {} tool calls", calls.len());
            }
        }
    }

    #[test]
    fn truncates_large_tool_results() {
        let result = "x".repeat(MAX_TOOL_RESULT_CHARS + 100);

        let truncated = truncate_tool_result(&result);

        assert!(truncated.chars().count() < result.chars().count());
        assert!(truncated.ends_with(TOOL_RESULT_TRUNCATED));
    }
}
