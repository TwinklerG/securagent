use secaudit_agent::{ChatMessage, Role, TokenUsage};

/// Default context window used when the caller does not provide model metadata.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;

/// Token-level context usage for the current conversation view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextUsage {
    pub window_tokens: u64,
    pub used_tokens: u64,
    pub free_tokens: u64,
    pub system_tokens: u64,
    pub tool_tokens: u64,
    pub message_tokens: u64,
    pub token_estimator: ContextTokenEstimator,
    pub cumulative_usage: TokenUsage,
}

impl ContextUsage {
    #[must_use]
    pub fn used_percent(&self) -> u64 {
        percent(self.used_tokens, self.window_tokens)
    }

    #[must_use]
    pub fn system_percent(&self) -> u64 {
        percent(self.system_tokens, self.window_tokens)
    }

    #[must_use]
    pub fn tool_percent(&self) -> u64 {
        percent(self.tool_tokens, self.window_tokens)
    }

    #[must_use]
    pub fn message_percent(&self) -> u64 {
        percent(self.message_tokens, self.window_tokens)
    }

    #[must_use]
    pub fn free_percent(&self) -> u64 {
        percent(self.free_tokens, self.window_tokens)
    }
}

impl Default for ContextUsage {
    fn default() -> Self {
        Self {
            window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            used_tokens: 0,
            free_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            system_tokens: 0,
            tool_tokens: 0,
            message_tokens: 0,
            token_estimator: ContextTokenEstimator::CharacterApproximation,
            cumulative_usage: TokenUsage::default(),
        }
    }
}

/// Token counting backend used by [`ContextUsageEstimator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTokenEstimator {
    /// Counts message content with a tiktoken-compatible model tokenizer.
    Tiktoken,
    /// Falls back to a character-based approximation when no tokenizer is known.
    CharacterApproximation,
}

impl ContextTokenEstimator {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tiktoken => "tiktoken",
            Self::CharacterApproximation => "character estimate",
        }
    }
}

/// Internal policy used by UI, sliding-window, and future compression logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsageEstimator {
    window_tokens: u64,
    model: Option<String>,
    token_estimator: ContextTokenEstimator,
}

impl ContextUsageEstimator {
    #[must_use]
    pub const fn new(window_tokens: u64) -> Self {
        Self {
            window_tokens,
            model: None,
            token_estimator: ContextTokenEstimator::CharacterApproximation,
        }
    }

    #[must_use]
    pub fn with_model<S: Into<String>>(window_tokens: u64, model: S) -> Self {
        let model = model.into();
        let token_estimator = if supports_tiktoken_model(&model) {
            ContextTokenEstimator::Tiktoken
        } else {
            ContextTokenEstimator::CharacterApproximation
        };

        Self {
            window_tokens,
            model: Some(model),
            token_estimator,
        }
    }

    #[must_use]
    pub fn estimate(&self, messages: &[ChatMessage]) -> ContextUsage {
        let mut system_tokens = 0u64;
        let mut tool_tokens = 0u64;
        let mut message_tokens = 0u64;

        for message in messages {
            let tokens = self.estimate_message_tokens(message);
            match message.role {
                Role::System => system_tokens = system_tokens.saturating_add(tokens),
                Role::Tool => tool_tokens = tool_tokens.saturating_add(tokens),
                Role::User | Role::Assistant => {
                    message_tokens = message_tokens.saturating_add(tokens);
                }
            }
        }

        let used_tokens = system_tokens
            .saturating_add(tool_tokens)
            .saturating_add(message_tokens)
            .min(self.window_tokens);
        let free_tokens = self.window_tokens.saturating_sub(used_tokens);

        ContextUsage {
            window_tokens: self.window_tokens,
            used_tokens,
            free_tokens,
            system_tokens: system_tokens.min(self.window_tokens),
            tool_tokens: tool_tokens.min(self.window_tokens),
            message_tokens: message_tokens.min(self.window_tokens),
            token_estimator: self.token_estimator,
            cumulative_usage: TokenUsage::sum_from_messages(messages),
        }
    }

    fn estimate_message_tokens(&self, message: &ChatMessage) -> u64 {
        const MESSAGE_OVERHEAD_TOKENS: u64 = 4;
        let content_tokens = message
            .content
            .as_deref()
            .map_or(0, |content| self.estimate_text_tokens(content));
        MESSAGE_OVERHEAD_TOKENS + content_tokens
    }

    fn estimate_text_tokens(&self, text: &str) -> u64 {
        if let Some(model) = &self.model
            && let Some(tokens) = count_tiktoken_text_tokens(model, text)
        {
            return tokens;
        }

        estimate_text_tokens_by_chars(text)
    }
}

impl Default for ContextUsageEstimator {
    fn default() -> Self {
        Self::new(DEFAULT_CONTEXT_WINDOW_TOKENS)
    }
}

fn supports_tiktoken_model(model: &str) -> bool {
    tiktoken::encoding_for_model(model).is_some()
        || model
            .rsplit_once('/')
            .is_some_and(|(_, model_name)| tiktoken::encoding_for_model(model_name).is_some())
}

fn count_tiktoken_text_tokens(model: &str, text: &str) -> Option<u64> {
    let encoding = tiktoken::encoding_for_model(model).or_else(|| {
        let (_, model_name) = model.rsplit_once('/')?;
        tiktoken::encoding_for_model(model_name)
    })?;

    Some(u64::try_from(encoding.count(text)).unwrap_or(u64::MAX))
}

fn estimate_text_tokens_by_chars(text: &str) -> u64 {
    let char_count = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
    char_count.div_ceil(4)
}

fn percent(value: u64, total: u64) -> u64 {
    value.saturating_mul(100).checked_div(total).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use secaudit_agent::ChatMessage;

    use super::{ContextTokenEstimator, ContextUsageEstimator, DEFAULT_CONTEXT_WINDOW_TOKENS};

    #[test]
    fn estimates_context_usage_by_role() {
        let messages = vec![
            ChatMessage::system("system prompt"),
            ChatMessage::user("please audit this file"),
            ChatMessage::tool_result("call-1", "tool output"),
        ];

        let usage = ContextUsageEstimator::default().estimate(&messages);

        assert_eq!(usage.window_tokens, DEFAULT_CONTEXT_WINDOW_TOKENS);
        assert!(usage.system_tokens > 0);
        assert!(usage.tool_tokens > 0);
        assert!(usage.message_tokens > 0);
        assert_eq!(
            usage.used_tokens,
            usage.system_tokens + usage.tool_tokens + usage.message_tokens
        );
        assert_eq!(
            usage.token_estimator,
            ContextTokenEstimator::CharacterApproximation
        );
    }

    #[test]
    fn uses_tiktoken_when_model_is_supported() {
        let messages = vec![ChatMessage::user("hello world")];

        let usage = ContextUsageEstimator::with_model(DEFAULT_CONTEXT_WINDOW_TOKENS, "gpt-4o")
            .estimate(&messages);

        assert_eq!(usage.token_estimator, ContextTokenEstimator::Tiktoken);
        assert_eq!(usage.message_tokens, 6);
    }

    #[test]
    fn uses_tiktoken_for_openrouter_openai_model_names() {
        let messages = vec![ChatMessage::user("hello world")];

        let usage =
            ContextUsageEstimator::with_model(DEFAULT_CONTEXT_WINDOW_TOKENS, "openai/gpt-4o")
                .estimate(&messages);

        assert_eq!(usage.token_estimator, ContextTokenEstimator::Tiktoken);
        assert_eq!(usage.message_tokens, 6);
    }

    #[test]
    fn falls_back_when_model_tokenizer_is_unknown() {
        let messages = vec![ChatMessage::user("hello world")];

        let usage =
            ContextUsageEstimator::with_model(DEFAULT_CONTEXT_WINDOW_TOKENS, "unknown-model")
                .estimate(&messages);

        assert_eq!(
            usage.token_estimator,
            ContextTokenEstimator::CharacterApproximation
        );
    }
}
