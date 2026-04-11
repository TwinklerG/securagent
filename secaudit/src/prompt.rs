// Prompt 模块：各审计阶段的 prompt 模板

/// 系统 prompt：定义安全审计专家角色与行为约束
pub const SYSTEM_PROMPT: &str = "你是一位资深安全代码审计专家，精通以下领域：
- OWASP Top 10 Web 应用安全风险分类
- CWE（Common Weakness Enumeration）漏洞标准编目
- 常见编程语言的安全编码规范与典型漏洞模式
- 安全漏洞的利用方式、影响评估与修复方案

你的工作方式：
1. 系统性地分析代码，不遗漏任何潜在风险点
2. 对每个发现给出准确的 CWE 编号和 OWASP 分类
3. 评估漏洞严重程度（Critical / High / Medium / Low / Info）
4. 提供具体、可操作的修复建议
5. 区分确认漏洞与潜在风险，避免误报

请始终以结构化方式输出审计结果。";

/// 构建规划阶段 prompt，引导 LLM 分析代码特征并制定审计计划。
#[must_use]
pub fn planning_prompt(code: &str, language: &str) -> String {
    format!(
        "请分析以下 {language} 代码，制定一份安全审计计划。

要求：
1. 识别代码的主要功能与涉及的安全相关领域（如输入处理、认证、数据存储等）
2. 根据代码特征，列出需要重点检查的漏洞类型，按风险优先级排序
3. 说明你将使用哪些工具、按什么顺序执行检查
4. 标注可能需要特别关注的代码区域

代码如下：
```{language}
{code}
```"
    )
}

/// 构建发现提取 prompt，引导 LLM 将审计总结转为结构化 JSON。
#[must_use]
pub fn findings_extraction_prompt(summary: &str) -> String {
    format!(
        r#"将以下审计总结转换为严格 JSON 数组，只输出 JSON 数组，不要其他内容。

每个元素格式：
{{
  "cwe_id": "CWE-xxx 或 null",
  "severity": "Critical | High | Medium | Low | Info",
  "description": "漏洞描述",
  "location": "代码位置或 null",
  "remediation": "修复建议或 null"
}}

如果没有发现任何安全问题，输出空数组 []。

审计总结：
{summary}"#
    )
}

/// Reflexion 记忆注入 prompt，将之前轮次的反思总结注入上下文。
#[must_use]
pub fn reflexion_memory_prompt(memory: &str) -> String {
    format!("以下是之前审计轮次的反思总结，请将这些经验教训纳入后续分析：\n\n{memory}")
}

/// Reflexion 反思 prompt，引导 LLM 对当前轮次的审计进行反思总结。
#[must_use]
pub fn reflexion_reflect_prompt(round: u32) -> String {
    format!(
        "第 {} 轮审计已完成。请反思本轮审计过程：
1. 本轮发现了哪些新的安全问题？
2. 是否有之前遗漏的检查项需要补充？
3. 工具使用是否有改进空间？
4. 总结本轮的关键教训。

请简洁总结，这些反思将用于指导下一轮审计。",
        round + 1
    )
}

/// 构建反思阶段 prompt，引导 LLM 回顾并评估所有审计发现。
#[must_use]
pub fn reflection_prompt(findings: &str) -> String {
    format!(
        "回顾以上所有审计发现，进行全面的反思与评估。

审计发现：
{findings}

请完成以下任务：
1. 评估每个发现的准确性，剔除可能的误报
2. 检查是否有遗漏的安全风险
3. 对确认的漏洞按严重程度重新排序
4. 补充或修正 CWE 编号和 OWASP 分类
5. 完善修复建议，确保其可操作性
6. 输出最终的结构化审计报告"
    )
}
