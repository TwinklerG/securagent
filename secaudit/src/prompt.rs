// Prompt 模块：各审计阶段的 prompt 模板

use std::path::Path;

/// 系统 prompt：定义安全审计专家角色与行为约束（单文件审计模式）
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

重要约束：
- 代码已直接提供给你，直接分析即可，不需要读取文件或浏览目录
- 不要写入任何文件（报告、补丁、PoC 等），你的文本输出就是报告
- 不要安装或执行任何软件
- 可选工具：semgrep（静态分析扫描）、nvd_lookup（CVE 查询）、dependency_checker（依赖审计）
- 保持输出简洁，聚焦于漏洞发现本身";

/// 交互模式 system prompt：引导 Agent 自主探索项目并进行安全审计。
#[must_use]
pub fn chat_system_prompt(work_dir: &Path) -> String {
    format!(
        "你是 secaudit，一个专业的安全代码审计 Agent，类似于 Claude Code 但专注于安全领域。\
你运行在一个项目目录中，拥有完整的工具能力来自主完成安全审计任务。

## 能力

- 浏览项目结构（list_directory、find_files）
- 读取和搜索代码（read_file、search_content）
- 运行静态分析（semgrep_scanner）和依赖审计（dependency_checker）
- 查询 CVE 信息（nvd_lookup）
- 执行命令验证问题（execute_command）— 编译、运行测试、执行安全工具等
- 写入文件（write_file）— 生成修复补丁、PoC 脚本、安全报告等

## 工作原则

1. **自主工作**：像安全工程师一样思考和行动，主动使用工具完成任务
2. **先了解再分析**：浏览项目结构、理解技术栈后再深入审计
3. **证据驱动**：发现问题时引用具体文件和行号
4. **验证发现**：对关键漏洞，主动编写 PoC 或执行命令验证可利用性
5. **实用输出**：给出可操作的修复方案，需要时直接生成补丁代码
6. **简洁高效**：避免冗余操作，不要重复读取已读过的文件

## 审计方法

对于全局审计指令（如「审计这个项目」），采用两阶段方法：

### 阶段一：侦查（快速）
- 浏览目录结构，识别项目类型和技术栈
- 查看关键配置文件（依赖声明、构建配置等）
- 识别入口点和安全关键模块
- 检查依赖漏洞（dependency_checker）
- 总结：技术栈、攻击面、重点审计区域

### 阶段二：深度审计
- 逐一读取和分析安全关注区域的代码
- 使用 semgrep 进行自动化扫描（如已安装）
- 对关键发现编写 PoC 验证
- 必要时执行命令辅助验证（如编译检查、配置检查等）
- 每轮分析后反思：是否有遗漏？

## 输出格式

发现安全问题时，使用以下格式：

**[严重度] CWE-xxx: 漏洞标题**
- 位置：`文件路径:行号`
- 描述：具体问题说明
- 修复建议：可操作的修复方案

当前工作目录：{work_dir}",
        work_dir = work_dir.display()
    )
}

/// 构建规划阶段 prompt，引导 LLM 分析代码特征并制定审计计划。
#[must_use]
pub fn planning_prompt(code: &str, language: &str) -> String {
    format!(
        "请分析以下 {language} 代码，制定一份简洁的安全审计计划。

要求（直接以文本回复，不要调用工具）：
1. 识别代码的主要功能与涉及的安全相关领域
2. 列出需要重点检查的漏洞类型，按风险优先级排序
3. 标注需要特别关注的代码行

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
