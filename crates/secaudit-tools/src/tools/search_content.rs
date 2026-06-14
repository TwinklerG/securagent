//! 正则内容搜索工具 — 在指定目录中递归搜索匹配正则表达式的文件内容。

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use glob::Pattern;
use regex::Regex;
use serde_json::{Value, json};
use tokio::fs;

use super::file_probe::is_binary;
use super::sandbox::{
    MAX_SEARCH_FILE_SIZE, SensitivePathPolicy, canonicalize_work_dir, check_file_size,
    resolve_search_dir,
};
use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_PATTERN: &str = "pattern";
const PARAM_PATH: &str = "path";
const PARAM_GLOB_FILTER: &str = "glob_filter";
const PARAM_CONTEXT_LINES: &str = "context_lines";
const PARAM_MAX_RESULTS: &str = "max_results";

// —— 默认值 ——

/// 默认上下文行数
const DEFAULT_CONTEXT_LINES: usize = 2;

/// 默认最大结果数
const DEFAULT_MAX_RESULTS: usize = 50;

// —— 工具元信息 ——

const TOOL_NAME: &str = "search_content";
const TOOL_DESC: &str = "在指定路径中搜索正则模式，返回匹配行及上下文";

// —— 输出格式 ——

/// 上下文行缩进前缀
const CONTEXT_LINE_PREFIX: &str = "  ";

/// 匹配结果分隔符
const MATCH_SEPARATOR: &str = "---";

/// 正则内容搜索工具
pub struct SearchContent {
    /// 工作目录（沙箱根）
    work_dir: PathBuf,
    sensitive_paths: SensitivePathPolicy,
}

impl SearchContent {
    /// 创建实例，`work_dir` 为沙箱根目录。
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

/// 异步递归收集目录下所有文件路径。
async fn collect_files(dir: &Path, sensitive_paths: &SensitivePathPolicy) -> Vec<PathBuf> {
    let mut files = Vec::new();
    Box::pin(collect_files_recursive(dir, &mut files, sensitive_paths)).await;
    files
}

/// 异步递归辅助函数。
async fn collect_files_recursive(
    dir: &Path,
    files: &mut Vec<PathBuf>,
    sensitive_paths: &SensitivePathPolicy,
) {
    let Ok(mut reader) = fs::read_dir(dir).await else {
        return;
    };
    loop {
        let Ok(entry) = reader.next_entry().await else {
            break;
        };
        let Some(entry) = entry else { break };
        let path = entry.path();

        let Ok(ft) = entry.file_type().await else {
            continue;
        };
        if ft.is_dir() {
            if sensitive_paths.has_sensitive_component(&path) {
                continue;
            }
            Box::pin(collect_files_recursive(&path, files, sensitive_paths)).await;
        } else {
            files.push(path);
        }
    }
}

/// 提取匹配行及上下文，返回格式化文本。
fn format_match(filepath: &str, lines: &[&str], match_line_idx: usize, context: usize) -> String {
    let mut output = String::new();
    let line_no = match_line_idx + 1;

    let start = match_line_idx.saturating_sub(context);
    let end = (match_line_idx + context + 1).min(lines.len());

    for i in start..end {
        let current_no = i + 1;
        if let Some(line) = lines.get(i) {
            if i == match_line_idx {
                let _ = writeln!(output, "{filepath}:{line_no}: {line}");
            } else {
                let _ = writeln!(output, "{CONTEXT_LINE_PREFIX}{current_no}: {line}");
            }
        }
    }

    output
}

#[async_trait]
impl Tool for SearchContent {
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
                    "description": "正则表达式"
                },
                PARAM_PATH: {
                    "type": "string",
                    "description": "搜索路径（默认工作目录）"
                },
                PARAM_GLOB_FILTER: {
                    "type": "string",
                    "description": "文件过滤 glob（如 *.py）"
                },
                PARAM_CONTEXT_LINES: {
                    "type": "integer",
                    "description": "上下文行数（默认 2）"
                },
                PARAM_MAX_RESULTS: {
                    "type": "integer",
                    "description": "最大结果数（默认 50）"
                }
            },
            "required": [PARAM_PATTERN]
        })
    }

    async fn precheck(&self, params: &Value) -> Result<(), String> {
        let pattern_str = params
            .get(PARAM_PATTERN)
            .and_then(Value::as_str)
            .ok_or_else(|| "缺少 pattern 参数".to_owned())?;

        Regex::new(pattern_str).map_err(|e| format!("正则表达式无效：{e}"))?;

        let search_dir = resolve_search_dir(
            &self.work_dir,
            params.get(PARAM_PATH).and_then(Value::as_str),
        )
        .map_err(|e| e.to_string())?;

        if !search_dir.is_dir() {
            return Err(format!("路径不是目录：{}", search_dir.display()));
        }
        if self.sensitive_paths.has_sensitive_component(&search_dir) {
            return Err(format!("拒绝搜索敏感目录：{}", search_dir.display()));
        }

        Ok(())
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        self.precheck(params).await.map_err(Error::Tool)?;

        let pattern_str = params
            .get(PARAM_PATTERN)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 pattern 参数".into()))?;

        let regex = Regex::new(pattern_str)
            .map_err(|e| Error::Tool(format!("正则表达式无效「{pattern_str}」：{e}")))?;

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

        // 解析 glob 过滤器
        let glob_filter = params
            .get(PARAM_GLOB_FILTER)
            .and_then(Value::as_str)
            .map(Pattern::new)
            .transpose()
            .map_err(|e| Error::Tool(format!("glob 模式无效：{e}")))?;

        let context_lines = params
            .get(PARAM_CONTEXT_LINES)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_CONTEXT_LINES, |v| v as usize);

        let max_results = params
            .get(PARAM_MAX_RESULTS)
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_RESULTS, |v| v as usize);

        // 异步递归遍历文件
        let files = collect_files(&search_dir, &self.sensitive_paths).await;
        let mut match_count: usize = 0;
        let mut output = String::new();

        let sandbox = canonicalize_work_dir(&self.work_dir)?;

        for file_path in &files {
            if match_count >= max_results {
                break;
            }
            if self.sensitive_paths.has_sensitive_component(file_path) {
                continue;
            }
            let Ok(canonical_path) = file_path.canonicalize() else {
                continue;
            };
            if !canonical_path.starts_with(&sandbox)
                || self
                    .sensitive_paths
                    .has_sensitive_component(&canonical_path)
            {
                continue;
            }

            // glob 过滤
            if let Some(gf) = &glob_filter {
                let file_name = canonical_path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                if !gf.matches(&file_name) {
                    continue;
                }
            }

            // 异步跳过二进制文件
            if check_file_size(&canonical_path, MAX_SEARCH_FILE_SIZE)
                .await
                .is_err()
            {
                continue;
            }
            if is_binary(&canonical_path).await {
                continue;
            }

            let Ok(content) = fs::read_to_string(&canonical_path).await else {
                continue;
            };

            let lines: Vec<&str> = content.lines().collect();

            // 生成相对路径用于输出
            let display_path = canonical_path
                .strip_prefix(&sandbox)
                .unwrap_or(&canonical_path)
                .display()
                .to_string();

            for (idx, line) in lines.iter().enumerate() {
                if match_count >= max_results {
                    break;
                }

                if regex.is_match(line) {
                    if match_count > 0 {
                        let _ = writeln!(output, "{MATCH_SEPARATOR}");
                    }
                    output.push_str(&format_match(&display_path, &lines, idx, context_lines));
                    match_count += 1;
                }
            }
        }

        if match_count == 0 {
            return Ok(format!("未找到匹配「{pattern_str}」的内容"));
        }

        let _ = write!(output, "\n共找到 {match_count} 处匹配");
        if match_count >= max_results {
            let _ = write!(output, "（已达上限 {max_results}）");
        }

        Ok(output)
    }
}
