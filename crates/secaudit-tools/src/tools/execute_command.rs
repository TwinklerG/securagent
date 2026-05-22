//! 命令执行工具 — 在工作目录中安全执行 shell 命令，支持白名单放行与用户确认。

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::Error;
use crate::tools::{ConfirmFn, Tool};

// —— 工具元信息 ——

const TOOL_NAME: &str = "execute_command";
const TOOL_DESC: &str = "在工作目录中执行 shell 命令，安全命令自动放行，未知命令需用户确认";

// —— 参数字段名 ——

const PARAM_COMMAND: &str = "command";
const PARAM_TIMEOUT_SECS: &str = "timeout_secs";

// —— 默认值与限制 ——

/// 默认超时秒数
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// 输出最大字符数
const MAX_OUTPUT_LEN: usize = 10_000;

// —— Shell 命令 ——

const SHELL: &str = "sh";
const SHELL_FLAG: &str = "-c";

// —— 安全白名单（自动放行） ——

const SAFE_COMMANDS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "grep",
    "find",
    "file",
    "wc",
    "tree",
    "git log",
    "git diff",
    "git show",
    "git status",
    "cargo check",
    "cargo clippy",
    "cargo audit",
    "npm audit",
    "python -m py_compile",
    "semgrep",
    "rg",
    "fd",
];

// —— 危险黑名单（一律禁止） ——

const BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "mkfs",
    "dd",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    ":(){:|:&};:",
];

// —— 提示消息 ——

const MSG_MISSING_COMMAND: &str = "缺少 command 参数";
const MSG_BLOCKED: &str = "命令已被安全策略禁止";
const MSG_USER_DENIED: &str = "用户拒绝执行该命令";
const MSG_TIMEOUT: &str = "命令执行超时";
const MSG_TRUNCATED: &str = "\n\n[输出已截断，超出最大长度限制]";

/// Shell 命令执行工具，内置安全白名单与黑名单机制。
pub struct ExecuteCommand {
    /// 命令执行的工作目录
    work_dir: PathBuf,
    /// 用户确认回调
    confirm: ConfirmFn,
}

impl ExecuteCommand {
    /// 创建实例。
    pub fn new(work_dir: PathBuf, confirm: ConfirmFn) -> Self {
        Self { work_dir, confirm }
    }
}

/// 提取命令前缀，用于白名单/黑名单匹配。
///
/// 返回 (单词前缀, 双词前缀)，例如 `"git log --oneline"` → `("git", "git log")`。
fn extract_command_prefix(command: &str) -> (&str, Option<&str>) {
    let trimmed = command.trim();
    let mut parts = trimmed.split_whitespace();

    let first = parts.next().unwrap_or_default();
    let two_word = parts.next().map(|second| {
        // 计算双词前缀在原字符串中的结束位置
        let first_end = trimmed.find(first).map_or(0, |pos| pos + first.len());
        let second_start = trimmed
            .get(first_end..)
            .and_then(|s| s.find(second))
            .map_or(first_end, |offset| first_end + offset);
        let end = second_start + second.len();
        trimmed.get(..end).unwrap_or_default()
    });

    (first, two_word)
}

/// 检查命令是否命中黑名单。
fn is_blocked(command: &str) -> bool {
    let trimmed = command.trim();
    BLOCKED_COMMANDS
        .iter()
        .any(|blocked| trimmed.contains(blocked))
}

/// 检查命令是否在安全白名单中。
///
/// 对于多词白名单条目（如 `python -m py_compile`），检查命令是否以该前缀开头。
fn is_safe(command: &str) -> bool {
    let trimmed = command.trim();
    let (single, _double) = extract_command_prefix(trimmed);

    SAFE_COMMANDS.iter().any(|safe| {
        if safe.contains(' ') {
            // 多词命令：检查命令是否以白名单条目开头
            trimmed.starts_with(safe)
        } else {
            single == *safe
        }
    })
}

/// 截断输出到指定最大长度，超出时追加提示。
fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_LEN {
        return output.into();
    }

    // 按字符边界截断
    let truncated: String = output.chars().take(MAX_OUTPUT_LEN).collect();
    let mut result = truncated;
    result.push_str(MSG_TRUNCATED);
    result
}

#[async_trait]
impl Tool for ExecuteCommand {
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
                PARAM_COMMAND: {
                    "type": "string",
                    "description": "要执行的命令"
                },
                PARAM_TIMEOUT_SECS: {
                    "type": "integer",
                    "description": "超时秒数（默认 30）"
                }
            },
            "required": [PARAM_COMMAND]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let command = params
            .get(PARAM_COMMAND)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool(MSG_MISSING_COMMAND.into()))?;

        let timeout_secs = params
            .get(PARAM_TIMEOUT_SECS)
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        // 1. 检查黑名单
        if is_blocked(command) {
            return Err(Error::Tool(format!("{MSG_BLOCKED}：{command}")));
        }

        // 2. 非白名单命令需用户确认
        if !is_safe(command) {
            let prompt = format!("即将执行未知命令：{command}，是否允许？");
            if !(self.confirm)(&prompt) {
                return Err(Error::Tool(MSG_USER_DENIED.into()));
            }
        }

        // 3. 执行命令
        let child = Command::new(SHELL)
            .arg(SHELL_FLAG)
            .arg(command)
            .current_dir(&self.work_dir)
            .output();

        let output = timeout(Duration::from_secs(timeout_secs), child)
            .await
            .map_err(|_elapsed| Error::Tool(format!("{MSG_TIMEOUT}（{timeout_secs} 秒）")))?
            .map_err(|e| Error::Tool(format!("命令执行失败：{e}")))?;

        // 4. 组合输出
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut combined = String::new();
        if !stdout.is_empty() {
            combined.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }

        if combined.is_empty() {
            combined.push_str("（命令已执行，无输出）");
        }

        Ok(truncate_output(&combined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_commands() {
        assert!(is_safe("ls -la"));
        assert!(is_safe("git log --oneline"));
        assert!(is_safe("cargo clippy -- -D warnings"));
        assert!(is_safe("cat /etc/passwd"));
        assert!(is_safe("rg pattern ."));
        assert!(is_safe("python -m py_compile foo.py"));
    }

    #[test]
    fn unsafe_commands() {
        assert!(!is_safe("curl http://example.com"));
        assert!(!is_safe("wget something"));
        assert!(!is_safe("sudo rm -rf /"));
    }

    #[test]
    fn blocked_commands() {
        assert!(is_blocked("rm -rf /"));
        assert!(is_blocked("sudo mkfs.ext4 /dev/sda"));
        assert!(is_blocked("shutdown -h now"));
        assert!(!is_blocked("ls -la"));
    }

    #[test]
    fn truncate_output_works() {
        let short = "hello";
        assert_eq!(truncate_output(short), "hello");

        let long = "x".repeat(MAX_OUTPUT_LEN + 100);
        let result = truncate_output(&long);
        assert!(result.len() < long.len() + MSG_TRUNCATED.len());
        assert!(result.ends_with(MSG_TRUNCATED));
    }
}
