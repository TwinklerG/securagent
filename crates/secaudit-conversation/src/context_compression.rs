use secaudit_agent::{ChatMessage, Role};
use std::fmt::Write;

use crate::context_usage::{ContextUsage, ContextUsageEstimator};
use crate::model::SummarySnapshot;

const DEFAULT_TRIGGER_PERCENT: u64 = 80;
const DEFAULT_TARGET_PERCENT: u64 = 60;
const SUMMARY_MAX_CHARS: usize = 8_000;
const MESSAGE_PREVIEW_MAX_CHARS: usize = 360;
const ELLIPSIS: &str = "...";
const SUMMARY_MESSAGE_PREFIX: &str =
    "以下是较早对话历史的压缩摘要，请在后续推理中作为背景使用，不要把它当作新的用户请求。";

/// Token-aware context compression policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCompressionPolicy {
    /// Compress full history once estimated context usage reaches this percent.
    pub trigger_percent: u64,
    /// Retained for configuration compatibility; compression now summarizes the covered history
    /// directly once the trigger is reached.
    pub target_percent: u64,
}

impl ContextCompressionPolicy {
    #[must_use]
    pub const fn new(trigger_percent: u64, target_percent: u64) -> Self {
        Self {
            trigger_percent,
            target_percent,
        }
    }

    #[must_use]
    pub fn build_context(
        &self,
        messages: &[ChatMessage],
        estimator: &ContextUsageEstimator,
        updated_at: String,
    ) -> CompressedContext {
        self.build_context_with_pending(messages, &[], estimator, updated_at)
    }

    #[must_use]
    pub fn build_context_with_pending(
        &self,
        messages: &[ChatMessage],
        pending_messages: &[ChatMessage],
        estimator: &ContextUsageEstimator,
        updated_at: String,
    ) -> CompressedContext {
        let full_usage = estimate_with_pending(estimator, messages, pending_messages);
        if full_usage.used_percent() < self.trigger_percent {
            return CompressedContext {
                messages: messages.to_vec(),
                summary: None,
                event: None,
                llm_summary_prompt: None,
            };
        }

        Self::build_compressed_context(
            messages,
            pending_messages,
            estimator,
            updated_at,
            &full_usage,
        )
    }

    #[must_use]
    pub fn build_context_forced(
        &self,
        messages: &[ChatMessage],
        estimator: &ContextUsageEstimator,
        updated_at: String,
    ) -> CompressedContext {
        let full_usage = estimator.estimate(messages);
        Self::build_compressed_context(messages, &[], estimator, updated_at, &full_usage)
    }

    fn build_compressed_context(
        messages: &[ChatMessage],
        pending_messages: &[ChatMessage],
        estimator: &ContextUsageEstimator,
        updated_at: String,
        full_usage: &ContextUsage,
    ) -> CompressedContext {
        let system = messages
            .iter()
            .find(|message| matches!(message.role, Role::System))
            .cloned();
        let non_system: Vec<ChatMessage> = messages
            .iter()
            .filter(|message| !matches!(message.role, Role::System))
            .cloned()
            .collect();

        let covered = non_system.as_slice();
        if covered.is_empty() {
            return CompressedContext {
                messages: messages.to_vec(),
                summary: None,
                event: None,
                llm_summary_prompt: None,
            };
        }

        let summary = SummarySnapshot {
            content: summarize_messages(covered),
            covered_message_count: covered.len(),
            updated_at,
        };
        let llm_summary_prompt = build_llm_summary_prompt(covered, estimator);

        let mut compressed = Vec::new();
        compressed.extend(system);
        compressed.push(ChatMessage::system(summary_system_content(
            &summary.content,
        )));
        let compressed_usage = estimate_with_pending(estimator, &compressed, pending_messages);
        let event = ContextCompressionEvent {
            covered_message_count: summary.covered_message_count,
            before_used_tokens: full_usage.used_tokens,
            before_used_percent: full_usage.used_percent(),
            after_used_tokens: compressed_usage.used_tokens,
            after_used_percent: compressed_usage.used_percent(),
            window_tokens: full_usage.window_tokens,
            updated_at: summary.updated_at.clone(),
        };

        CompressedContext {
            messages: compressed,
            summary: Some(summary),
            event: Some(event),
            llm_summary_prompt,
        }
    }
}

fn estimate_with_pending(
    estimator: &ContextUsageEstimator,
    messages: &[ChatMessage],
    pending_messages: &[ChatMessage],
) -> ContextUsage {
    if pending_messages.is_empty() {
        return estimator.estimate(messages);
    }

    let mut combined = Vec::with_capacity(messages.len() + pending_messages.len());
    combined.extend_from_slice(messages);
    combined.extend_from_slice(pending_messages);
    estimator.estimate(&combined)
}

impl Default for ContextCompressionPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_TRIGGER_PERCENT, DEFAULT_TARGET_PERCENT)
    }
}

#[derive(Debug, Clone)]
pub struct CompressedContext {
    pub messages: Vec<ChatMessage>,
    pub summary: Option<SummarySnapshot>,
    pub event: Option<ContextCompressionEvent>,
    pub llm_summary_prompt: Option<String>,
}

impl CompressedContext {
    pub fn apply_summary_content(&mut self, content: &str) {
        let content = truncate_chars(content, SUMMARY_MAX_CHARS);
        if let Some(summary) = &mut self.summary {
            summary.content.clone_from(&content);
        }

        let replacement = summary_system_content(&content);
        if let Some(message) = self.messages.iter_mut().find(|message| {
            matches!(message.role, Role::System)
                && message
                    .content
                    .as_deref()
                    .is_some_and(|content| content.starts_with(SUMMARY_MESSAGE_PREFIX))
        }) {
            message.content = Some(replacement);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCompressionEvent {
    pub covered_message_count: usize,
    pub before_used_tokens: u64,
    pub before_used_percent: u64,
    pub after_used_tokens: u64,
    pub after_used_percent: u64,
    pub window_tokens: u64,
    pub updated_at: String,
}

fn summarize_messages(messages: &[ChatMessage]) -> String {
    let mut summary = String::new();
    summary.push_str("# 压缩上下文摘要\n");
    let _ = writeln!(summary, "- 覆盖早期消息数：{}", messages.len());

    for message in messages {
        let Some(content) = message.content.as_deref() else {
            continue;
        };
        let role = role_label(&message.role);
        let preview = truncate_chars(&compact_text(content), MESSAGE_PREVIEW_MAX_CHARS);
        if preview.is_empty() {
            continue;
        }
        let _ = writeln!(summary, "- {role}: {preview}");
    }

    truncate_chars(&summary, SUMMARY_MAX_CHARS)
}

fn build_llm_summary_prompt(
    messages: &[ChatMessage],
    estimator: &ContextUsageEstimator,
) -> Option<String> {
    let mut selected = Vec::new();
    for message in messages.iter().rev() {
        let mut candidate = selected.clone();
        candidate.insert(0, message.clone());
        let prompt = llm_summary_prompt_from_messages(&candidate);
        if estimator.estimate_messages_tokens_uncapped(&[ChatMessage::user(prompt.clone())])
            > estimator.window_tokens()
        {
            break;
        }
        selected = candidate;
    }

    if selected.is_empty() {
        return None;
    }

    Some(llm_summary_prompt_from_messages(&selected))
}

fn llm_summary_prompt_from_messages(messages: &[ChatMessage]) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "请压缩以下对话历史，为每条用户、助手和工具消息提炼简短摘要，并保留用户目标、已完成事项、关键约束、重要文件/命令/错误、测试结果和未完成任务。\n",
    );
    prompt.push_str("输出中文结构化摘要，尽量简洁，不要添加新事实。\n\n");
    prompt.push_str("# 待压缩历史\n");

    for message in messages {
        let Some(content) = message.content.as_deref() else {
            continue;
        };
        let role = role_label(&message.role);
        let _ = writeln!(prompt, "\n## {role}\n{}", compact_text(content));
    }

    prompt
}

fn summary_system_content(summary: &str) -> String {
    format!("{SUMMARY_MESSAGE_PREFIX}\n\n{summary}")
}

fn role_label(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn compact_text(text: &str) -> String {
    let mut compact = String::new();
    let mut pending_space = false;

    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }

        if pending_space && !compact.is_empty() {
            compact.push(' ');
        }
        compact.push(ch);
        pending_space = false;
    }

    compact
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(ELLIPSIS.len()))
        .collect::<String>();
    truncated.push_str(ELLIPSIS);
    truncated
}

#[cfg(test)]
mod tests {
    use secaudit_agent::{ChatMessage, Role};

    use super::ContextCompressionPolicy;
    use crate::context_usage::ContextUsageEstimator;

    #[test]
    fn leaves_context_uncompressed_below_token_threshold() {
        let messages = vec![ChatMessage::system("sys"), ChatMessage::user("short")];
        let policy = ContextCompressionPolicy::new(80, 60);
        let estimator = ContextUsageEstimator::new(1_000);

        let context = policy.build_context(&messages, &estimator, "now".to_owned());

        assert!(context.summary.is_none());
        assert_eq!(context.messages.len(), messages.len());
    }

    #[test]
    fn compresses_older_messages_when_full_history_exceeds_window_threshold() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old user ".repeat(80)),
            ChatMessage::tool_result("call-1".to_owned(), "old tool ".repeat(80)),
            ChatMessage::user("recent user ".repeat(20)),
            ChatMessage::user("latest"),
        ];
        let policy = ContextCompressionPolicy::new(40, 30);
        let estimator = ContextUsageEstimator::new(200);

        let context = policy.build_context(&messages, &estimator, "now".to_owned());

        let summary = context.summary.expect("summary");
        let event = context.event.expect("event");
        assert!(summary.covered_message_count > 0);
        assert_eq!(event.covered_message_count, summary.covered_message_count);
        assert!(event.before_used_percent >= event.after_used_percent);
        assert!(summary.content.contains("old user"));
        assert!(context.messages.iter().any(|message| {
            matches!(message.role, Role::System)
                && message
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("压缩上下文摘要"))
        }));
        assert_eq!(context.messages.len(), 2);
        assert!(summary.content.contains("latest"));
    }

    #[test]
    fn pending_user_message_can_trigger_compression_before_agent_call() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old context ".repeat(90)),
            ChatMessage::user("latest context"),
        ];
        let pending = vec![ChatMessage::user("large current request ".repeat(160))];
        let policy = ContextCompressionPolicy::new(80, 60);
        let estimator = ContextUsageEstimator::new(1_000);

        let context =
            policy.build_context_with_pending(&messages, &pending, &estimator, "now".to_owned());

        let event = context
            .event
            .expect("pending input should trigger compaction");
        assert!(event.covered_message_count > 0);
        assert!(event.before_used_percent >= 80);
        assert!(
            context.messages.iter().all(|message| {
                message
                    .content
                    .as_deref()
                    .is_none_or(|content| !content.contains("large current request"))
            }),
            "pending input should affect budgeting but not be duplicated in returned history"
        );
    }

    #[test]
    fn does_not_start_recent_context_with_tool_message() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old ".repeat(80)),
            ChatMessage::tool_result("call-1".to_owned(), "tool ".repeat(10)),
            ChatMessage::user("latest"),
        ];
        let policy = ContextCompressionPolicy::new(30, 20);
        let estimator = ContextUsageEstimator::new(120);

        let context = policy.build_context(&messages, &estimator, "now".to_owned());
        let summary_index = context
            .messages
            .iter()
            .position(|message| {
                message
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("压缩上下文摘要"))
            })
            .expect("summary message");

        assert!(!matches!(
            context
                .messages
                .get(summary_index + 1)
                .map(|message| &message.role),
            Some(Role::Tool)
        ));
    }

    #[test]
    fn forced_compression_summarizes_even_below_token_threshold() {
        let messages = vec![ChatMessage::system("sys"), ChatMessage::user("short")];
        let policy = ContextCompressionPolicy::new(80, 60);
        let estimator = ContextUsageEstimator::new(1_000);

        let context = policy.build_context_forced(&messages, &estimator, "now".to_owned());

        let summary = context.summary.expect("summary");
        assert_eq!(summary.covered_message_count, 1);
        assert!(summary.content.contains("short"));
        assert_eq!(context.messages.len(), 2);
    }
}
