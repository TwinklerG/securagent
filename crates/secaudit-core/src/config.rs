use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use secaudit_storage::LOGS_DIR;
use secaudit_storage::RUNTIME_DIR;

use crate::error::Error;

/// API key 环境变量。
const ENV_API_KEY: &str = "SECAUDIT_API_KEY";
/// API 基础 URL 环境变量。
const ENV_API_BASE_URL: &str = "SECAUDIT_API_BASE_URL";
/// 模型名称环境变量。
const ENV_MODEL: &str = "SECAUDIT_MODEL";
/// 最大循环轮次环境变量。
const ENV_MAX_ITERATIONS: &str = "SECAUDIT_MAX_ITERATIONS";
/// 推理策略环境变量。
const ENV_STRATEGY: &str = "SECAUDIT_STRATEGY";
/// 是否启用 Skills 环境变量。
const ENV_ENABLE_SKILLS: &str = "SECAUDIT_ENABLE_SKILLS";
/// 上下文窗口大小环境变量。
const ENV_CONTEXT_WINDOW_TOKENS: &str = "SECAUDIT_CONTEXT_WINDOW_TOKENS";
/// 默认配置文件名。
const CONFIG_FILE: &str = "config.json";

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
/// 默认上下文窗口大小
const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;

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
    /// 当前模型的上下文窗口 token 数
    pub context_window_tokens: u64,
    #[serde(default)]
    pub context_window_tokens_overridden: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    api_base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    max_iterations: Option<u32>,
    reasoning_strategy: Option<String>,
    enable_skills: Option<bool>,
    context_window_tokens: Option<u64>,
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
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            context_window_tokens_overridden: false,
        }
    }
}

impl Config {
    /// 从默认配置文件和环境变量加载配置。
    ///
    /// 默认配置文件路径为 `~/.secaudit/config.json`。文件不存在时会跳过；
    /// 环境变量优先级高于配置文件。
    ///
    /// # Errors
    ///
    /// 配置文件解析失败、字段非法或 API Key 缺失时返回错误。
    pub fn load() -> Result<Self, Error> {
        Self::load_with_sources(Self::default_config_path().as_deref(), process_env)
    }

    /// 从环境变量加载配置，未设置的字段使用默认值。
    ///
    /// # Errors
    ///
    /// 当必需的环境变量（如 `SECAUDIT_API_KEY`）缺失时返回错误。
    pub fn from_env() -> Result<Self, Error> {
        let mut config = Self::default();
        config.apply_env(process_env)?;
        config.validate()
    }

    /// 从配置文件加载。
    ///
    /// # Errors
    ///
    /// 文件不存在或解析失败时返回错误。
    pub fn from_file(path: &Path) -> Result<Self, Error> {
        let mut config = Self::default();
        config.apply_file(path)?;
        config.validate()
    }

    /// 默认配置文件路径。
    #[must_use]
    pub fn default_config_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home_dir| home_dir.join(RUNTIME_DIR).join(CONFIG_FILE))
    }

    /// 默认日志目录路径
    #[must_use]
    pub fn default_log_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home_dir| home_dir.join(RUNTIME_DIR).join(LOGS_DIR))
    }

    #[must_use]
    pub const fn has_context_window_override(&self) -> bool {
        self.context_window_tokens_overridden
    }

    fn load_with_sources<F>(path: Option<&Path>, read_env: F) -> Result<Self, Error>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut config = Self::default();
        if let Some(path) = path.filter(|path| path.is_file()) {
            config.apply_file(path)?;
        }
        config.apply_env(read_env)?;
        config.validate()
    }

    fn apply_file(&mut self, path: &Path) -> Result<(), Error> {
        let content = fs::read_to_string(path).map_err(|error| {
            Error::Config(format!("无法读取配置文件 {}：{error}", path.display()))
        })?;
        let file_config = serde_json::from_str::<FileConfig>(&content).map_err(|error| {
            Error::Config(format!("无法解析配置文件 {}：{error}", path.display()))
        })?;

        self.apply_patch(file_config)
    }

    fn apply_env<F>(&mut self, mut read_env: F) -> Result<(), Error>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let max_iterations = read_env(ENV_MAX_ITERATIONS)
            .map(|value| parse_max_iterations(ENV_MAX_ITERATIONS, &value))
            .transpose()?;
        let enable_skills = read_env(ENV_ENABLE_SKILLS)
            .map(|value| parse_bool(ENV_ENABLE_SKILLS, &value))
            .transpose()?;
        let context_window_tokens = read_env(ENV_CONTEXT_WINDOW_TOKENS)
            .map(|value| {
                value.trim().parse::<u64>().map_err(|error| {
                    Error::Config(format!(
                        "{ENV_CONTEXT_WINDOW_TOKENS} 必须是正整数，当前值为 {value:?}：{error}"
                    ))
                })
            })
            .transpose()?;
        self.apply_patch(FileConfig {
            api_base_url: read_env(ENV_API_BASE_URL),
            api_key: read_env(ENV_API_KEY),
            model: read_env(ENV_MODEL),
            max_iterations,
            reasoning_strategy: read_env(ENV_STRATEGY),
            enable_skills,
            context_window_tokens,
        })
    }

    fn apply_patch(&mut self, patch: FileConfig) -> Result<(), Error> {
        if let Some(value) = non_empty_string(patch.api_base_url) {
            self.api_base_url = value;
        }
        if let Some(value) = non_empty_string(patch.api_key) {
            self.api_key = value;
        }
        if let Some(value) = non_empty_string(patch.model) {
            self.model = value;
        }
        if let Some(value) = patch.max_iterations {
            if value == 0 {
                return Err(Error::Config("配置项 max_iterations 必须大于 0".to_owned()));
            }
            self.max_iterations = value;
        }
        if let Some(value) = non_empty_string(patch.reasoning_strategy) {
            self.reasoning_strategy = value;
        }
        if let Some(value) = patch.enable_skills {
            self.enable_skills = value;
        }
        if let Some(value) = patch.context_window_tokens {
            if value == 0 {
                return Err(Error::Config(
                    "配置项 context_window_tokens 必须大于 0".to_owned(),
                ));
            }
            self.context_window_tokens = value;
            self.context_window_tokens_overridden = true;
        }
        Ok(())
    }

    fn validate(mut self) -> Result<Self, Error> {
        self.api_base_url = require_non_empty("api_base_url", self.api_base_url)?;
        self.api_key = require_non_empty_with_hint("api_key", self.api_key)?;
        self.model = require_non_empty("model", self.model)?;
        self.reasoning_strategy = require_non_empty("reasoning_strategy", self.reasoning_strategy)?;
        if self.max_iterations == 0 {
            return Err(Error::Config("配置项 max_iterations 必须大于 0".to_owned()));
        }
        if self.context_window_tokens == 0 {
            return Err(Error::Config(
                "配置项 context_window_tokens 必须大于 0".to_owned(),
            ));
        }
        Ok(self)
    }
}

fn process_env(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn require_non_empty(name: &str, value: String) -> Result<String, Error> {
    non_empty_string(Some(value)).ok_or_else(|| Error::Config(format!("配置项 {name} 不能为空")))
}

fn require_non_empty_with_hint(name: &str, value: String) -> Result<String, Error> {
    non_empty_string(Some(value)).ok_or_else(|| {
        let file_path = Config::default_config_path().map_or_else(
            || "~/.secaudit/config.json".to_owned(),
            |path| path.display().to_string(),
        );
        Error::Config(format!(
            "配置项 {name} 不能为空。请设置环境变量 {ENV_API_KEY}，或在 {file_path} 写入 API Key"
        ))
    })
}

fn parse_max_iterations(source: &str, value: &str) -> Result<u32, Error> {
    let parsed = value.trim().parse::<u32>().map_err(|error| {
        Error::Config(format!(
            "{source} 必须是正整数，当前值为 {value:?}：{error}"
        ))
    })?;
    if parsed == 0 {
        return Err(Error::Config(format!("{source} 必须大于 0")));
    }
    Ok(parsed)
}

fn parse_bool(source: &str, value: &str) -> Result<bool, Error> {
    value.trim().parse::<bool>().map_err(|error| {
        Error::Config(format!(
            "{source} 必须是布尔值 true 或 false，当前值为 {value:?}：{error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        CONFIG_FILE, Config, ENV_API_BASE_URL, ENV_API_KEY, ENV_CONTEXT_WINDOW_TOKENS,
        ENV_ENABLE_SKILLS, ENV_MODEL,
    };

    #[test]
    fn from_file_loads_partial_json_with_defaults() {
        let temp = TempDir::new().expect("create tempdir");
        let config_path = temp.path().join(CONFIG_FILE);
        fs::write(
            &config_path,
            r#"{
                "api_key": "file-key",
                "model": "file-model"
            }"#,
        )
        .expect("write config");

        let config = Config::from_file(&config_path).expect("load config");

        assert_eq!(config.api_key, "file-key");
        assert_eq!(config.model, "file-model");
        assert_eq!(config.api_base_url, "https://api.openai.com/v1");
        assert_eq!(config.max_iterations, 40);
        assert!(config.enable_skills);
        assert_eq!(config.context_window_tokens, 128_000);
        assert!(!config.has_context_window_override());
    }

    #[test]
    fn load_with_sources_prefers_environment_over_file() {
        let temp = TempDir::new().expect("create tempdir");
        let config_path = temp.path().join(CONFIG_FILE);
        fs::write(
            &config_path,
            r#"{
                "api_key": "file-key",
                "api_base_url": "https://file.example/v1",
                "model": "file-model"
            }"#,
        )
        .expect("write config");

        let config = Config::load_with_sources(Some(&config_path), |name| match name {
            ENV_API_KEY => Some("env-key".to_owned()),
            ENV_API_BASE_URL => Some("https://env.example/v1".to_owned()),
            ENV_MODEL => Some("env-model".to_owned()),
            ENV_ENABLE_SKILLS => Some("false".to_owned()),
            ENV_CONTEXT_WINDOW_TOKENS => Some("64".to_owned()),
            _ => None,
        })
        .expect("load config");

        assert_eq!(config.api_key, "env-key");
        assert_eq!(config.api_base_url, "https://env.example/v1");
        assert_eq!(config.model, "env-model");
        assert!(!config.enable_skills);
        assert_eq!(config.context_window_tokens, 64);
        assert!(config.has_context_window_override());
    }

    #[test]
    fn load_with_sources_requires_api_key() {
        let error = Config::load_with_sources(None, |_| None).expect_err("missing api key");

        assert!(error.to_string().contains("api_key"));
    }
}
