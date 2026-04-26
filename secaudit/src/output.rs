// 输出模块：CLI 彩色展示与结构化报告生成

pub mod cli;
pub mod report;

/// 省略标记
const ELLIPSIS: &str = "...";

/// 按字符数截断字符串，超出时追加省略标记。
#[must_use]
pub(crate) fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}{ELLIPSIS}")
}
