//! 文件读取工具 — 读取指定文件内容并附加行号，支持行范围选取。

use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

use super::shared::resolve_sandbox_path;
use crate::error::Error;
use crate::tools::Tool;

// —— 工具元信息 ——

const TOOL_NAME: &str = "read_file";
const TOOL_DESC: &str = "读取文件内容，支持指定行范围，返回带行号的文本";

// —— 参数字段名 ——

const PARAM_PATH: &str = "path";
const PARAM_OFFSET: &str = "offset";
const PARAM_LIMIT: &str = "limit";

// —— 默认值 ——

/// 默认起始行号（1-based）
const DEFAULT_OFFSET: usize = 1;

/// 默认读取行数
const DEFAULT_LIMIT: usize = 2000;

/// 行号显示宽度
const LINE_NUMBER_WIDTH: usize = 6;

// —— 提示消息 ——

const MSG_MISSING_PATH: &str = "缺少 path 参数";

/// 文件读取工具，支持沙箱路径校验与行范围选取。
pub struct ReadFile {
    /// 沙箱工作目录
    work_dir: PathBuf,
}

impl ReadFile {
    /// 创建实例，`work_dir` 为沙箱根目录。
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }
}

/// 格式化带行号的文件内容。
///
/// 对 `lines` 中从 `offset`（1-based）开始、最多 `limit` 行进行编号格式化。
/// 若文件总行数超出 `limit`，在末尾附加截断提示。
fn format_lines(lines: &[&str], offset: usize, limit: usize) -> String {
    let total = lines.len();
    // offset 为 1-based，转换为 0-based 索引；保护越界
    let start_idx = offset.saturating_sub(1).min(total);
    let end_idx = (start_idx + limit).min(total);

    let mut output = String::new();
    for (i, line) in lines.get(start_idx..end_idx).into_iter().flatten().enumerate() {
        let line_no = start_idx + i + 1;
        let _ = writeln!(output, "{line_no:>LINE_NUMBER_WIDTH$}\t{line}");
    }

    // 截断提示
    if end_idx < total || start_idx > 0 {
        let display_start = start_idx + 1;
        let _ = write!(
            output,
            "\n[... 文件共 {total} 行，已显示 {display_start}~{end_idx} 行]"
        );
    }

    output
}

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn description(&self) -> &'static str {
        TOOL_DESC
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_PATH: {
                    "type": "string",
                    "description": "文件路径（相对于工作目录或绝对路径）"
                },
                PARAM_OFFSET: {
                    "type": "integer",
                    "description": "起始行号（从 1 开始，默认 1）"
                },
                PARAM_LIMIT: {
                    "type": "integer",
                    "description": "读取行数（默认 2000）"
                }
            },
            "required": [PARAM_PATH]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let path_str = params
            .get(PARAM_PATH)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool(MSG_MISSING_PATH.into()))?;

        // 沙箱路径校验
        let resolved = resolve_sandbox_path(&self.work_dir, path_str)?;

        if !resolved.is_file() {
            return Err(Error::Tool(format!(
                "路径不是文件：{}",
                resolved.display()
            )));
        }

        // 异步读取文件
        let content = fs::read_to_string(&resolved)
            .await
            .map_err(|e| Error::Tool(format!("读取文件失败「{path_str}」：{e}")))?;

        let offset = params
            .get(PARAM_OFFSET)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_OFFSET, |v| v.max(1) as usize);

        let limit = params
            .get(PARAM_LIMIT)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_LIMIT, |v| v.max(1) as usize);

        let lines: Vec<&str> = content.lines().collect();

        Ok(format_lines(&lines, offset, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_lines_basic() {
        let lines = vec!["fn main() {", "    println!(\"hi\");", "}"];
        let output = format_lines(&lines, 1, 2000);
        assert!(output.contains("1\t"), "应包含行号 1");
        assert!(output.contains("fn main()"), "应包含首行内容");
        // 全量输出，不应有截断提示
        assert!(!output.contains("[..."), "全量输出不应有截断提示");
    }

    #[test]
    fn test_format_lines_with_limit() {
        let lines: Vec<&str> = vec!["line"; 100];
        let output = format_lines(&lines, 1, 10);
        assert!(
            output.contains("[... 文件共 100 行，已显示 1~10 行]"),
            "应包含截断提示"
        );
    }

    #[test]
    fn test_format_lines_with_offset() {
        let lines: Vec<&str> = vec!["line"; 50];
        let output = format_lines(&lines, 10, 5);
        assert!(output.contains("10\t"), "应包含偏移起始行号");
        assert!(
            output.contains("[... 文件共 50 行，已显示 10~14 行]"),
            "应包含截断提示"
        );
    }

    #[test]
    fn test_format_lines_empty() {
        let lines: Vec<&str> = vec![];
        let output = format_lines(&lines, 1, 2000);
        assert!(output.is_empty(), "空文件应返回空字符串");
    }
}
