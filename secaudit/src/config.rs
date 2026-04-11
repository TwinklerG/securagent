use std::path::PathBuf;

use crate::error::Error;

/// 环境变量前缀
const ENV_PREFIX: &str = "SECAUDIT_";

/// 默认 API 基础 URL
const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";

/// 默认模型名称
const DEFAULT_MODEL: &str = "gpt-4o";

/// 默认最大 `ReAct` 循环轮次
const DEFAULT_MAX_ITERATIONS: u32 = 10;

/// 默认规则文件目录
const DEFAULT_RULES_DIR: &str = "rules";

/// 默认推理策略
const DEFAULT_STRATEGY: &str = "react";

/// 应用配置，支持环境变量（`SECAUDIT_` 前缀）和配置文件两种来源。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    /// LLM API 基础 URL
    pub api_base_url: String,
    /// LLM API 密钥（环境变量 `SECAUDIT_API_KEY`）
    pub api_key: String,
    /// 使用的模型名称
    pub model: String,
    /// 最大 `ReAct` 循环轮次
    pub max_iterations: u32,
    /// 规则文件目录路径
    pub rules_dir: PathBuf,
    /// 推理策略（react / reflexion）
    pub reasoning_strategy: String,
}

impl Config {
    /// 从环境变量加载配置，未设置的字段使用默认值。
    ///
    /// # Errors
    ///
    /// 当必需的环境变量（如 `SECAUDIT_API_KEY`）缺失时返回错误。
    pub fn from_env() -> Result<Self, Error> {
        use std::env::var;

        let api_key = var(format!("{ENV_PREFIX}API_KEY"))
            .map_err(|err| Error::Config(format!("环境变量 SECAUDIT_API_KEY 未设置：{err}")))?;

        let api_base_url = var(format!("{ENV_PREFIX}API_BASE_URL"))
            .unwrap_or_else(|_| DEFAULT_API_BASE_URL.into());

        let model = var(format!("{ENV_PREFIX}MODEL")).unwrap_or_else(|_| DEFAULT_MODEL.into());

        let max_iterations = var(format!("{ENV_PREFIX}MAX_ITERATIONS"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_ITERATIONS);

        let rules_dir = var(format!("{ENV_PREFIX}RULES_DIR"))
            .map_or_else(|_| PathBuf::from(DEFAULT_RULES_DIR), PathBuf::from);

        let reasoning_strategy =
            var(format!("{ENV_PREFIX}STRATEGY")).unwrap_or_else(|_| DEFAULT_STRATEGY.into());

        Ok(Self {
            api_base_url,
            api_key,
            model,
            max_iterations,
            rules_dir,
            reasoning_strategy,
        })
    }
}
