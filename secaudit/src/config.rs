use std::path::Path;

use crate::error::Error;

/// 环境变量前缀
const ENV_PREFIX: &str = "SECAUDIT_";

/// 默认 API 基础 URL
const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";

/// 默认模型名称
const DEFAULT_MODEL: &str = "gpt-4o";

/// 默认最大 `ReAct` 循环轮次
const DEFAULT_MAX_ITERATIONS: u32 = 40;

/// 默认推理策略
const DEFAULT_STRATEGY: &str = "react";

/// 应用配置，支持 `.env` 文件和环境变量（`SECAUDIT_` 前缀）两种来源。
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
    /// 推理策略（react / reflexion）
    pub reasoning_strategy: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_base_url: DEFAULT_API_BASE_URL.into(),
            api_key: String::new(),
            model: DEFAULT_MODEL.into(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            reasoning_strategy: DEFAULT_STRATEGY.into(),
        }
    }
}

impl Config {
    /// 从 `.env` 文件和环境变量加载配置，未设置的字段使用默认值。
    ///
    /// 加载顺序：
    /// 1. 尝试从当前目录的 `.env` 文件加载（若存在）
    /// 2. 覆盖环境变量（`SECAUDIT_` 前缀）
    ///
    /// # Errors
    ///
    /// 当必需的环境变量（如 `SECAUDIT_API_KEY`）缺失时返回错误。
    pub fn from_env() -> Result<Self, Error> {
        use std::env::var;

        // 尝试加载 .env 文件（若不存在则忽略错误）
        let _ = dotenv::dotenv();

        let api_key = var(format!("{ENV_PREFIX}API_KEY"))
            .map_err(|err| Error::Config(format!("环境变量 SECAUDIT_API_KEY 未设置：{err}")))?;

        let api_base_url = var(format!("{ENV_PREFIX}API_BASE_URL"))
            .unwrap_or_else(|_| DEFAULT_API_BASE_URL.into());

        let model = var(format!("{ENV_PREFIX}MODEL")).unwrap_or_else(|_| DEFAULT_MODEL.into());

        let max_iterations = var(format!("{ENV_PREFIX}MAX_ITERATIONS"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_ITERATIONS);

        let reasoning_strategy =
            var(format!("{ENV_PREFIX}STRATEGY")).unwrap_or_else(|_| DEFAULT_STRATEGY.into());

        Ok(Self {
            api_base_url,
            api_key,
            model,
            max_iterations,
            reasoning_strategy,
        })
    }

    /// 从指定路径的 `.env` 文件加载配置。
    ///
    /// # Errors
    ///
    /// 当文件不存在或必需的环境变量缺失时返回错误。
    #[allow(dead_code)]
    pub fn from_env_file(path: &Path) -> Result<Self, Error> {
        use std::env::var;

        dotenv::from_path(path)
            .map_err(|err| Error::Config(format!("加载 .env 文件失败：{:?}", err)))?;

        let api_key = var(format!("{ENV_PREFIX}API_KEY"))
            .map_err(|err| Error::Config(format!("环境变量 SECAUDIT_API_KEY 未设置：{err}")))?;

        let api_base_url = var(format!("{ENV_PREFIX}API_BASE_URL"))
            .unwrap_or_else(|_| DEFAULT_API_BASE_URL.into());

        let model = var(format!("{ENV_PREFIX}MODEL")).unwrap_or_else(|_| DEFAULT_MODEL.into());

        let max_iterations = var(format!("{ENV_PREFIX}MAX_ITERATIONS"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_ITERATIONS);

        let reasoning_strategy =
            var(format!("{ENV_PREFIX}STRATEGY")).unwrap_or_else(|_| DEFAULT_STRATEGY.into());

        Ok(Self {
            api_base_url,
            api_key,
            model,
            max_iterations,
            reasoning_strategy,
        })
    }

    /// 从配置文件加载（调用 `.env` 文件加载逻辑）。
    ///
    /// # Errors
    ///
    /// 文件不存在或解析失败时返回错误。
    #[allow(dead_code)]
    pub fn from_file(path: &Path) -> Result<Self, Error> {
        Self::from_env_file(path)
    }
}
