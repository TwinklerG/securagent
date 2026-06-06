//! `ReAct` 策略 -- 标准的 Observe-Reason-Act 循环。

use async_trait::async_trait;

use super::{Strategy, StrategyResult};
use crate::agent::events::EventBus;
use crate::agent::executor::{ReActExecutor, StepResult};
use crate::agent::state::AgentState;
use crate::config::Config;
use crate::error::Error;
use crate::llm::ChatMessage;

/// 开始执行的用户指令
const EXECUTION_INSTRUCTION: &str = "\
请按照审计计划，逐一分析代码中的安全问题。\
对每个发现给出 CWE 编号、严重度、位置和修复建议。\
如需运行 semgrep 扫描可调用工具，否则直接分析即可。";

/// 连续无工具调用的空循环阈值（超过则提前终止）
const MAX_EMPTY_ROUNDS: u32 = 2;

/// 接近迭代上限时注入催促 prompt 的剩余轮次阈值
const WRAP_UP_THRESHOLD: u32 = 3;

/// 催促 LLM 收尾的提示消息
const MSG_WRAP_UP: &str = "\
你即将达到工具调用轮次上限。请立即总结你的审计发现并给出最终报告，不要再调用工具。";

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

            // 接近上限时注入催促 prompt，引导 LLM 收尾
            if iteration + WRAP_UP_THRESHOLD == max_iter {
                executor.push_message(ChatMessage::user(MSG_WRAP_UP));
            }

            events.set_state(AgentState::Executing { iteration });

            let step = executor.step().await?;

            match step {
                StepResult::ToolCalls(calls) => {
                    empty_rounds = 0;
                    events.notify_tool_calls(&calls);
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

        Ok(StrategyResult {
            iterations_used: iteration,
        })
    }
}
