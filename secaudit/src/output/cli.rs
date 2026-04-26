// CLI 彩色输出

use std::iter::repeat_n;

use super::truncate_with_ellipsis;
use colored::Colorize;
use secaudit_agent::AuditReport;
use secaudit_agent::state::AgentState;

/// 分隔线长度
const SEPARATOR_LEN: usize = 60;
/// 分隔线字符
const SEPARATOR_CHAR: char = '-';

/// 打印状态转换
pub fn print_state(state: &AgentState) {
    let label = state.label();
    let text = match state {
        AgentState::Init => format!("[>> {label}] 加载配置与工具").green(),
        AgentState::Planning => format!("[>> {label}] 分析代码，制定审计计划").blue(),
        AgentState::Executing { iteration } => {
            format!("[>> {label}] 第 {} 轮 ReAct 循环", iteration + 1).yellow()
        }
        AgentState::Analyzing => format!("[>> {label}] 处理工具返回结果").yellow(),
        AgentState::Reflecting => format!("[>> {label}] 回顾发现，剔除误报").blue(),
        AgentState::Extracting => format!("[>> {label}] 提取结构化发现").blue(),
        AgentState::Reporting => format!("[>> {label}] 生成审计报告").blue(),
        AgentState::Done => format!("[>> {label}] 审计完成").green(),
    };
    println!("{text}");
}

/// 打印工具调用
pub fn print_tool_call(name: &str, args: &str) {
    println!(
        "{} {}({})",
        "[工具]".cyan().bold(),
        name.cyan(),
        args.cyan()
    );
}

/// 思考摘要最大字符数
const THINKING_MAX_CHARS: usize = 200;

/// 打印思考过程
pub fn print_thinking(text: &str) {
    // 截取首行，限制长度避免刷屏
    let preview = text.lines().next().unwrap_or(text);
    let display = truncate_with_ellipsis(preview, THINKING_MAX_CHARS);
    println!("{} {}", "[思考]".dimmed(), display.dimmed());
}

/// 打印分隔线
pub fn print_separator() {
    let line: String = repeat_n(SEPARATOR_CHAR, SEPARATOR_LEN).collect();
    println!("{}", line.dimmed());
}

/// 打印审计报告摘要
pub fn print_report(report: &AuditReport) {
    println!();
    println!("{}", "=== 安全审计报告 ===".bold());
    println!("目标：{}", report.target.cyan());
    println!("语言：{}", report.language.cyan());
    println!("迭代轮次：{}", report.iterations_used);
    println!();

    println!("{}", "--- 总结 ---".bold());
    println!("{}", report.summary);
    println!();

    if report.findings.is_empty() {
        println!("{}", "未发现安全问题。".green());
    } else {
        println!(
            "{} (共 {} 项)",
            "--- 发现 ---".bold(),
            report.findings.len()
        );
        println!();

        for (idx, finding) in report.findings.iter().enumerate() {
            let cwe = finding.cwe_id.as_deref().unwrap_or("N/A");
            let title = format!("#{} [{}] {}", idx + 1, cwe, finding.description);

            let colored_title = match finding.severity.to_lowercase().as_str() {
                "critical" | "high" => title.red().bold(),
                "medium" => title.yellow(),
                "low" | "info" => title.dimmed(),
                _ => title.normal(),
            };
            println!("{colored_title}");

            println!("  严重度：{}", finding.severity);
            if let Some(loc) = &finding.location {
                println!("  位置：{loc}");
            }
            if let Some(rem) = &finding.remediation {
                println!("  修复建议：{rem}");
            }
            println!();
        }
    }
}
