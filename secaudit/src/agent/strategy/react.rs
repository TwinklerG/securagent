//! `ReAct` 策略 -- 标准的 Observe-Reason-Act 循环。

use async_trait::async_trait;

use super::{Strategy, StrategyResult};
use crate::agent::EventBus;
use crate::agent::executor::{ReActExecutor, StepResult};
use crate::agent::state::AgentState;
use crate::config::Config;
use crate::error::Error;
use crate::llm::{ChatMessage, Role};

/// 开始执行的用户指令
const EXECUTION_INSTRUCTION: &str = "请按照审计计划开始执行。使用可用的工具进行分析。";

/// 连续无工具调用的空循环阈值（超过则提前终止）
const MAX_EMPTY_ROUNDS: u32 = 2;

/// `ReAct` 推理策略。
pub struct ReactStrategy;

#[async_trait]
impl Strategy for ReactStrategy {
    async fn run(
        &mut self,
        executor: &mut ReActExecutor<'_>,
        events: &mut EventBus,
        config: &Config,
    ) -> Result<StrategyResult, Error> {
        let max_iter = config.max_iterations;
        let mut iteration = 0u32;
        let mut empty_rounds = 0u32;
        let mut last_response = String::new();

        executor.push_message(ChatMessage::user(EXECUTION_INSTRUCTION));

        loop {
            if iteration >= max_iter {
                break;
            }

            events.set_state(AgentState::Executing { iteration });

            let step = executor.step().await?;

            match step {
                StepResult::ToolCalls(calls) => {
                    empty_rounds = 0;
                    for call in &calls {
                        events.notify_tool_call(&call.function.name, &call.function.arguments);
                    }
                    let _results = executor.execute_tool_calls(&calls).await?;
                    events.set_state(AgentState::Analyzing);
                    // 工具结果已作为 tool role 消息在历史中，LLM 能直接看到
                }
                StepResult::TextResponse(text) => {
                    events.notify_think(&text);

                    // 检测卡死循环：LLM 输出与上一轮相同
                    if text == last_response {
                        tracing::warn!("检测到重复响应，提前终止 ReAct 循环");
                        break;
                    }
                    last_response.clone_from(&text);

                    empty_rounds += 1;
                    if empty_rounds >= MAX_EMPTY_ROUNDS {
                        tracing::info!("连续 {empty_rounds} 轮无工具调用，提前终止");
                        break;
                    }

                    // 如果是首次无工具调用，可能是 LLM 在总结，直接结束
                    if empty_rounds == 1 {
                        break;
                    }
                }
            }

            iteration += 1;
        }

        // 返回最后一条 assistant 消息作为小结
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
            iterations_used: iteration,
        })
    }
}
