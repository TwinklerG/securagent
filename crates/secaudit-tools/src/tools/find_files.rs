//! 文件查找工具 — 按 glob 模式递归查找匹配的文件路径。

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::sandbox::{canonicalize_work_dir, resolve_search_dir};
use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_PATTERN: &str = "pattern";
const PARAM_PATH: &str = "path";
const PARAM_MAX_RESULTS: &str = "max_results";

// —— 工具元信息 ——

const TOOL_NAME: &str = "find_files";
const TOOL_DESC: &str = "按 glob 模式查找文件，返回匹配的文件路径列表";

// —— 默认值 ——

/// 默认最大结果数
const DEFAULT_MAX_RESULTS: usize = 100;

/// 文件查找工具
pub struct FindFiles {
    /// 工作目录（沙箱根）
    work_dir: PathBuf,
}

impl FindFiles {
    /// 创建实例，`work_dir` 为沙箱根目录。
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }
}

#[async_trait]
impl Tool for FindFiles {
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
                PARAM_PATTERN: {
                    "type": "string",
                    "description": "glob 模式（如 **/*.py、src/**/*.rs）"
                },
                PARAM_PATH: {
                    "type": "string",
                    "description": "搜索根目录（默认工作目录）"
                },
                PARAM_MAX_RESULTS: {
                    "type": "integer",
                    "description": "最大结果数（默认 100）"
                }
            },
            "required": [PARAM_PATTERN]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let pattern = params
            .get(PARAM_PATTERN)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 pattern 参数".into()))?;

        // 解析搜索路径
        let search_dir = resolve_search_dir(
            &self.work_dir,
            params.get(PARAM_PATH).and_then(Value::as_str),
        )?;

        if !search_dir.is_dir() {
            return Err(Error::Tool(format!(
                "路径不是目录：{}",
                search_dir.display()
            )));
        }

        let max_results = params
            .get(PARAM_MAX_RESULTS)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_RESULTS, |v| v as usize);

        // 构造完整 glob 模式
        let full_pattern = format!("{}/{pattern}", search_dir.display());

        let entries = glob::glob(&full_pattern)
            .map_err(|e| Error::Tool(format!("glob 模式无效「{pattern}」：{e}")))?;

        let sandbox = canonicalize_work_dir(&self.work_dir)?;

        let mut output = String::new();
        let mut count: usize = 0;

        for entry in entries {
            if count >= max_results {
                break;
            }

            let Ok(path) = entry else { continue };

            // 仅包含文件（跳过目录）
            if !path.is_file() {
                continue;
            }

            // 生成相对路径
            let relative = path.strip_prefix(&sandbox).unwrap_or(&path);
            let _ = writeln!(output, "{}", relative.display());
            count += 1;
        }

        if count == 0 {
            return Ok(format!("未找到匹配「{pattern}」的文件"));
        }

        let _ = write!(output, "\n共找到 {count} 个匹配文件");
        if count >= max_results {
            let _ = write!(output, "（已达上限 {max_results}）");
        }

        Ok(output)
    }
}
