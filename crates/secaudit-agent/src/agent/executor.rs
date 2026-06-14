// ReAct 循环执行器
use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::time::sleep;

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
    /// LLM 调用重试与熔断策略
    llm_policy: LlmCallPolicy,
}

/// LLM 调用重试与熔断配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmCallConfig {
    /// 最大尝试次数（含首次调用）。
    pub max_attempts: u32,
    /// 指数退避初始延迟。
    pub initial_delay: Duration,
    /// 指数退避最大延迟。
    pub max_delay: Duration,
    /// 熔断连续失败阈值。
    pub circuit_breaker_failure_threshold: u32,
    /// 熔断冷却时间。
    pub circuit_breaker_cooldown: Duration,
}

impl Default for LlmCallConfig {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(2),
            circuit_breaker_failure_threshold: 3,
            circuit_breaker_cooldown: Duration::from_secs(30),
        }
    }
}

impl LlmCallConfig {
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            max_attempts: self.max_attempts.max(1),
            initial_delay: self.initial_delay.min(self.max_delay),
            max_delay: self.max_delay,
            circuit_breaker_failure_threshold: self.circuit_breaker_failure_threshold.max(1),
            circuit_breaker_cooldown: self.circuit_breaker_cooldown,
        }
    }
}

#[derive(Debug)]
struct CircuitBreakerState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl CircuitBreakerState {
    const fn new() -> Self {
        Self {
            consecutive_failures: 0,
            open_until: None,
        }
    }
}

/// 可跨多个 executor 共享的 LLM 调用保护策略。
#[derive(Debug, Clone)]
pub struct LlmCallPolicy {
    config: LlmCallConfig,
    state: Arc<Mutex<CircuitBreakerState>>,
}

impl Default for LlmCallPolicy {
    fn default() -> Self {
        Self::new(LlmCallConfig::default())
    }
}

impl LlmCallPolicy {
    #[must_use]
    pub fn new(config: LlmCallConfig) -> Self {
        Self {
            config: config.normalized(),
            state: Arc::new(Mutex::new(CircuitBreakerState::new())),
        }
    }

    fn before_call(&self) -> Result<(), Error> {
        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .map_err(|_error| Error::Llm("LLM 熔断状态锁已损坏".to_owned()))?;

        if let Some(open_until) = state.open_until {
            if now < open_until {
                let remaining = open_until.saturating_duration_since(now);
                return Err(Error::Llm(format!(
                    "LLM 熔断开启，约 {}ms 后重试",
                    remaining.as_millis()
                )));
            }
            state.open_until = None;
        }

        Ok(())
    }

    fn record_success(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.consecutive_failures = 0;
            state.open_until = None;
        }
    }

    fn record_failure(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            if state.consecutive_failures >= self.config.circuit_breaker_failure_threshold {
                state.open_until = Some(Instant::now() + self.config.circuit_breaker_cooldown);
            }
        }
    }

    fn retry_delay(&self, completed_failures: u32) -> Duration {
        if completed_failures == 0 {
            return Duration::ZERO;
        }

        let multiplier = 1_u32
            .checked_shl(completed_failures.saturating_sub(1).min(31))
            .unwrap_or(u32::MAX);
        self.config
            .initial_delay
            .saturating_mul(multiplier)
            .min(self.config.max_delay)
    }
}

impl<'a> ReActExecutor<'a> {
    /// 创建执行器，自动从 `Tool` trait 构建工具定义列表。
    #[must_use]
    pub fn new(llm: &'a HttpLlmClient, tools: &'a [Box<dyn Tool>]) -> Self {
        Self::with_llm_policy(llm, tools, LlmCallPolicy::default())
    }

    /// 创建执行器，并使用调用方提供的 LLM 调用保护策略。
    #[must_use]
    pub fn with_llm_policy(
        llm: &'a HttpLlmClient,
        tools: &'a [Box<dyn Tool>],
        llm_policy: LlmCallPolicy,
    ) -> Self {
        let tool_defs = tool_definitions_from_tools(tools);

        Self {
            llm,
            tools,
            tool_defs,
            messages: Vec::new(),
            llm_policy,
        }
    }

    /// 添加消息到对话历史
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// 执行一步 `ReAct`：发送当前对话给 LLM，返回步骤结果。
    ///
    /// 将 LLM 回复自动加入对话历史。
    ///
    /// # Errors
    ///
    /// LLM 调用失败时会对可重试错误做一次普通 `chat` 重试，仍失败时返回 [`Error::Llm`]。
    pub async fn step(&mut self) -> Result<StepResult, Error> {
        let response = self.chat_with_retry().await?;

        // 将 assistant 消息加入历史
        self.messages.push(response.clone());

        Ok(step_result_from_response(&response))
    }

    /// 流式执行一步 `ReAct`：与 [`Self::step`] 等价，但通过 `on_delta` 回调
    /// 实时回吐 assistant 文本增量片段。
    ///
    /// 当流式 LLM 调用失败时，降级为普通 `chat` 调用继续执行。
    ///
    /// # Errors
    ///
    /// 流式调用与降级后的普通 `chat` 调用均失败时返回 [`Error::Llm`]。
    pub async fn step_stream<F, U>(&mut self, on_delta: F, on_usage: U) -> Result<StepResult, Error>
    where
        F: FnMut(&str) + Send,
        U: FnMut(TokenUsage) + Send,
    {
        let response = match self
            .llm
            .chat_stream(&self.messages, Some(&self.tool_defs), on_delta, on_usage)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!("streaming LLM call failed; falling back to normal chat: {error}");
                self.chat_with_retry().await?
            }
        };

        self.messages.push(response.clone());

        Ok(step_result_from_response(&response))
    }

    /// 执行工具调用并将结果加入对话历史。
    ///
    /// 前置校验和工具执行失败时不传播错误，而是将错误信息作为结果返回给 LLM，
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

                    if let Err(reason) = t.precheck(&params).await {
                        format!(
                            "工具前置校验未通过：{reason}\n\n建议：请调整参数或改用更安全的方式完成目标。"
                        )
                    } else {
                        // 捕获工具执行错误，返回错误信息并附加重试建议
                        match t.execute(&params).await {
                            Ok(output) => output,
                            Err(e) => format!(
                                "工具执行失败：{e}\n\n建议：请检查参数是否正确，或尝试使用其他工具完成同样的目标。"
                            ),
                        }
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

    async fn chat_with_retry(&self) -> Result<ChatMessage, Error> {
        self.llm_policy.before_call()?;
        let mut attempt = 1_u32;
        loop {
            match self.llm.chat(&self.messages, Some(&self.tool_defs)).await {
                Ok(response) => {
                    self.llm_policy.record_success();
                    return Ok(response);
                }
                Err(error) => {
                    if attempt >= self.llm_policy.config.max_attempts
                        || !is_retryable_llm_error(&error)
                    {
                        self.llm_policy.record_failure();
                        return Err(Error::from(error));
                    }
                    let delay = self.llm_policy.retry_delay(attempt);
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        "LLM chat call failed; retrying after exponential backoff: {error}"
                    );
                    if !delay.is_zero() {
                        sleep(delay).await;
                    }
                    attempt += 1;
                }
            }
        }
    }
}

fn is_retryable_llm_error(error: &impl Display) -> bool {
    let message = error.to_string();
    message.contains("API 调用失败") || message.contains("流式响应读取失败")
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
