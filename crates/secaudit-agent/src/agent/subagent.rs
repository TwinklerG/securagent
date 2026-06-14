//! 子 Agent 自动拉起与前置侦查。

use std::path::Path;

use crate::agent::executor::{LlmCallPolicy, ReActExecutor, StepResult};
use crate::error::Error;
use crate::llm::{ChatMessage, HttpLlmClient, TokenUsage};
use crate::tools::{SensitivePathPolicyConfig, Tool};

const SUBAGENT_NAME: &str = "recon";
const MAX_SUBAGENT_TOOL_ROUNDS: u32 = 6;
const MAX_SUBAGENT_SUMMARY_CHARS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentTrigger {
    pub(crate) name: String,
    pub(crate) reason: String,
}

#[must_use]
pub(crate) fn maybe_trigger(user_message: &str) -> Option<SubagentTrigger> {
    let lower = user_message.to_lowercase();
    let broad_keywords = [
        "审计这个项目",
        "审计项目",
        "分析这个项目",
        "扫描项目",
        "扫描仓库",
        "整个项目",
        "全局审计",
        "安全审计",
        "attack surface",
        "audit this project",
        "audit the project",
        "scan this repo",
        "scan the repo",
        "scan repository",
        "whole project",
        "entire project",
    ];

    broad_keywords
        .iter()
        .any(|keyword| lower.contains(keyword))
        .then(|| SubagentTrigger {
            name: SUBAGENT_NAME.to_owned(),
            reason: "检测到大范围项目审计请求，先自动拉起子 Agent 做侦查摘要".to_owned(),
        })
}

#[must_use]
pub(crate) fn build_tools(
    work_dir: &Path,
    sensitive_path_policy: SensitivePathPolicyConfig,
) -> Vec<Box<dyn Tool>> {
    let policy = secaudit_tools::SensitivePathPolicy::new(sensitive_path_policy);
    vec![
        Box::new(secaudit_tools::ListDirectory::with_sensitive_path_policy(
            work_dir.to_path_buf(),
            policy.clone(),
        )),
        Box::new(secaudit_tools::FindFiles::with_sensitive_path_policy(
            work_dir.to_path_buf(),
            policy.clone(),
        )),
        Box::new(secaudit_tools::SearchContent::with_sensitive_path_policy(
            work_dir.to_path_buf(),
            policy.clone(),
        )),
        Box::new(secaudit_tools::ReadFile::with_sensitive_path_policy(
            work_dir.to_path_buf(),
            policy.clone(),
        )),
        Box::new(secaudit_tools::SemgrepScanner::with_sensitive_path_policy(
            work_dir.to_path_buf(),
            policy.clone(),
        )),
        Box::new(
            secaudit_tools::DependencyChecker::with_sensitive_path_policy(
                work_dir.to_path_buf(),
                policy,
            ),
        ),
        Box::new(secaudit_tools::NvdLookup::new()),
    ]
}

pub(crate) async fn run_recon<F, U>(
    llm: &HttpLlmClient,
    tools: &[Box<dyn Tool>],
    llm_policy: LlmCallPolicy,
    work_dir: &Path,
    user_message: &str,
    mut on_delta: F,
    mut on_usage: U,
) -> Result<String, Error>
where
    F: FnMut(&str) + Send,
    U: FnMut(TokenUsage) + Send,
{
    let mut executor = ReActExecutor::with_llm_policy(llm, tools, llm_policy);
    executor.push_message(ChatMessage::system(system_prompt(work_dir)));
    executor.push_message(ChatMessage::user(task_prompt(user_message)));

    for _ in 0..MAX_SUBAGENT_TOOL_ROUNDS {
        match executor.step_stream(&mut on_delta, &mut on_usage).await? {
            StepResult::TextResponse(text) => return Ok(truncate_summary(&text)),
            StepResult::ToolCalls(calls) => {
                let _results = executor.execute_tool_calls(&calls).await?;
            }
        }
    }

    executor.push_message(ChatMessage::user(
        "工具轮次已到上限。请基于已收集到的信息输出侦查摘要，不要再调用工具。",
    ));
    match executor.step_stream(on_delta, on_usage).await? {
        StepResult::TextResponse(text) => Ok(truncate_summary(&text)),
        StepResult::ToolCalls(_) => {
            Ok("子 Agent 已达到侦查上限，但模型仍请求继续调用工具；本轮不注入摘要。".to_owned())
        }
    }
}

fn system_prompt(work_dir: &Path) -> String {
    format!(
        "你是 secaudit 的侦查子 Agent。你的职责是在主 Agent 开始大范围安全任务前，\
快速了解项目结构、技术栈、入口点、依赖与安全关键区域，并输出给主 Agent 使用的简洁摘要。\n\n\
约束：\n\
- 只能做只读侦查和安全扫描，不要修改文件，不要执行命令。\n\
- 优先使用 list_directory、find_files、search_content、read_file、dependency_checker、semgrep_scanner。\n\
- 输出必须聚焦：技术栈、关键目录/文件、攻击面、建议主 Agent 深入检查的文件。\n\n\
当前工作目录：{}",
        work_dir.display()
    )
}

fn task_prompt(user_message: &str) -> String {
    format!(
        "用户原始请求：\n{user_message}\n\n\
请先为主 Agent 完成项目侦查。输出 Markdown 摘要，包含：\n\
1. 项目类型与主要技术栈\n\
2. 关键入口点、配置文件和安全敏感模块\n\
3. 初步依赖/静态扫描信号\n\
4. 建议主 Agent 接下来优先审计的 3-6 个区域"
    )
}

fn truncate_summary(summary: &str) -> String {
    if summary.chars().count() <= MAX_SUBAGENT_SUMMARY_CHARS {
        return summary.to_owned();
    }

    let mut truncated = summary
        .chars()
        .take(MAX_SUBAGENT_SUMMARY_CHARS)
        .collect::<String>();
    truncated.push_str("\n\n[子 Agent 摘要已截断，输出过长。]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::maybe_trigger;

    #[test]
    fn triggers_for_broad_project_audit() {
        assert!(maybe_trigger("请审计这个项目的安全风险").is_some());
        assert!(maybe_trigger("scan this repo for security bugs").is_some());
    }

    #[test]
    fn does_not_trigger_for_narrow_file_question() {
        assert!(maybe_trigger("分析 src/main.rs 第 10 行").is_none());
    }
}
