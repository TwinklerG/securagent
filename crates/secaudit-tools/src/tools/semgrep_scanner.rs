//! Semgrep 扫描工具 — 调用本地 Semgrep CLI 执行静态安全分析。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::io::ErrorKind;
use tokio::process::Command;

use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_PROJECT_PATH: &str = "project_path";
const PARAM_RULESET: &str = "ruleset";

// —— 工具元信息 ——

const TOOL_NAME: &str = "semgrep_scanner";
const TOOL_DESC: &str =
    "调用 Semgrep 静态分析工具扫描项目代码，支持 OWASP Top 10、语言特定等多种规则集";

// —— 默认规则集 ——

const DEFAULT_RULESET: &str = "p/owasp-top-ten";

// —— 命令 ——

const CMD_SEMGREP: &str = "semgrep";

/// Semgrep 扫描结果条目
#[derive(Debug, Deserialize)]
struct SemgrepResult {
    /// 检测结果列表
    #[serde(default)]
    results: Vec<SemgrepFinding>,
}

/// 单条检测发现
#[derive(Debug, Deserialize)]
struct SemgrepFinding {
    /// 规则 ID
    check_id: String,
    /// 匹配的代码片段
    #[serde(default)]
    extra: SemgrepExtra,
    /// 起始位置
    start: SemgrepPosition,
    /// 文件路径
    path: String,
}

/// Semgrep 额外信息
#[derive(Debug, Default, Deserialize)]
struct SemgrepExtra {
    /// 规则描述
    #[serde(default)]
    message: String,
    /// 严重度
    #[serde(default)]
    severity: String,
    /// CWE 标签
    #[serde(default)]
    metadata: SemgrepMetadata,
}

/// Semgrep 元数据
#[derive(Debug, Default, Deserialize)]
struct SemgrepMetadata {
    /// CWE 编号列表
    #[serde(default)]
    cwe: Vec<String>,
}

/// Semgrep 位置
#[derive(Debug, Deserialize)]
struct SemgrepPosition {
    /// 行号
    line: u32,
}

/// Semgrep 静态安全扫描工具
pub struct SemgrepScanner;

impl SemgrepScanner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for SemgrepScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SemgrepScanner {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(TOOL_NAME)
    }

    fn description(&self) -> Cow<'_, str> {
        Cow::Borrowed(TOOL_DESC)
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_PROJECT_PATH: {
                    "type": "string",
                    "description": "待扫描的项目路径或文件路径"
                },
                PARAM_RULESET: {
                    "type": "string",
                    "description": "Semgrep 规则集（如 p/owasp-top-ten、p/python、p/javascript），默认为 p/owasp-top-ten"
                }
            },
            "required": [PARAM_PROJECT_PATH]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let project_path = params
            .get(PARAM_PROJECT_PATH)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 project_path 参数".into()))?;

        let ruleset = params
            .get(PARAM_RULESET)
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_RULESET);

        // 调用 semgrep CLI
        let output = Command::new(CMD_SEMGREP)
            .args([
                "scan",
                "--config",
                ruleset,
                "--json",
                "--quiet",
                project_path,
            ])
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);

                // 尝试解析 JSON 输出
                if let Ok(result) = serde_json::from_str::<SemgrepResult>(&stdout) {
                    let findings: Vec<Value> = result
                        .results
                        .iter()
                        .map(|f| {
                            json!({
                                "rule_id": f.check_id,
                                "severity": f.extra.severity,
                                "message": f.extra.message,
                                "file": f.path,
                                "line": f.start.line,
                                "cwe": f.extra.metadata.cwe,
                            })
                        })
                        .collect();

                    let summary = json!({
                        "total": findings.len(),
                        "ruleset": ruleset,
                        "findings": findings,
                        "summary": format!(
                            "Semgrep 扫描完成（规则集：{ruleset}），发现 {} 个安全问题",
                            findings.len()
                        )
                    });

                    serde_json::to_string_pretty(&summary).map_err(|e| Error::Tool(e.to_string()))
                } else {
                    // JSON 解析失败，返回原始输出
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Ok(format!("Semgrep 输出:\n{stdout}\n{stderr}"))
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                Ok("Semgrep 未安装。请先安装：pip install semgrep 或 brew install semgrep".into())
            }
            Err(e) => Err(Error::Tool(format!("执行 Semgrep 失败：{e}"))),
        }
    }
}
