// Agent 状态定义

use std::fmt::{self, Display};

use serde::Serialize;

/// Agent 运行状态
#[derive(Debug, Clone, Serialize)]
pub enum AgentState {
    /// 初始化：加载配置和工具
    Init,
    /// 规划中：分析代码，制定审计计划
    Planning,
    /// 执行中：ReAct 循环执行工具
    Executing {
        /// 当前迭代轮次
        iteration: u32,
    },
    /// 分析中：处理工具返回结果
    Analyzing,
    /// 重试中：工具调用失败后重试
    #[expect(dead_code, reason = "预留用于自我修正增强")]
    Retrying {
        /// 失败的工具名称
        tool_name: String,
        /// 重试次数
        attempt: u32,
    },
    /// 反思中：回顾所有发现，剔除误报
    Reflecting,
    /// 提取中：从审计总结中提取结构化发现
    Extracting,
    /// 生成报告
    Reporting,
    /// 发生错误
    #[expect(dead_code, reason = "预留用于错误恢复流程")]
    Error {
        /// 错误信息
        message: String,
    },
    /// 审计完成
    Done,
}

impl AgentState {
    /// 获取状态的中文描述
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Init => "初始化",
            Self::Planning => "规划中",
            Self::Executing { .. } => "执行中",
            Self::Analyzing => "分析中",
            Self::Retrying { .. } => "重试中",
            Self::Reflecting => "反思中",
            Self::Extracting => "提取中",
            Self::Reporting => "生成报告",
            Self::Error { .. } => "错误",
            Self::Done => "完成",
        }
    }
}

impl Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}
