//! Reflexion 策略 -- 在 `ReAct` 基础上累积反思记忆。
//!
//! 每轮 `ReAct` 执行后生成反思总结，下一轮将累积的反思作为额外上下文注入。
//! 为迭代三的记忆检索机制打基础。

use async_trait::async_trait;

use super::{Strategy, StrategyResult};
use crate::agent::EventBus;
use crate::agent::executor::{ReActExecutor, StepResult};
use crate::agent::state::AgentState;
use crate::config::Config;
use crate::error::Error;
use crate::llm::{ChatMessage, Role};
use crate::prompt;

/// 默认最大反思轮次
const DEFAULT_MAX_REFLECTIONS: u32 = 3;

/// 开始执行的用户指令
const EXECUTION_INSTRUCTION: &str = "请按照审计计划开始执行。使用可用的工具进行分析。";

/// 继续深入审计的用户指令
const CONTINUE_INSTRUCTION: &str =
    "基于以上反思，请继续深入审计。使用工具验证之前可能遗漏的安全问题。";

/// Reflexion 推理策略。
pub struct ReflexionStrategy {
    /// 累积的反思记录
    reflections: Vec<String>,
    /// 最大反思轮次
    max_reflections: u32,
}

impl ReflexionStrategy {
    /// 创建 Reflexion 策略实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            reflections: Vec::new(),
            max_reflections: DEFAULT_MAX_REFLECTIONS,
        }
    }
}

#[async_trait]
impl Strategy for ReflexionStrategy {
    async fn run(
        &mut self,
        executor: &mut ReActExecutor<'_>,
        events: &mut EventBus,
        config: &Config,
    ) -> Result<StrategyResult, Error> {
        let max_iter = config.max_iterations;
        let mut total_iterations = 0u32;

        for reflection_round in 0..self.max_reflections {
            // 注入累积反思到上下文
            if !self.reflections.is_empty() {
                let memory = self.reflections.join("\n---\n");
                executor.push_message(ChatMessage::user(prompt::reflexion_memory_prompt(&memory)));
            }

            if reflection_round == 0 {
                executor.push_message(ChatMessage::user(EXECUTION_INSTRUCTION));
            } else {
                executor.push_message(ChatMessage::user(CONTINUE_INSTRUCTION));
            }

            // 内层 ReAct 循环
            let mut iteration = 0u32;
            let mut has_tool_calls = false;
            let mut last_response = String::new();

            loop {
                if iteration >= max_iter {
                    break;
                }

                events.set_state(AgentState::Executing {
                    iteration: total_iterations,
                });
                let step = executor.step().await?;

                match step {
                    StepResult::ToolCalls(calls) => {
                        has_tool_calls = true;
                        for call in &calls {
                            events.notify_tool_call(&call.function.name, &call.function.arguments);
                        }
                        let _results = executor.execute_tool_calls(&calls).await?;
                        events.set_state(AgentState::Analyzing);
                        // 工具结果已作为 tool role 消息在历史中
                    }
                    StepResult::TextResponse(text) => {
                        events.notify_think(&text);

                        // 检测卡死循环
                        if text == last_response {
                            tracing::warn!("检测到重复响应，终止当前反思轮");
                            break;
                        }
                        last_response.clone_from(&text);

                        break;
                    }
                }

                iteration += 1;
                total_iterations += 1;
            }

            // 如果这一轮没有工具调用，说明 LLM 认为已无需进一步检查
            if !has_tool_calls && reflection_round > 0 {
                break;
            }

            // 生成本轮反思
            events.set_state(AgentState::Reflecting);
            executor.push_message(ChatMessage::user(prompt::reflexion_reflect_prompt(
                reflection_round,
            )));
            let max_reflection_tool_rounds = config.max_iterations.max(1);
            let mut reflection_tool_rounds = 0u32;
            loop {
                let reflection = executor.step().await?;
                match reflection {
                    StepResult::TextResponse(text) => {
                        events.notify_think(&text);
                        self.reflections.push(text);
                        break;
                    }
                    StepResult::ToolCalls(calls) => {
                        reflection_tool_rounds += 1;
                        tracing::warn!(
                            round = reflection_round,
                            tool_rounds = reflection_tool_rounds,
                            "反思生成阶段收到工具调用，自动执行后重试"
                        );
                        for call in &calls {
                            events.notify_tool_call(&call.function.name, &call.function.arguments);
                        }
                        let _results = executor.execute_tool_calls(&calls).await?;
                        events.set_state(AgentState::Analyzing);

                        if reflection_tool_rounds >= max_reflection_tool_rounds {
                            tracing::warn!("反思生成阶段达到工具调用上限，跳过本轮反思");
                            break;
                        }
                    }
                }
            }
        }

        // 返回最终总结
        let summary = executor
            .messages()
            .iter()
            .rev()
            .find_map(|m| {
                if matches!(m.role, Role::Assistant) {
                    m.content.as_deref().map(ToOwned::to_owned)
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Ok(StrategyResult {
            summary,
            iterations_used: total_iterations,
        })
    }
}
