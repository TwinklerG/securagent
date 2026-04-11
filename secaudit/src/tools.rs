mod code_parser;
mod cwe_knowledge_base;
mod dependency_checker;
mod nvd_lookup;
mod pattern_scanner;
mod semgrep_scanner;

use async_trait::async_trait;

pub use code_parser::CodeParser;
pub use cwe_knowledge_base::CweKnowledgeBase;
pub use dependency_checker::DependencyChecker;
pub use nvd_lookup::NvdLookup;
pub use pattern_scanner::PatternScanner;
pub use semgrep_scanner::SemgrepScanner;

use crate::config::Config;
use crate::error::Error;

/// 工具 trait，定义 Agent 可调用的外部能力接口。
///
/// 每个工具需提供名称、描述和参数 schema，供 LLM 理解其用途并正确调用。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称（唯一标识）
    fn name(&self) -> &'static str;

    /// 工具描述（供 LLM 理解用途）
    fn description(&self) -> &'static str;

    /// 工具参数的 JSON Schema
    fn parameters_schema(&self) -> serde_json::Value;

    /// 执行工具，返回文本结果
    ///
    /// # Errors
    ///
    /// 工具执行失败时返回 [`Error::Tool`]。
    async fn execute(&self, params: &serde_json::Value) -> Result<String, Error>;
}

/// 创建默认工具集，包含所有内置安全审计工具。
///
/// 优先从配置的规则目录加载外部规则；加载失败时回退到内置默认规则。
pub fn default_tools(config: &Config) -> Vec<Box<dyn Tool>> {
    let scanner = PatternScanner::with_rules_dir(&config.rules_dir).unwrap_or_else(|e| {
        tracing::warn!("加载外部规则失败，使用默认规则：{e}");
        PatternScanner::new()
    });

    vec![
        Box::new(CodeParser::new()),
        Box::new(scanner),
        Box::new(DependencyChecker),
        Box::new(CweKnowledgeBase::new()),
        Box::new(SemgrepScanner::new()),
        Box::new(NvdLookup::new()),
    ]
}
