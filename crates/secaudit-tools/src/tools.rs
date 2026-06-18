mod dependency_checker;
mod execute_command;
mod file_probe;
mod find_files;
mod list_directory;
mod nvd_lookup;
mod process;
mod read_file;
mod sandbox;
mod search_content;
mod semgrep_scanner;
mod write_file;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

pub use dependency_checker::DependencyChecker;
pub use execute_command::{CommandPolicyConfig, ExecuteCommand};
pub use find_files::FindFiles;
pub use list_directory::ListDirectory;
pub use nvd_lookup::NvdLookup;
pub use read_file::ReadFile;
pub use sandbox::{SensitivePathPolicy, SensitivePathPolicyConfig};
pub use search_content::SearchContent;
pub use semgrep_scanner::SemgrepScanner;
pub use write_file::WriteFile;

use crate::error::Error;

/// 确认回调：Agent 在执行危险操作前调用，返回 true 表示用户同意。
pub type ConfirmFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 工具 trait，定义 Agent 可调用的外部能力接口。
///
/// 每个工具需提供名称、描述和参数 schema，供 LLM 理解其用途并正确调用。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称（唯一标识）
    fn name(&self) -> Cow<'_, str>;

    /// 工具描述（供 LLM 理解用途）
    fn description(&self) -> Cow<'_, str>;

    /// 工具参数的 JSON Schema
    fn parameters_schema(&self) -> serde_json::Value;

    /// 执行工具前的轻量前置校验。
    ///
    /// 默认实现允许执行；高风险工具可覆盖该方法，在真正执行前拒绝危险参数。
    async fn precheck(&self, _params: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// 执行工具，返回文本结果
    ///
    /// # Errors
    ///
    /// 工具执行失败时返回 [`Error::Tool`]。
    async fn execute(&self, params: &serde_json::Value) -> Result<String, Error>;
}

/// 创建默认工具集（交互模式），包含通用文件操作工具和安全专用工具。
pub fn default_tools(work_dir: PathBuf, confirm: ConfirmFn) -> Vec<Box<dyn Tool>> {
    default_tools_with_command_policy(work_dir, confirm, CommandPolicyConfig::default())
}

/// 创建默认工具集，并注入用户配置的命令安全策略。
pub fn default_tools_with_command_policy(
    work_dir: PathBuf,
    confirm: ConfirmFn,
    command_policy: CommandPolicyConfig,
) -> Vec<Box<dyn Tool>> {
    default_tools_with_policies(
        work_dir,
        confirm,
        command_policy,
        SensitivePathPolicyConfig::default(),
    )
}

/// 创建默认工具集，并注入用户配置的命令与路径安全策略。
pub fn default_tools_with_policies(
    work_dir: PathBuf,
    confirm: ConfirmFn,
    command_policy: CommandPolicyConfig,
    sensitive_path_policy: SensitivePathPolicyConfig,
) -> Vec<Box<dyn Tool>> {
    let sensitive_path_policy = SensitivePathPolicy::new(sensitive_path_policy);
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadFile::with_sensitive_path_policy(
            work_dir.clone(),
            sensitive_path_policy.clone(),
        )),
        Box::new(ListDirectory::with_sensitive_path_policy(
            work_dir.clone(),
            sensitive_path_policy.clone(),
        )),
        Box::new(SearchContent::with_sensitive_path_policy(
            work_dir.clone(),
            sensitive_path_policy.clone(),
        )),
        Box::new(FindFiles::with_sensitive_path_policy(
            work_dir.clone(),
            sensitive_path_policy.clone(),
        )),
        Box::new(ExecuteCommand::with_policy(
            work_dir.clone(),
            Arc::clone(&confirm),
            command_policy,
        )),
        Box::new(WriteFile::with_sensitive_path_policy(
            work_dir.clone(),
            confirm,
            sensitive_path_policy.clone(),
        )),
    ];
    tools.extend(security_audit_tools_with_sensitive_path_policy(
        work_dir,
        sensitive_path_policy,
    ));
    tools
}

/// 创建单文件审计专用工具集 — 仅包含只读分析工具。
///
/// 单文件审计时代码已内联在 prompt 中，不需要文件操作和命令执行工具。
/// 限制工具集可避免 LLM 写入无关文件、安装软件等浪费行为。
#[must_use]
pub fn audit_tools(work_dir: PathBuf) -> Vec<Box<dyn Tool>> {
    security_audit_tools(work_dir)
}

/// 创建单文件审计专用工具集，并注入用户配置的路径安全策略。
#[must_use]
pub fn audit_tools_with_sensitive_path_policy(
    work_dir: PathBuf,
    sensitive_path_policy: SensitivePathPolicyConfig,
) -> Vec<Box<dyn Tool>> {
    security_audit_tools_with_sensitive_path_policy(
        work_dir,
        SensitivePathPolicy::new(sensitive_path_policy),
    )
}

/// 创建安全审计专用工具集（无文件写入/命令执行）。
fn security_audit_tools(work_dir: PathBuf) -> Vec<Box<dyn Tool>> {
    security_audit_tools_with_sensitive_path_policy(work_dir, SensitivePathPolicy::default())
}

fn security_audit_tools_with_sensitive_path_policy(
    work_dir: PathBuf,
    sensitive_path_policy: SensitivePathPolicy,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SemgrepScanner::with_sensitive_path_policy(
            work_dir.clone(),
            sensitive_path_policy.clone(),
        )),
        Box::new(DependencyChecker::with_sensitive_path_policy(
            work_dir,
            sensitive_path_policy,
        )),
        Box::new(NvdLookup::new()),
    ]
}
