mod dependency_checker;
mod execute_command;
mod find_files;
mod list_directory;
mod nvd_lookup;
mod read_file;
mod search_content;
mod semgrep_scanner;
pub(crate) mod shared;
mod write_file;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

pub use dependency_checker::DependencyChecker;
pub use execute_command::ExecuteCommand;
pub use find_files::FindFiles;
pub use list_directory::ListDirectory;
pub use nvd_lookup::NvdLookup;
pub use read_file::ReadFile;
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

    /// 执行工具，返回文本结果
    ///
    /// # Errors
    ///
    /// 工具执行失败时返回 [`Error::Tool`]。
    async fn execute(&self, params: &serde_json::Value) -> Result<String, Error>;
}

/// 创建默认工具集（交互模式），包含通用文件操作工具和安全专用工具。
pub fn default_tools(work_dir: PathBuf, confirm: ConfirmFn) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadFile::new(work_dir.clone())),
        Box::new(ListDirectory::new(work_dir.clone())),
        Box::new(SearchContent::new(work_dir.clone())),
        Box::new(FindFiles::new(work_dir.clone())),
        Box::new(ExecuteCommand::new(work_dir.clone(), Arc::clone(&confirm))),
        Box::new(WriteFile::new(work_dir, confirm)),
    ];
    tools.extend(security_audit_tools());
    tools
}

/// 创建单文件审计专用工具集 — 仅包含只读分析工具。
///
/// 单文件审计时代码已内联在 prompt 中，不需要文件操作和命令执行工具。
/// 限制工具集可避免 LLM 写入无关文件、安装软件等浪费行为。
#[must_use]
pub fn audit_tools() -> Vec<Box<dyn Tool>> {
    security_audit_tools()
}

/// 创建安全审计专用工具集（无文件写入/命令执行）。
fn security_audit_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SemgrepScanner::new()),
        Box::new(DependencyChecker),
        Box::new(NvdLookup::new()),
    ]
}
