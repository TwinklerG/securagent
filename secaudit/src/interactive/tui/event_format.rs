use std::collections::BTreeMap;

use serde::Deserialize;

/// 摘要文本最大字符数。
const PREVIEW_MAX_CHARS: usize = 56;

/// 工具调用参数最大展示键数量。
const ARG_KEY_PREVIEW_LIMIT: usize = 4;

#[derive(Debug, Deserialize)]
struct ToolArgsPreview {
    #[serde(flatten)]
    fields: BTreeMap<String, serde_json::Value>,
}

/// 将工具调用参数从 JSON 文本转换为友好摘要。
#[must_use]
pub fn summarize_tool_args(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return "无参数".to_owned();
    }

    let parsed: Result<ToolArgsPreview, _> = serde_json::from_str(trimmed);
    let Ok(parsed_args) = parsed else {
        return truncate_chars(trimmed, PREVIEW_MAX_CHARS);
    };

    if parsed_args.fields.is_empty() {
        return "空对象".to_owned();
    }

    let keys = parsed_args
        .fields
        .keys()
        .take(ARG_KEY_PREVIEW_LIMIT)
        .cloned()
        .collect::<Vec<_>>();

    let has_more = parsed_args.fields.len() > keys.len();
    let suffix = if has_more { ", ..." } else { "" };
    format!("{}{}", keys.join(", "), suffix)
}

/// 将工具执行结果压缩为一行预览。
#[must_use]
pub fn summarize_tool_result(result: &str) -> String {
    let flattened = result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    if flattened.is_empty() {
        return "(空结果)".to_owned();
    }

    truncate_chars(flattened.as_str(), PREVIEW_MAX_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let preview: String = text.chars().take(max_chars).collect();
    format!("{preview}...")
}

#[cfg(test)]
mod tests {
    use super::{summarize_tool_args, summarize_tool_result};

    #[test]
    fn summarize_tool_args_prefers_key_list() {
        let args = r#"{"path":"src/main.rs","start":1,"end":20}"#;
        assert_eq!(summarize_tool_args(args), "end, path, start");
    }

    #[test]
    fn summarize_tool_args_non_json_fallback() {
        let args = "path=src/main.rs and maybe more";
        let summary = summarize_tool_args(args);
        assert!(summary.starts_with("path="));
    }

    #[test]
    fn summarize_tool_result_flattens_lines() {
        let output = "line1\n\nline2\nline3";
        assert_eq!(summarize_tool_result(output), "line1 | line2 | line3");
    }
}
