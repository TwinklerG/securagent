//! 依赖漏洞检查工具 — 调用包管理审计工具检查项目依赖中的已知 CVE。

use async_trait::async_trait;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::error::Error;
use crate::tools::Tool;

use super::sandbox::{SensitivePathPolicy, resolve_existing_path};

// —— 参数字段名 ——

const PARAM_PROJECT_PATH: &str = "project_path";

// —— 工具元信息 ——

const TOOL_NAME: &str = "dependency_checker";
const TOOL_DESC: &str = "检查项目依赖中的已知 CVE 漏洞，支持 Cargo、npm 和 pip 项目";

// —— 包管理锁文件 ——

const CARGO_LOCK: &str = "Cargo.lock";
const PACKAGE_LOCK: &str = "package-lock.json";
const REQUIREMENTS_TXT: &str = "requirements.txt";

// —— 审计命令 ——

const CMD_CARGO: &str = "cargo";
const CMD_NPM: &str = "npm";
const CMD_PIP_AUDIT: &str = "pip-audit";

/// 依赖漏洞检查工具
pub struct DependencyChecker {
    work_dir: PathBuf,
    sensitive_paths: SensitivePathPolicy,
}

impl DependencyChecker {
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        Self::with_sensitive_path_policy(work_dir, SensitivePathPolicy::default())
    }

    #[must_use]
    pub fn with_sensitive_path_policy(
        work_dir: PathBuf,
        sensitive_paths: SensitivePathPolicy,
    ) -> Self {
        Self {
            work_dir,
            sensitive_paths,
        }
    }
}

impl Default for DependencyChecker {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}

/// 运行外部审计命令，返回输出文本。
async fn run_audit(program: &str, args: &[&str], dir: &Path) -> Result<String, Error> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stdout.is_empty() && stderr.is_empty() {
                Ok("审计完成，未发现已知漏洞".into())
            } else {
                // 审计工具通常以非零退出码表示发现漏洞，仍需返回输出
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }
                Ok(result)
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Ok(format!("审计工具 `{program}` 未安装，请先安装后再试"))
        }
        Err(e) => Err(Error::Tool(format!("执行 `{program}` 失败：{e}"))),
    }
}

#[async_trait]
impl Tool for DependencyChecker {
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
                    "description": "项目根目录路径"
                }
            },
            "required": [PARAM_PROJECT_PATH]
        })
    }

    async fn precheck(&self, params: &Value) -> Result<(), String> {
        let project_path = params
            .get(PARAM_PROJECT_PATH)
            .and_then(Value::as_str)
            .ok_or_else(|| "缺少 project_path 参数".to_owned())?;

        let dir = resolve_existing_path(&self.work_dir, project_path).map_err(|e| e.to_string())?;
        if !dir.is_dir() {
            return Err(format!("路径不存在或不是目录：{project_path}"));
        }
        if self.sensitive_paths.has_sensitive_component(&dir) {
            return Err(format!("拒绝扫描敏感目录：{}", dir.display()));
        }

        Ok(())
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        self.precheck(params).await.map_err(Error::Tool)?;

        let project_path = params
            .get(PARAM_PROJECT_PATH)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 project_path 参数".into()))?;

        let dir = resolve_existing_path(&self.work_dir, project_path)?;
        if !dir.is_dir() {
            return Err(Error::Tool(format!("路径不存在或不是目录：{project_path}")));
        }

        let mut results = Vec::new();

        // 检测 Rust 项目
        if dir.join(CARGO_LOCK).exists() {
            let output = run_audit(CMD_CARGO, &["audit", "--json"], &dir).await?;
            results.push(json!({
                "ecosystem": "rust",
                "tool": "cargo audit",
                "output": output
            }));
        }

        // 检测 Node.js 项目
        if dir.join(PACKAGE_LOCK).exists() {
            let output = run_audit(CMD_NPM, &["audit", "--json"], &dir).await?;
            results.push(json!({
                "ecosystem": "nodejs",
                "tool": "npm audit",
                "output": output
            }));
        }

        // 检测 Python 项目
        if dir.join(REQUIREMENTS_TXT).exists() {
            let output = run_audit(
                CMD_PIP_AUDIT,
                &["--format", "json", "-r", REQUIREMENTS_TXT],
                &dir,
            )
            .await?;
            results.push(json!({
                "ecosystem": "python",
                "tool": "pip-audit",
                "output": output
            }));
        }

        if results.is_empty() {
            return Ok(json!({
                "summary": "未检测到支持的包管理文件（Cargo.lock / package-lock.json / requirements.txt）"
            })
            .to_string());
        }

        let result = json!({
            "scanned": results.len(),
            "results": results
        });

        serde_json::to_string_pretty(&result).map_err(|e| Error::Tool(e.to_string()))
    }
}
