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

/// 默认启用 Skills
const DEFAULT_ENABLE_SKILLS: bool = true;

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
    /// 推理策略（react / reflexion）
    pub reasoning_strategy: String,
    /// 是否启用 Skills 系统
    pub enable_skills: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_base_url: DEFAULT_API_BASE_URL.into(),
            api_key: String::new(),
            model: DEFAULT_MODEL.into(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            reasoning_strategy: DEFAULT_STRATEGY.into(),
            enable_skills: DEFAULT_ENABLE_SKILLS,
        }
    }
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

        let reasoning_strategy =
            var(format!("{ENV_PREFIX}STRATEGY")).unwrap_or_else(|_| DEFAULT_STRATEGY.into());

        let enable_skills = var(format!("{ENV_PREFIX}ENABLE_SKILLS"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_ENABLE_SKILLS);

        Ok(Self {
            api_base_url,
            api_key,
            model,
            max_iterations,
            reasoning_strategy,
            enable_skills,
        })
    }

    /// 从配置文件加载（骨架方法，迭代三实现配置文件分层）。
    ///
    /// # Errors
    ///
    /// 文件不存在或解析失败时返回错误。
    pub fn from_file(_path: &Path) -> Result<Self, Error> {
        // TODO: 迭代三实现 TOML/JSON 配置文件解析
        Err(Error::Config("配置文件加载尚未实现".into()))
    }
}
