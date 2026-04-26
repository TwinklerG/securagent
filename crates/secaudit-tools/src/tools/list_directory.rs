//! 目录列出工具 — 列出目录内容，显示文件名、类型和大小，支持递归遍历。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

use super::shared::resolve_sandbox_path;
use crate::error::Error;
use crate::tools::Tool;

// —— 工具元信息 ——

const TOOL_NAME: &str = "list_directory";
const TOOL_DESC: &str = "列出目录内容，显示文件名、类型和大小";

// —— 参数字段名 ——

const PARAM_PATH: &str = "path";
const PARAM_RECURSIVE: &str = "recursive";
const PARAM_MAX_DEPTH: &str = "max_depth";

// —— 默认值 ——

/// 默认最大递归深度
const DEFAULT_MAX_DEPTH: usize = 3;

/// 每层缩进空格数
const INDENT_SPACES: usize = 2;

// —— 类型标签 ——

const LABEL_DIR: &str = "目录";
const LABEL_FILE: &str = "文件";
const LABEL_SYMLINK: &str = "符号链接";

// —— 字节单位 ——

const UNIT_BYTES: &str = "字节";

// —— 提示消息 ——

const MSG_MISSING_PATH: &str = "缺少 path 参数";

/// 目录列出工具，支持沙箱路径校验与递归遍历。
pub struct ListDirectory {
    /// 沙箱工作目录
    work_dir: PathBuf,
}

impl ListDirectory {
    /// 创建实例，`work_dir` 为沙箱根目录。
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        Self { work_dir }
    }
}

/// 目录条目信息，用于排序和格式化输出。
struct DirEntry {
    /// 显示名称（目录末尾带 `/`）
    name: String,
    /// 是否为目录
    is_dir: bool,
    /// 是否为符号链接
    is_symlink: bool,
    /// 文件大小（目录为 0）
    size: u64,
    /// 完整路径（递归遍历时使用）
    path: PathBuf,
}

/// 读取并排序目录条目：目录在前、文件在后，各自按名称字母序排列。
async fn read_sorted_entries(dir: &Path) -> Result<Vec<DirEntry>, Error> {
    let mut reader = fs::read_dir(dir)
        .await
        .map_err(|e| Error::Tool(format!("读取目录失败「{}」：{e}", dir.display())))?;

    let mut entries = Vec::new();

    loop {
        let entry = reader
            .next_entry()
            .await
            .map_err(|e| Error::Tool(format!("遍历目录条目失败：{e}")))?;

        let Some(entry) = entry else { break };

        let file_type = entry
            .file_type()
            .await
            .map_err(|e| Error::Tool(format!("获取文件类型失败：{e}")))?;

        let raw_name = entry.file_name().to_string_lossy().into_owned();

        let is_dir = file_type.is_dir();
        let is_symlink = file_type.is_symlink();

        let name = if is_dir {
            format!("{raw_name}/")
        } else {
            raw_name
        };

        let size = if is_dir {
            0
        } else {
            entry.metadata().await.map_or(0, |m| m.len())
        };

        entries.push(DirEntry {
            name,
            is_dir,
            is_symlink,
            size,
            path: entry.path(),
        });
    }

    // 排序：目录优先，同类按名称字母序
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    Ok(entries)
}

/// 格式化单条目录条目。
fn format_entry(entry: &DirEntry, depth: usize) -> String {
    let indent = " ".repeat(depth * INDENT_SPACES);
    let label = if entry.is_symlink {
        LABEL_SYMLINK
    } else if entry.is_dir {
        LABEL_DIR
    } else {
        LABEL_FILE
    };

    if entry.is_dir {
        format!("{indent}[{label}] {}", entry.name)
    } else {
        format!(
            "{indent}[{label}] {} ({} {UNIT_BYTES})",
            entry.name, entry.size
        )
    }
}

/// 递归列出目录内容，写入 `output`。
///
/// `depth` 为当前递归深度（从 0 开始），`max_depth` 为最大深度。
async fn list_recursive(
    dir: &Path,
    output: &mut String,
    depth: usize,
    max_depth: usize,
) -> Result<(), Error> {
    let entries = read_sorted_entries(dir).await?;

    for entry in &entries {
        let _ = writeln!(output, "{}", format_entry(entry, depth));

        if entry.is_dir && depth < max_depth {
            // 使用 Box::pin 避免异步递归的 size 问题
            Box::pin(list_recursive(&entry.path, output, depth + 1, max_depth)).await?;
        }
    }

    Ok(())
}

#[async_trait]
impl Tool for ListDirectory {
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
                    "description": "目录路径（相对于工作目录或绝对路径）"
                },
                PARAM_RECURSIVE: {
                    "type": "boolean",
                    "description": "是否递归列出（默认 false）"
                },
                PARAM_MAX_DEPTH: {
                    "type": "integer",
                    "description": "递归最大深度（默认 3）"
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

        if !resolved.is_dir() {
            return Err(Error::Tool(format!("路径不是目录：{}", resolved.display())));
        }

        let recursive = params
            .get(PARAM_RECURSIVE)
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let max_depth = params
            .get(PARAM_MAX_DEPTH)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_DEPTH, |v| v as usize);

        let mut output = String::new();

        if recursive {
            list_recursive(&resolved, &mut output, 0, max_depth).await?;
        } else {
            let entries = read_sorted_entries(&resolved).await?;
            for entry in &entries {
                let _ = writeln!(output, "{}", format_entry(entry, 0));
            }
        }

        if output.is_empty() {
            return Ok(format!("目录为空：{path_str}"));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_entry_file() {
        let entry = DirEntry {
            name: "main.rs".into(),
            is_dir: false,
            is_symlink: false,
            size: 1234,
            path: PathBuf::from("main.rs"),
        };
        let formatted = format_entry(&entry, 0);
        assert_eq!(formatted, "[文件] main.rs (1234 字节)");
    }

    #[test]
    fn format_entry_dir() {
        let entry = DirEntry {
            name: "src/".into(),
            is_dir: true,
            is_symlink: false,
            size: 0,
            path: PathBuf::from("src"),
        };
        let formatted = format_entry(&entry, 0);
        assert_eq!(formatted, "[目录] src/");
    }

    #[test]
    fn format_entry_symlink() {
        let entry = DirEntry {
            name: "link".into(),
            is_dir: false,
            is_symlink: true,
            size: 100,
            path: PathBuf::from("link"),
        };
        let formatted = format_entry(&entry, 0);
        assert_eq!(formatted, "[符号链接] link (100 字节)");
    }

    #[test]
    fn format_entry_indented() {
        let entry = DirEntry {
            name: "nested.rs".into(),
            is_dir: false,
            is_symlink: false,
            size: 42,
            path: PathBuf::from("nested.rs"),
        };
        let formatted = format_entry(&entry, 2);
        assert_eq!(formatted, "    [文件] nested.rs (42 字节)");
    }
}
