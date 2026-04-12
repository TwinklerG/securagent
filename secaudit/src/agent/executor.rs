// ReAct 循环执行器

use crate::error::Error;
use crate::llm::{ChatMessage, HttpLlmClient, ToolCallResponse, ToolDefinition};
use crate::tools::Tool;

/// `ReAct` 单步执行结果
pub enum StepResult {
    /// LLM 请求了工具调用
    ToolCalls(Vec<ToolCallResponse>),
    /// LLM 给出了文本回复（无工具调用），表示思考完成
    TextResponse(String),
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
        let tool_defs: Vec<ToolDefinition> = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().into(),
                description: t.description().into(),
                parameters: t.parameters_schema(),
            })
            .collect();

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

        if let Some(tool_calls) = &response.tool_calls
            && !tool_calls.is_empty()
        {
            return Ok(StepResult::ToolCalls(tool_calls.clone()));
        }

        Ok(StepResult::TextResponse(
            response.content.unwrap_or_default(),
        ))
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
