// Agent 模块：ReAct 循环与审计流程编排

pub mod executor;
pub(crate) mod skill_tool;
pub mod state;
pub mod strategy;

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Config;
use crate::error::Error;
use crate::llm::{self, ChatMessage, HttpLlmClient, Role, TokenUsage, ToolDefinition};
use crate::prompt;
use crate::session::Session;
use crate::tools;
use crate::tools::{ConfirmFn, Tool};
use executor::{ReActExecutor, StepResult, tool_definitions_from_tools};
use state::AgentState;

use secaudit_skills::SkillRegistry;
use serde::{Deserialize, Serialize};
use tracing::debug;

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
/// 流式 token 增量回调（每次仅传当前 chunk 的文本片段）
pub type TokenCallback = Box<dyn Fn(&str) + Send + Sync>;
pub type UsageCallback = Box<dyn Fn(TokenUsage) + Send + Sync>;
/// 工具调用回调（工具名, 参数）
pub type ToolCallCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
/// 工具结果回调（工具名, 结果）
pub type ToolResultCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

/// 事件总线：管理 Agent 状态与回调，独立于 LLM/工具借用。
pub(crate) struct EventBus {
    /// 当前运行状态
    state: AgentState,
    /// 状态变更回调
    on_state_change: Option<StateCallback>,
    /// 思考过程回调
    on_think: Option<ThinkCallback>,
    /// 流式 token 增量回调
    on_token: Option<TokenCallback>,
    /// 真实 token usage 回调
    on_usage: Option<UsageCallback>,
    /// 工具调用回调
    on_tool_call: Option<ToolCallCallback>,
    /// 工具结果回调
    on_tool_result: Option<ToolResultCallback>,
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

    /// 通知流式 token 增量
    pub(crate) fn notify_token(&self, delta: &str) {
        if let Some(cb) = &self.on_token {
            cb(delta);
        }
    }

    pub(crate) fn notify_usage(&self, usage: TokenUsage) {
        if let Some(cb) = &self.on_usage {
            cb(usage);
        }
    }

    /// 通知工具调用
    pub(crate) fn notify_tool_call(&self, name: &str, args: &str) {
        if let Some(cb) = &self.on_tool_call {
            cb(name, args);
        }
    }

    /// 通知工具结果
    pub(crate) fn notify_tool_result(&self, name: &str, result: &str) {
        if let Some(cb) = &self.on_tool_result {
            cb(name, result);
        }
    }
}

/// 安全代码审计 Agent
///
/// 支持两种工作模式：
/// - 单文件审计 `audit()`：传入代码文本，走完整规划→执行→反思→报告流程
/// - 交互式对话 `chat()`：接收用户消息，Agent 自主使用工具后返回文本回复
pub struct Agent {
    /// 应用配置
    config: Config,
    /// LLM 客户端
    llm: HttpLlmClient,
    /// 可用工具集
    tools: Vec<Box<dyn Tool>>,
    /// 事件总线：状态与回调管理
    events: EventBus,
    /// Skill 注册表
    skill_registry: Option<Arc<SkillRegistry>>,
    /// 工作目录
    work_dir: PathBuf,
}

/// 连续无工具调用的空循环阈值
const MAX_EMPTY_ROUNDS: u32 = 2;

/// 接近迭代上限时注入催促 prompt 的剩余轮次阈值
const WRAP_UP_THRESHOLD: u32 = 3;

/// 催促 LLM 收尾的提示消息
const MSG_WRAP_UP: &str = "\
你即将达到工具调用轮次上限。请立即总结你的发现并给出最终回复，不要再调用工具。";
///
/// 规划等预期文本输出的阶段，LLM 可能先调用工具探索项目再给出文本，
/// 因此需要更宽松的工具轮次上限。
const MAX_TEXT_PHASE_TOOL_ROUNDS: u32 = 12;

impl Agent {
    /// 在预期文本输出的阶段中推进执行：
    /// 若模型先返回工具调用，则先执行工具并继续请求，直到拿到文本或达到上限。
    async fn step_until_text(
        &self,
        executor: &mut ReActExecutor<'_>,
        phase: &str,
    ) -> Result<String, Error> {
        let max_tool_rounds = MAX_TEXT_PHASE_TOOL_ROUNDS;
        let mut tool_rounds = 0u32;

        loop {
            let events_ref = &self.events;
            let step_result = executor
                .step_stream(
                    |delta| events_ref.notify_token(delta),
                    |usage| events_ref.notify_usage(usage),
                )
                .await?;
            match step_result {
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

                    let results = executor.execute_tool_calls(&calls).await?;
                    for (name, result) in &results {
                        self.events.notify_tool_result(name, result);
                    }

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
    pub fn new(config: Config, work_dir: PathBuf, confirm: ConfirmFn) -> Self {
        let llm = llm::create_client(&config);
        let mut tools = tools::default_tools(work_dir.clone(), confirm);
        let skill_registry = if config.enable_skills {
            match SkillRegistry::load_from_dir(&work_dir) {
                Ok(registry) => {
                    let arc = Arc::new(registry);
                    tools.push(Box::new(skill_tool::UseSkillTool::new(arc.clone())));
                    Some(arc)
                }
                Err(err) => {
                    tracing::warn!("加载 Skills 失败，已禁用 Skill 匹配：{err}");
                    None
                }
            }
        } else {
            None
        };

        Self {
            config,
            llm,
            tools,
            events: EventBus {
                state: AgentState::Init,
                on_state_change: None,
                on_think: None,
                on_token: None,
                on_usage: None,
                on_tool_call: None,
                on_tool_result: None,
            },
            skill_registry,
            work_dir,
        }
    }

    /// 获取工作目录
    #[must_use]
    pub fn work_dir(&self) -> &PathBuf {
        &self.work_dir
    }

    /// 设置状态变更回调
    pub fn on_state_change<F: Fn(&AgentState) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_state_change = Some(Box::new(cb));
    }

    /// 设置思考过程回调
    pub fn on_think<F: Fn(&str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_think = Some(Box::new(cb));
    }

    /// 设置流式 token 增量回调。
    ///
    /// 注册后，Agent 在调用 LLM 时会启用流式响应，每收到一个文本片段就调用 `cb`。
    /// 适合 TUI/前端实时打字机式渲染。
    pub fn on_token<F: Fn(&str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_token = Some(Box::new(cb));
    }

    pub fn on_usage<F: Fn(TokenUsage) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_usage = Some(Box::new(cb));
    }

    /// 设置工具调用回调
    pub fn on_tool_call<F: Fn(&str, &str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_tool_call = Some(Box::new(cb));
    }

    /// 设置工具结果回调
    pub fn on_tool_result<F: Fn(&str, &str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.events.on_tool_result = Some(Box::new(cb));
    }

    /// 获取可用工具名称列表
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().into_owned()).collect()
    }

    /// 获取可用工具定义列表。
    #[must_use]
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        tool_definitions_from_tools(&self.tools)
    }

    /// 获取可用 Skill 名称和描述列表。
    #[must_use]
    pub fn skill_list(&self) -> Vec<(String, String)> {
        self.skill_registry
            .as_ref()
            .map(|r| r.list())
            .unwrap_or_default()
    }

    /// 将匹配的 Skill 注入为本轮 Agent 输入；未匹配时保持原始用户输入。
    fn build_agent_input(&self, user_message: &str, session_id: &str) -> String {
        self.skill_registry
            .as_ref()
            .and_then(|registry| registry.match_command(user_message))
            .map_or_else(
                || user_message.to_owned(),
                |skill| {
                    let notice = format!("触发 Skill：{}", skill.name());
                    self.events.notify_think(&notice);
                    skill.build_prompt(user_message, session_id)
                },
            )
    }

    /// 单轮交互：接收用户消息，Agent 自主使用工具后返回文本回复。
    ///
    /// `Session` 持有对话历史，`chat()` 内部进入 `ReAct` 循环，直到 LLM 返回纯文本。
    ///
    /// # Errors
    ///
    /// LLM 调用或工具执行失败时返回 [`Error`]。
    pub async fn chat(
        &mut self,
        session: &mut Session,
        user_message: &str,
    ) -> Result<String, Error> {
        // 首次对话注入 system prompt
        if session.messages().is_empty() {
            let system_prompt = prompt::chat_system_prompt(&self.work_dir);
            session.push_message(ChatMessage::system(system_prompt));
        }

        let agent_input = self.build_agent_input(user_message, &session.id);

        // 追加原始用户消息，保持会话历史与界面展示一致。
        session.push_message(ChatMessage::user(user_message));

        // 构建 executor 并加载历史；本轮输入使用 Skill 注入后的内容。
        let mut executor = ReActExecutor::new(&self.llm, &self.tools);
        let history_len = session.messages().len().saturating_sub(1);
        for msg in session.messages().iter().take(history_len) {
            executor.push_message(msg.clone());
        }
        executor.push_message(ChatMessage::user(agent_input));

        // ReAct 循环
        let max_iter = self.config.max_iterations;
        let mut iteration = 0u32;
        let mut empty_rounds = 0u32;
        let mut final_text = String::new();

        self.events
            .set_state(AgentState::Executing { iteration: 0 });

        loop {
            if iteration >= max_iter {
                break;
            }

            // 接近上限时注入催促 prompt，引导 LLM 收尾
            if iteration + WRAP_UP_THRESHOLD == max_iter {
                executor.push_message(ChatMessage::user(MSG_WRAP_UP));
            }

            let step = {
                let events_ref = &self.events;
                executor
                    .step_stream(
                        |delta| events_ref.notify_token(delta),
                        |usage| events_ref.notify_usage(usage),
                    )
                    .await?
            };

            match step {
                StepResult::ToolCalls(calls) => {
                    debug!("第 {iteration} 轮工具调用：{calls:?}");
                    empty_rounds = 0;
                    for call in &calls {
                        self.events
                            .notify_tool_call(&call.function.name, &call.function.arguments);
                    }
                    let results = executor.execute_tool_calls(&calls).await?;
                    for (name, result) in &results {
                        self.events.notify_tool_result(name, result);
                    }
                    self.events.set_state(AgentState::Analyzing);
                }
                StepResult::TextResponse(text) => {
                    self.events.notify_think(&text);
                    final_text.clone_from(&text);

                    empty_rounds += 1;
                    if empty_rounds >= MAX_EMPTY_ROUNDS {
                        break;
                    }
                    // 首次无工具调用即视为回复完成
                    break;
                }
            }

            iteration += 1;
        }

        // 同步新消息到 session（跳过已有的历史前缀）
        let session_len = session.messages().len();
        let all_messages = executor.messages();
        for msg in all_messages.get(session_len..).unwrap_or_default() {
            session.push_message(msg.clone());
        }

        self.events.set_state(AgentState::Done);

        Ok(final_text)
    }

    /// 执行安全审计（单文件模式）
    ///
    /// 使用受限工具集（仅 `semgrep`、`dependency_checker`、`nvd_lookup`），
    /// 代码已内联在 prompt 中，不需要文件操作和命令执行工具。
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

        // 单文件审计使用受限工具集
        let mut audit_tools = tools::audit_tools();
        if let Some(registry) = &self.skill_registry {
            audit_tools.push(Box::new(skill_tool::UseSkillTool::new(Arc::clone(
                registry,
            ))));
        }
        let mut executor = ReActExecutor::new(&self.llm, &audit_tools);

        // 1. 系统 prompt
        executor.push_message(ChatMessage::system(prompt::SYSTEM_PROMPT));

        // 2. 规划阶段
        self.events.set_state(AgentState::Planning);
        executor.push_message(ChatMessage::user(prompt::planning_prompt(code, language)));

        let plan = self.step_until_text(&mut executor, "规划阶段").await?;
        self.events.notify_think(&plan);

        // 3. 策略推理循环
        let mut strat = self
            .config
            .reasoning_strategy
            .parse::<strategy::StrategyKind>()
            .unwrap_or_default()
            .build();
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
