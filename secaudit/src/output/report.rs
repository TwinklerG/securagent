// 结构化报告生成：JSON 与 Markdown 格式

use std::fmt::Write;

use crate::agent::AuditReport;
use crate::error::Error;

/// 将审计报告序列化为 JSON
///
/// # Errors
///
/// 序列化失败时返回 [`Error::Parse`]。
pub fn to_json(report: &AuditReport) -> Result<String, Error> {
    serde_json::to_string_pretty(report).map_err(|e| Error::Parse(format!("报告序列化失败：{e}")))
}

/// 将审计报告格式化为 Markdown
#[must_use]
pub fn to_markdown(report: &AuditReport) -> String {
    let mut md = String::new();

    md.push_str("# 安全审计报告\n\n");
    let _ = writeln!(md, "- **目标**: {}", report.target);
    let _ = writeln!(md, "- **语言**: {}", report.language);
    let _ = writeln!(md, "- **迭代轮次**: {}", report.iterations_used);
    md.push('\n');

    md.push_str("## 总结\n\n");
    md.push_str(&report.summary);
    md.push_str("\n\n");

    md.push_str("## 发现\n\n");

    if report.findings.is_empty() {
        md.push_str("未发现安全问题。\n");
    } else {
        for finding in &report.findings {
            let cwe = finding.cwe_id.as_deref().unwrap_or("N/A");
            let _ = writeln!(md, "### {cwe}: {}\n", finding.description);
            let _ = writeln!(md, "- **严重度**: {}", finding.severity);
            if let Some(loc) = &finding.location {
                let _ = writeln!(md, "- **位置**: {loc}");
            }
            if let Some(rem) = &finding.remediation {
                let _ = writeln!(md, "- **修复建议**: {rem}");
            }
            md.push('\n');
        }
    }

    md
}
