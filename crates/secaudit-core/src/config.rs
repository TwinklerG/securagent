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
/// 命令自动放行白名单环境变量（逗号分隔）。
const ENV_COMMAND_ALLOWLIST: &str = "SECAUDIT_COMMAND_ALLOWLIST";
/// 命令禁止黑名单环境变量（逗号分隔）。
const ENV_COMMAND_BLOCKLIST: &str = "SECAUDIT_COMMAND_BLOCKLIST";
/// 敏感路径追加黑名单环境变量（逗号分隔）。
const ENV_SENSITIVE_PATH_BLOCKLIST: &str = "SECAUDIT_SENSITIVE_PATH_BLOCKLIST";
/// LLM 最大调用尝试次数环境变量。
const ENV_LLM_RETRY_MAX_ATTEMPTS: &str = "SECAUDIT_LLM_RETRY_MAX_ATTEMPTS";
/// LLM 重试初始退避毫秒数环境变量。
const ENV_LLM_RETRY_INITIAL_DELAY_MS: &str = "SECAUDIT_LLM_RETRY_INITIAL_DELAY_MS";
/// LLM 重试最大退避毫秒数环境变量。
const ENV_LLM_RETRY_MAX_DELAY_MS: &str = "SECAUDIT_LLM_RETRY_MAX_DELAY_MS";
/// LLM 熔断连续失败阈值环境变量。
const ENV_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD: &str =
    "SECAUDIT_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD";
/// LLM 熔断冷却毫秒数环境变量。
const ENV_LLM_CIRCUIT_BREAKER_COOLDOWN_MS: &str = "SECAUDIT_LLM_CIRCUIT_BREAKER_COOLDOWN_MS";
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
/// 默认 LLM 最大调用尝试次数
const DEFAULT_LLM_RETRY_MAX_ATTEMPTS: u32 = 2;
/// 默认 LLM 重试初始退避毫秒数
const DEFAULT_LLM_RETRY_INITIAL_DELAY_MS: u64 = 200;
/// 默认 LLM 重试最大退避毫秒数
const DEFAULT_LLM_RETRY_MAX_DELAY_MS: u64 = 2_000;
/// 默认 LLM 熔断连续失败阈值
const DEFAULT_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// 默认 LLM 熔断冷却毫秒数
const DEFAULT_LLM_CIRCUIT_BREAKER_COOLDOWN_MS: u64 = 30_000;

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
    /// 用户配置的命令自动放行白名单。
    #[serde(default)]
    pub command_allowlist: Vec<String>,
    /// 用户配置的命令禁止黑名单。
    #[serde(default)]
    pub command_blocklist: Vec<String>,
    /// 用户追加的敏感路径/组件黑名单。
    #[serde(default)]
    pub sensitive_path_blocklist: Vec<String>,
    /// LLM 调用最大尝试次数（含首次调用）。
    pub llm_retry_max_attempts: u32,
    /// LLM 指数退避初始延迟毫秒数。
    pub llm_retry_initial_delay_ms: u64,
    /// LLM 指数退避最大延迟毫秒数。
    pub llm_retry_max_delay_ms: u64,
    /// LLM 熔断连续失败阈值。
    pub llm_circuit_breaker_failure_threshold: u32,
    /// LLM 熔断冷却毫秒数。
    pub llm_circuit_breaker_cooldown_ms: u64,
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
    command_allowlist: Option<Vec<String>>,
    command_blocklist: Option<Vec<String>>,
    sensitive_path_blocklist: Option<Vec<String>>,
    llm_retry_max_attempts: Option<u32>,
    llm_retry_initial_delay_ms: Option<u64>,
    llm_retry_max_delay_ms: Option<u64>,
    llm_circuit_breaker_failure_threshold: Option<u32>,
    llm_circuit_breaker_cooldown_ms: Option<u64>,
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
            command_allowlist: Vec::new(),
            command_blocklist: Vec::new(),
            sensitive_path_blocklist: Vec::new(),
            llm_retry_max_attempts: DEFAULT_LLM_RETRY_MAX_ATTEMPTS,
            llm_retry_initial_delay_ms: DEFAULT_LLM_RETRY_INITIAL_DELAY_MS,
            llm_retry_max_delay_ms: DEFAULT_LLM_RETRY_MAX_DELAY_MS,
            llm_circuit_breaker_failure_threshold: DEFAULT_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD,
            llm_circuit_breaker_cooldown_ms: DEFAULT_LLM_CIRCUIT_BREAKER_COOLDOWN_MS,
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
        let command_allowlist = read_env(ENV_COMMAND_ALLOWLIST)
            .map(|value| parse_csv_list(ENV_COMMAND_ALLOWLIST, &value));
        let command_blocklist = read_env(ENV_COMMAND_BLOCKLIST)
            .map(|value| parse_csv_list(ENV_COMMAND_BLOCKLIST, &value));
        let sensitive_path_blocklist = read_env(ENV_SENSITIVE_PATH_BLOCKLIST)
            .map(|value| parse_csv_list(ENV_SENSITIVE_PATH_BLOCKLIST, &value));
        let llm_retry_max_attempts = read_env(ENV_LLM_RETRY_MAX_ATTEMPTS)
            .map(|value| parse_positive_u32(ENV_LLM_RETRY_MAX_ATTEMPTS, &value))
            .transpose()?;
        let llm_retry_initial_delay_ms = read_env(ENV_LLM_RETRY_INITIAL_DELAY_MS)
            .map(|value| parse_u64(ENV_LLM_RETRY_INITIAL_DELAY_MS, &value))
            .transpose()?;
        let llm_retry_max_delay_ms = read_env(ENV_LLM_RETRY_MAX_DELAY_MS)
            .map(|value| parse_u64(ENV_LLM_RETRY_MAX_DELAY_MS, &value))
            .transpose()?;
        let llm_circuit_breaker_failure_threshold =
            read_env(ENV_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD)
                .map(|value| parse_positive_u32(ENV_LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD, &value))
                .transpose()?;
        let llm_circuit_breaker_cooldown_ms = read_env(ENV_LLM_CIRCUIT_BREAKER_COOLDOWN_MS)
            .map(|value| parse_u64(ENV_LLM_CIRCUIT_BREAKER_COOLDOWN_MS, &value))
            .transpose()?;
        self.apply_patch(FileConfig {
            api_base_url: read_env(ENV_API_BASE_URL),
            api_key: read_env(ENV_API_KEY),
            model: read_env(ENV_MODEL),
            max_iterations,
            reasoning_strategy: read_env(ENV_STRATEGY),
            enable_skills,
            context_window_tokens,
            command_allowlist,
            command_blocklist,
            sensitive_path_blocklist,
            llm_retry_max_attempts,
            llm_retry_initial_delay_ms,
            llm_retry_max_delay_ms,
            llm_circuit_breaker_failure_threshold,
            llm_circuit_breaker_cooldown_ms,
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
        if let Some(value) = normalize_string_list(patch.command_allowlist) {
            self.command_allowlist = value;
        }
        if let Some(value) = normalize_string_list(patch.command_blocklist) {
            self.command_blocklist = value;
        }
        if let Some(value) = normalize_string_list(patch.sensitive_path_blocklist) {
            self.sensitive_path_blocklist = value;
        }
        if let Some(value) = patch.llm_retry_max_attempts {
            if value == 0 {
                return Err(Error::Config(
                    "配置项 llm_retry_max_attempts 必须大于 0".to_owned(),
                ));
            }
            self.llm_retry_max_attempts = value;
        }
        if let Some(value) = patch.llm_retry_initial_delay_ms {
            self.llm_retry_initial_delay_ms = value;
        }
        if let Some(value) = patch.llm_retry_max_delay_ms {
            self.llm_retry_max_delay_ms = value;
        }
        if let Some(value) = patch.llm_circuit_breaker_failure_threshold {
            if value == 0 {
                return Err(Error::Config(
                    "配置项 llm_circuit_breaker_failure_threshold 必须大于 0".to_owned(),
                ));
            }
            self.llm_circuit_breaker_failure_threshold = value;
        }
        if let Some(value) = patch.llm_circuit_breaker_cooldown_ms {
            self.llm_circuit_breaker_cooldown_ms = value;
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
        if self.llm_retry_max_attempts == 0 {
            return Err(Error::Config(
                "配置项 llm_retry_max_attempts 必须大于 0".to_owned(),
            ));
        }
        if self.llm_retry_initial_delay_ms > self.llm_retry_max_delay_ms {
            return Err(Error::Config(
                "配置项 llm_retry_initial_delay_ms 不能大于 llm_retry_max_delay_ms".to_owned(),
            ));
        }
        if self.llm_circuit_breaker_failure_threshold == 0 {
            return Err(Error::Config(
                "配置项 llm_circuit_breaker_failure_threshold 必须大于 0".to_owned(),
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
    parse_positive_u32(source, value)
}

fn parse_positive_u32(source: &str, value: &str) -> Result<u32, Error> {
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

fn parse_u64(source: &str, value: &str) -> Result<u64, Error> {
    value.trim().parse::<u64>().map_err(|error| {
        Error::Config(format!(
            "{source} 必须是非负整数，当前值为 {value:?}：{error}"
        ))
    })
}

fn parse_csv_list(_source: &str, value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|item| non_empty_string(Some(item.to_owned())))
        .collect()
}

fn normalize_string_list(value: Option<Vec<String>>) -> Option<Vec<String>> {
    value.map(|items| {
        items
            .into_iter()
            .filter_map(|item| non_empty_string(Some(item)))
            .collect()
    })
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
