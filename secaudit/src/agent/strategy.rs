//! 推理策略模块 -- 支持 `ReAct` 和 Reflexion 两种推理框架。

mod react;
mod reflexion;

pub use react::ReactStrategy;
pub use reflexion::ReflexionStrategy;

use super::EventBus;
use super::executor::ReActExecutor;
use crate::config::Config;
use crate::error::Error;

/// 策略执行结果。
#[derive(Debug)]
pub struct StrategyResult {
    /// 实际使用的迭代轮次
    pub iterations_used: u32,
}

/// 推理策略接口。
///
/// 策略负责执行审计的核心推理循环，返回执行结果（含迭代轮次）。
#[async_trait::async_trait]
pub trait Strategy: Send {
    /// 执行推理循环。
    ///
    /// 参数中的 executor 已包含系统 prompt 和规划结果。
    async fn run(
        &mut self,
        executor: &mut ReActExecutor<'_>,
        events: &mut EventBus,
        config: &Config,
    ) -> Result<StrategyResult, Error>;
}

/// 推理策略类型标识。
#[derive(Debug, Clone, Default)]
pub enum StrategyKind {
    /// `ReAct` 模式（默认）
    #[default]
    React,
    /// Reflexion 模式（`ReAct` + 反思累积）
    Reflexion,
}

/// `ReAct` 策略标识字符串
pub const STRATEGY_REACT: &str = "react";
/// Reflexion 策略标识字符串
pub const STRATEGY_REFLEXION: &str = "reflexion";

impl StrategyKind {
    /// 从字符串标识解析策略类型。
    #[must_use]
    pub fn from_str_name(s: &str) -> Self {
        // TODO: 用一个 FromStr trait
        match s {
            STRATEGY_REFLEXION => Self::Reflexion,
            _ => Self::React,
        }
    }

    /// 创建对应的策略实例。
    #[must_use]
    pub fn build(self) -> Box<dyn Strategy> {
        match self {
            Self::React => Box::new(ReactStrategy),
            Self::Reflexion => Box::new(ReflexionStrategy::new()),
        }
    }
}
