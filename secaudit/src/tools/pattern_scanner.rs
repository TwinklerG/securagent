//! 漏洞模式匹配工具 — 基于可配置规则集扫描代码中的已知漏洞模式。

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_CODE: &str = "code";
const PARAM_LANGUAGE: &str = "language";

// —— 文件扩展名 ——

const TOML_EXTENSION: &str = "toml";

/// 漏洞模式规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VulnRule {
    /// 规则 ID
    pub id: String,
    /// CWE 编号
    pub cwe_id: String,
    /// 漏洞名称
    pub name: String,
    /// 严重度（high/medium/low）
    pub severity: String,
    /// 匹配的正则模式
    pub pattern: String,
    /// 适用语言列表
    pub languages: Vec<String>,
    /// 描述
    pub description: String,
}

/// 单次匹配结果
#[derive(Debug, Serialize)]
struct MatchResult {
    rule_id: String,
    cwe_id: String,
    name: String,
    severity: String,
    matched_text: String,
    line: usize,
    description: String,
}

/// TOML 规则文件的顶层结构
#[derive(Deserialize)]
struct RulesFile {
    /// 规则列表
    rules: Vec<VulnRule>,
}

/// 从指定目录加载所有 `.toml` 规则文件。
///
/// # Errors
///
/// 文件读取或 TOML 解析失败时返回 [`Error::Tool`]。
fn load_rules_from_dir(dir: &Path) -> Result<Vec<VulnRule>, Error> {
    let mut rules = Vec::new();

    if !dir.is_dir() {
        return Ok(rules); // 目录不存在时返回空列表
    }

    let entries = fs::read_dir(dir).map_err(|e| Error::Tool(format!("读取规则目录失败：{e}")))?;

    for entry in entries {
        let entry = entry.map_err(|e| Error::Tool(format!("读取目录条目失败：{e}")))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some(TOML_EXTENSION) {
            continue;
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| Error::Tool(format!("读取规则文件 {} 失败：{e}", path.display())))?;

        let file: RulesFile = toml::from_str(&content)
            .map_err(|e| Error::Tool(format!("解析规则文件 {} 失败：{e}", path.display())))?;

        rules.extend(file.rules);
    }

    Ok(rules)
}

/// 漏洞模式扫描工具
pub struct PatternScanner {
    rules: Vec<VulnRule>,
}

impl PatternScanner {
    /// 使用内置默认规则集创建扫描器。
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// 从指定目录加载外部规则，并合并内置默认规则。
    ///
    /// # Errors
    ///
    /// 规则文件加载失败时返回错误。
    pub fn with_rules_dir(dir: &Path) -> Result<Self, Error> {
        let rules = load_rules_from_dir(dir)?;
        Ok(Self { rules })
    }
}

#[async_trait]
impl Tool for PatternScanner {
    fn name(&self) -> &'static str {
        "pattern_scanner"
    }

    fn description(&self) -> &'static str {
        "基于可配置规则集扫描代码中的已知漏洞模式，返回匹配到的安全风险列表"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_CODE: {
                    "type": "string",
                    "description": "待扫描的源代码"
                },
                PARAM_LANGUAGE: {
                    "type": "string",
                    "description": "编程语言（python/javascript/rust）"
                }
            },
            "required": [PARAM_CODE, PARAM_LANGUAGE]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let code = params
            .get(PARAM_CODE)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 code 参数".into()))?;

        let language = params
            .get(PARAM_LANGUAGE)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 language 参数".into()))?;

        let mut matches = Vec::new();

        for rule in &self.rules {
            // 跳过不适用当前语言的规则
            if !rule.languages.iter().any(|l| l == language) {
                continue;
            }

            let re = Regex::new(&rule.pattern)
                .map_err(|e| Error::Tool(format!("规则 {} 的正则编译失败：{e}", rule.id)))?;

            for m in re.find_iter(code) {
                // 计算匹配所在行号
                let line_num = code
                    .get(..m.start())
                    .map_or(1, |prefix| prefix.lines().count());

                matches.push(MatchResult {
                    rule_id: rule.id.clone(),
                    cwe_id: rule.cwe_id.clone(),
                    name: rule.name.clone(),
                    severity: rule.severity.clone(),
                    matched_text: m.as_str().to_owned(),
                    line: line_num,
                    description: rule.description.clone(),
                });
            }
        }

        let result = json!({
            "total": matches.len(),
            "matches": matches,
            "summary": format!(
                "扫描完成，发现 {} 个潜在漏洞（共 {} 条规则）",
                matches.len(),
                self.rules.len()
            )
        });

        serde_json::to_string_pretty(&result).map_err(|e| Error::Tool(e.to_string()))
    }
}
