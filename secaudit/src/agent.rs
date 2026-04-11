// Agent 模块：ReAct 循环与审计流程编排

pub mod executor;
pub mod state;
pub mod strategy;

use crate::config::Config;
use crate::error::Error;
use crate::llm::{ChatMessage, LlmClient, Role};
use crate::prompt;
use crate::tools;
use crate::tools::Tool;
use executor::{ReActExecutor, StepResult};
use state::AgentState;

use serde::{Deserialize, Serialize};

/// 审计发现
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// CWE 编号
    pub cwe_id: Option<String>,
    /// 严重程度（Critical / High / Medium / Low / Info）
    pub severity: String,
    /// 漏洞描述
    pub description: String,
    /// 代码位置
    pub location: Option<String>,
    /// 修复建议
    pub remediation: Option<String>,
}

/// 审计报告
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    /// 审计目标
    pub target: String,
    /// 编程语言
    pub language: String,
    /// 发现的安全问题列表
    pub findings: Vec<Finding>,
    /// 审计总结
    pub summary: String,
    /// 实际使用的迭代轮次
    pub iterations_used: u32,
    /// 完整对话历史（用于 trajectory 导出，不参与序列化）
    #[serde(skip)]
    pub messages: Vec<ChatMessage>,
}

/// 状态变更回调
pub type StateCallback = Box<dyn Fn(&AgentState) + Send + Sync>;
/// 思考过程回调
pub type ThinkCallback = Box<dyn Fn(&str) + Send + Sync>;
/// 工具调用回调（工具名, 参数）
pub type ToolCallCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

/// 事件总线：管理 Agent 状态与回调，独立于 LLM/工具借用。
pub(crate) struct EventBus {
    /// 当前运行状态
    state: AgentState,
    /// 状态变更回调
    on_state_change: Option<StateCallback>,
    /// 思考过程回调
    on_think: Option<ThinkCallback>,
    /// 工具调用回调
    on_tool_call: Option<ToolCallCallback>,
}

impl EventBus {
    /// 更新状态并触发回调
    pub(crate) fn set_state(&mut self, state: AgentState) {
        self.state = state;
        if let Some(cb) = &self.on_state_change {
            cb(&self.state);
        }
    }

    /// 通知思考内容
    pub(crate) fn notify_think(&self, text: &str) {
        if let Some(cb) = &self.on_think {
            cb(text);
        }
    }

    /// 通知工具调用
    pub(crate) fn notify_tool_call(&self, name: &str, args: &str) {
        if let Some(cb) = &self.on_tool_call {
            cb(name, args);
        }
    }
}

/// 安全代码审计 Agent
///
/// 基于 ReAct（Observe-Reason-Act）模式，协调 LLM 与工具完成代码安全审计。
pub struct Agent {
    /// 应用配置
    config: Config,
    /// LLM 客户端
    llm: LlmClient,
    /// 可用工具集
    tools: Vec<Box<dyn Tool>>,
    /// 事件总线：状态与回调管理
    events: EventBus,
}

impl Agent {
    /// 在预期文本输出的阶段中推进执行：
    /// 若模型先返回工具调用，则先执行工具并继续请求，直到拿到文本或达到上限。
    async fn step_until_text(
        &self,
        executor: &mut ReActExecutor<'_>,
        phase: &str,
    ) -> Result<String, Error> {
        let max_tool_rounds = self.config.max_iterations.max(1);
        let mut tool_rounds = 0u32;

        loop {
            match executor.step().await? {
                StepResult::TextResponse(text) => return Ok(text),
                StepResult::ToolCalls(calls) => {
                    tool_rounds += 1;
                    tracing::warn!(
                        phase,
                        tool_rounds,
                        "阶段预期文本输出，但收到了工具调用，已自动执行并继续"
                    );

                    for call in &calls {
                        self.events
                            .notify_tool_call(&call.function.name, &call.function.arguments);
                    }

                    let _results = executor.execute_tool_calls(&calls).await?;

                    if tool_rounds >= max_tool_rounds {
                        return Err(Error::Llm(format!(
                            "{phase} 在执行 {tool_rounds} 轮工具调用后仍未返回文本"
                        )));
                    }
                }
            }
        }
    }

    /// 创建 Agent 实例
    #[must_use]
    pub fn new(config: Config) -> Self {
        let llm = LlmClient::new(&config);
        let tools = tools::default_tools(&config);
        Self {
            config,
            llm,
            tools,
            events: EventBus {
                state: AgentState::Init,
                on_state_change: None,
                on_think: None,
                on_tool_call: None,
            },
        }
    }

    /// 设置状态变更回调
    pub fn on_state_change<F: Fn(&AgentState) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_state_change = Some(Box::new(cb));
    }

    /// 设置思考过程回调
    pub fn on_think<F: Fn(&str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_think = Some(Box::new(cb));
    }

    /// 设置工具调用回调
    pub fn on_tool_call<F: Fn(&str, &str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_tool_call = Some(Box::new(cb));
    }

    /// 执行安全审计
    ///
    /// 完整流程：初始化 -> 规划 -> `ReAct` 执行循环 -> 反思 -> 生成报告
    ///
    /// # Errors
    ///
    /// LLM 调用或工具执行过程中发生不可恢复错误时返回对应 [`Error`]。
    pub async fn audit(
        &mut self,
        code: &str,
        language: &str,
        target: &str,
    ) -> Result<AuditReport, Error> {
        self.events.set_state(AgentState::Init);

        let mut executor = ReActExecutor::new(&self.llm, &self.tools);

        // 1. 系统 prompt
        executor.push_message(ChatMessage::system(prompt::SYSTEM_PROMPT));

        // 2. 规划阶段
        self.events.set_state(AgentState::Planning);
        executor.push_message(ChatMessage::user(prompt::planning_prompt(code, language)));

        let plan = self.step_until_text(&mut executor, "规划阶段").await?;
        self.events.notify_think(&plan);

        // 3. 策略推理循环
        let mut strat =
            strategy::StrategyKind::from_str_name(&self.config.reasoning_strategy).build();
        let strategy_result = strat
            .run(&mut executor, &mut self.events, &self.config)
            .await?;

        // 4. 反思阶段
        self.events.set_state(AgentState::Reflecting);

        let findings_text: String = executor
            .messages()
            .iter()
            .filter_map(|m| {
                if matches!(m.role, Role::Assistant) {
                    m.content.as_deref()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n---\n");

        executor.push_message(ChatMessage::user(prompt::reflection_prompt(&findings_text)));

        let final_report_text = self.step_until_text(&mut executor, "反思阶段").await?;

        self.events.notify_think(&final_report_text);

        // 5. 提取结构化发现
        self.events.set_state(AgentState::Extracting);

        executor.push_message(ChatMessage::user(prompt::findings_extraction_prompt(
            &final_report_text,
        )));

        let extraction_text = self.step_until_text(&mut executor, "发现提取阶段").await?;
        let findings =
            serde_json::from_str::<Vec<Finding>>(&extraction_text).unwrap_or_else(|err| {
                tracing::warn!("发现提取 JSON 解析失败，回退为空列表：{err}");
                Vec::new()
            });

        // 6. 生成报告
        self.events.set_state(AgentState::Reporting);

        let messages = executor.messages().to_vec();

        let report = AuditReport {
            target: target.into(),
            language: language.into(),
            findings,
            summary: final_report_text,
            iterations_used: strategy_result.iterations_used,
            messages,
        };

        self.events.set_state(AgentState::Done);

        Ok(report)
    }
}
