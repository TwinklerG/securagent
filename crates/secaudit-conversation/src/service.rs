use std::path::{Path, PathBuf};
use std::slice;

use chrono::Utc;
use secaudit_agent::{Agent, ChatMessage, Session};

use crate::context_compression::{
    CompressedContext, ContextCompressionEvent, ContextCompressionPolicy,
};
use crate::context_usage::{ContextUsage, ContextUsageEstimator};
use crate::error::{Error, Result};
use crate::model::{
    ManagedSession, SessionListItem, SessionManagementInfo, SessionMetadata, SessionStatus,
    SummarySnapshot,
};
use crate::sliding_window::SlidingWindowPolicy;
use crate::storage::ConversationLayout;

/// 会话服务配置。
#[derive(Debug, Clone)]
pub struct ConversationConfig {
    /// 存储布局。
    pub storage: ConversationLayout,
    /// 滑动窗口策略。
    pub sliding_window: SlidingWindowPolicy,
    /// Token/context usage estimator shared by UI and future compression.
    pub context_usage: ContextUsageEstimator,
    /// Token-aware context compression strategy.
    pub compression: ContextCompressionPolicy,
}

/// Result of one managed chat turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatOutcome {
    /// Final assistant text returned by the agent.
    pub response: String,
    /// Context compression metadata when this turn compacted older history.
    pub compression: Option<ContextCompressionEvent>,
}

/// Result of an explicit context compaction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOutcome {
    /// Context compression metadata for the compacted history.
    pub compression: Option<ContextCompressionEvent>,
}

impl ConversationConfig {
    /// 使用默认 `~/.secaudit` 根目录。
    ///
    /// # Errors
    ///
    /// 无法推导默认根目录时返回错误。
    pub fn default_storage() -> Result<Self> {
        Ok(Self {
            storage: ConversationLayout::default_root()?,
            sliding_window: SlidingWindowPolicy::default(),
            context_usage: ContextUsageEstimator::default(),
            compression: ContextCompressionPolicy::default(),
        })
    }

    /// 使用显式根目录。
    #[must_use]
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            storage: ConversationLayout::new(root),
            sliding_window: SlidingWindowPolicy::default(),
            context_usage: ContextUsageEstimator::default(),
            compression: ContextCompressionPolicy::default(),
        }
    }

    /// Use an explicit context window from model/config metadata.
    #[must_use]
    pub fn with_context_window(mut self, window_tokens: u64) -> Self {
        self.context_usage = ContextUsageEstimator::new(window_tokens);
        self
    }

    /// Use explicit model metadata for context token estimation.
    #[must_use]
    pub fn with_context_model<S: Into<String>>(mut self, window_tokens: u64, model: S) -> Self {
        self.context_usage = ContextUsageEstimator::with_model(window_tokens, model);
        self
    }

    /// Use an explicit token-aware compression policy.
    #[must_use]
    pub const fn with_compression(mut self, compression: ContextCompressionPolicy) -> Self {
        self.compression = compression;
        self
    }
}

/// 共享会话服务。
#[derive(Debug, Clone)]
pub struct ConversationService {
    config: ConversationConfig,
}

impl ConversationService {
    /// 创建会话服务。
    #[must_use]
    pub fn new(config: ConversationConfig) -> Self {
        Self { config }
    }

    /// 使用默认配置创建会话服务。
    ///
    /// # Errors
    ///
    /// 无法推导默认持久化目录时返回错误。
    pub fn with_default_storage() -> Result<Self> {
        Ok(Self::new(ConversationConfig::default_storage()?))
    }

    /// 启动新会话。
    ///
    /// # Errors
    ///
    /// 初始化项目目录失败时返回错误。
    pub fn start_session(&self, work_dir: &Path) -> Result<ManagedSession> {
        self.config.storage.create_session(work_dir)
    }

    /// 加载已有会话。
    ///
    /// # Errors
    ///
    /// 会话不存在或读取失败时返回错误。
    pub fn load_session(&self, work_dir: &Path, session_id: &str) -> Result<ManagedSession> {
        self.config.storage.load_session(work_dir, session_id)
    }

    /// 列出项目会话。
    ///
    /// # Errors
    ///
    /// 初始化项目目录或读取索引失败时返回错误。
    pub fn list_sessions(&self, work_dir: &Path) -> Result<Vec<SessionMetadata>> {
        self.config.storage.list_sessions(work_dir)
    }

    /// 列出项目会话，并附带最近用户/助手消息预览。
    ///
    /// # Errors
    ///
    /// 初始化项目目录、读取索引或加载会话失败时返回错误。
    pub fn list_sessions_with_preview(&self, work_dir: &Path) -> Result<Vec<SessionListItem>> {
        self.config.storage.list_sessions_with_preview(work_dir)
    }

    /// 归档项目会话。
    ///
    /// # Errors
    ///
    /// 会话不存在或文件移动失败时返回错误。
    pub fn archive_session(&self, work_dir: &Path, session_id: &str) -> Result<SessionMetadata> {
        self.config.storage.archive_session(work_dir, session_id)
    }

    /// 发送一轮 chat，并在完成后保存完整历史。
    ///
    /// # Errors
    ///
    /// Agent 执行或保存会话失败时返回错误。
    pub async fn chat(
        &self,
        agent: &mut Agent,
        managed: &mut ManagedSession,
        user_message: &str,
    ) -> Result<ChatOutcome> {
        self.chat_with_compression_callback(agent, managed, user_message, |_| {})
            .await
    }

    /// Send one chat turn and notify immediately when automatic compression happens.
    ///
    /// The callback runs after a summary has been stored on the managed session and persisted.
    /// Compression can happen before the agent starts the LLM turn, or immediately after the turn
    /// if the newly appended messages cross the compression threshold.
    ///
    /// # Errors
    ///
    /// Agent execution or session persistence failure returns an error.
    pub async fn chat_with_compression_callback<F>(
        &self,
        agent: &mut Agent,
        managed: &mut ManagedSession,
        user_message: &str,
        mut on_compression: F,
    ) -> Result<ChatOutcome>
    where
        F: FnMut(&ContextCompressionEvent),
    {
        agent.ensure_chat_system_prompt(managed.session_mut());
        let agent_input = agent.build_chat_agent_input(user_message, managed.id());
        let pending_agent_message = ChatMessage::user(agent_input.clone());
        let mut compressed = self
            .build_active_context_with_pending(managed, slice::from_ref(&pending_agent_message));
        self.apply_llm_summary(agent, &mut compressed).await;
        let mut compression = compressed.event.clone();
        if let Some(event) = &mut compression {
            let after_usage = estimate_with_pending(
                &self.config.context_usage,
                &compressed.messages,
                slice::from_ref(&pending_agent_message),
            );
            event.after_used_tokens = after_usage.used_tokens;
            event.after_used_percent = after_usage.used_percent();
        }
        self.ensure_within_context_window(
            &compressed.messages,
            slice::from_ref(&pending_agent_message),
        )?;
        if let Some(summary) = compressed.summary.clone() {
            managed.set_summary(Some(summary));
            self.config.storage.save_session(managed)?;
            if let Some(event) = compression.as_ref() {
                on_compression(event);
            }
        }
        let context_messages = compressed.messages;
        let context_original_len = context_messages.len();
        let mut context_session = session_with_messages(managed.session(), context_messages);

        let response = agent
            .chat_with_agent_input(&mut context_session, user_message, agent_input)
            .await?;
        sync_new_messages(
            managed.session_mut(),
            &context_session,
            context_original_len,
        );
        if managed.status() == SessionStatus::Archived {
            managed.set_status(SessionStatus::Active);
        }
        if managed.summary().is_none() {
            let mut post_turn_compressed = self.build_active_context_with_pending(managed, &[]);
            self.apply_llm_summary(agent, &mut post_turn_compressed)
                .await;
            if let Some(summary) = post_turn_compressed.summary.clone() {
                let mut event = post_turn_compressed.event.clone();
                if let Some(event) = &mut event {
                    let after_usage = self
                        .config
                        .context_usage
                        .estimate(&post_turn_compressed.messages);
                    event.after_used_tokens = after_usage.used_tokens;
                    event.after_used_percent = after_usage.used_percent();
                }
                managed.set_summary(Some(summary));
                self.config.storage.save_session(managed)?;
                if let Some(event) = event.as_ref() {
                    on_compression(event);
                }
                if compression.is_none() {
                    compression = event;
                }
            }
        }
        self.config.storage.save_session(managed)?;
        Ok(ChatOutcome {
            response,
            compression,
        })
    }

    /// Explicitly compact the current session history.
    ///
    /// # Errors
    ///
    /// Saving the updated session summary may fail.
    pub async fn compact(
        &self,
        agent: &Agent,
        managed: &mut ManagedSession,
    ) -> Result<CompactOutcome> {
        let mut compressed = self.config.compression.build_context_forced(
            managed.session().messages(),
            &self.config.context_usage,
            Utc::now().to_rfc3339(),
        );
        self.apply_llm_summary(agent, &mut compressed).await;

        if let Some(summary) = compressed.summary.clone() {
            managed.set_summary(Some(summary));
        }
        let mut compression = compressed.event.clone();
        if let Some(event) = &mut compression {
            let after_usage = self.config.context_usage.estimate(&compressed.messages);
            event.after_used_tokens = after_usage.used_tokens;
            event.after_used_percent = after_usage.used_percent();
        }
        if managed.status() == SessionStatus::Archived {
            managed.set_status(SessionStatus::Active);
        }
        self.config.storage.save_session(managed)?;

        Ok(CompactOutcome { compression })
    }

    /// 保存当前会话。
    ///
    /// # Errors
    ///
    /// 写入失败时返回错误。
    pub fn save_session(&self, managed: &ManagedSession) -> Result<SessionMetadata> {
        self.config.storage.save_session(managed)
    }

    /// 生成会话管理投影。
    #[must_use]
    pub fn management_info(&self, managed: &ManagedSession) -> SessionManagementInfo {
        SessionManagementInfo {
            project_key: managed.project_key().to_string(),
            status: managed.status(),
            session_path: self
                .config
                .storage
                .session_path(managed)
                .display()
                .to_string(),
            storage_root: self.config.storage.root().display().to_string(),
        }
    }

    /// Estimate token-level context usage for the full session history.
    #[must_use]
    pub fn context_usage(&self, managed: &ManagedSession) -> ContextUsage {
        self.config
            .context_usage
            .estimate(managed.session().messages())
    }

    /// Estimate token-level context usage for the messages selected for the next LLM call.
    #[must_use]
    pub fn active_context_usage(&self, managed: &ManagedSession) -> ContextUsage {
        let compressed = self.build_active_context_with_pending(managed, &[]);
        self.config.context_usage.estimate(&compressed.messages)
    }

    fn build_active_context_with_pending(
        &self,
        managed: &ManagedSession,
        pending_messages: &[ChatMessage],
    ) -> CompressedContext {
        if let Some(summary) = managed.summary() {
            let messages = messages_from_existing_summary(managed, summary);
            return CompressedContext {
                messages,
                summary: None,
                event: None,
                llm_summary_prompt: None,
            };
        }

        self.config.compression.build_context_with_pending(
            managed.session().messages(),
            pending_messages,
            &self.config.context_usage,
            Utc::now().to_rfc3339(),
        )
    }

    async fn apply_llm_summary(&self, agent: &Agent, compressed: &mut CompressedContext) {
        if let Some(prompt) = compressed.llm_summary_prompt.as_deref()
            && let Ok(summary) = agent.summarize_context(prompt).await
        {
            compressed.apply_summary_content(&summary);
        }
    }

    fn ensure_within_context_window(
        &self,
        messages: &[ChatMessage],
        pending_messages: &[ChatMessage],
    ) -> Result<()> {
        let mut combined = Vec::with_capacity(messages.len() + pending_messages.len());
        combined.extend_from_slice(messages);
        combined.extend_from_slice(pending_messages);
        let used_tokens = self
            .config
            .context_usage
            .estimate_messages_tokens_uncapped(&combined);
        let window_tokens = self.config.context_usage.window_tokens();
        if used_tokens > window_tokens {
            return Err(Error::ContextTooLarge {
                used_tokens,
                window_tokens,
            });
        }

        Ok(())
    }
}

fn session_with_messages(template: &Session, messages: Vec<ChatMessage>) -> Session {
    Session {
        id: template.id.clone(),
        created_at: template.created_at.clone(),
        messages,
        work_dir: template.work_dir.clone(),
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

fn sync_new_messages(full: &mut Session, context: &Session, context_original_len: usize) {
    let appended_count = context
        .messages()
        .len()
        .saturating_sub(context_original_len);
    if appended_count == 0 {
        return;
    }

    let start = context.messages().len() - appended_count;
    for message in context.messages().iter().skip(start) {
        full.push_message(message.clone());
    }
}

fn messages_from_existing_summary(
    managed: &ManagedSession,
    summary: &SummarySnapshot,
) -> Vec<ChatMessage> {
    let system = managed
        .session()
        .messages()
        .iter()
        .find(|message| matches!(message.role, secaudit_agent::Role::System))
        .cloned();
    let remaining = managed
        .session()
        .messages()
        .iter()
        .filter(|message| !matches!(message.role, secaudit_agent::Role::System))
        .skip(summary.covered_message_count)
        .cloned();

    let mut messages = Vec::new();
    messages.extend(system);
    messages.push(ChatMessage::system(format!(
        "以下是较早对话历史的压缩摘要，请在后续推理中作为背景使用，不要把它当作新的用户请求。\n\n{}",
        summary.content
    )));
    messages.extend(remaining);
    messages
}

#[cfg(test)]
mod tests {
    use std::fs;

    use secaudit_agent::ChatMessage;
    use tempfile::TempDir;

    use super::{ConversationConfig, ConversationService};
    use crate::sliding_window::SlidingWindowPolicy;

    #[test]
    fn service_creates_and_lists_session() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));

        let managed = service.start_session(&work_dir).expect("start session");
        assert!(
            service
                .list_sessions(&work_dir)
                .expect("list empty sessions")
                .is_empty()
        );
        let result = service.save_session(&managed).err();
        assert!(result.is_some(), "empty sessions should not persist");
    }

    #[test]
    fn service_lists_saved_session_after_content_exists() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));

        let mut managed = service.start_session(&work_dir).expect("start session");
        managed
            .session_mut()
            .push_message(ChatMessage::user("hello"));
        service.save_session(&managed).expect("save session");

        let sessions = service.list_sessions(&work_dir).expect("list sessions");

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions.first().map(|session| session.session_id.as_str()),
            Some(managed.id())
        );
    }

    #[test]
    fn sliding_window_does_not_mutate_full_history() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let mut config = ConversationConfig::with_root(temp.path().join("runtime"));
        config.sliding_window = SlidingWindowPolicy::new(1);
        let service = ConversationService::new(config);
        let mut managed = service.start_session(&work_dir).expect("start session");

        managed
            .session_mut()
            .push_message(ChatMessage::system("sys"));
        managed.session_mut().push_message(ChatMessage::user("u1"));
        managed.session_mut().push_message(ChatMessage::user("u2"));
        service.save_session(&managed).expect("save session");

        let loaded = service
            .load_session(&work_dir, managed.id())
            .expect("load session");

        assert_eq!(loaded.session().messages().len(), 3);
    }

    #[test]
    fn active_context_keeps_full_history_until_token_threshold() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let mut config = ConversationConfig::with_root(temp.path().join("runtime"));
        config.sliding_window = SlidingWindowPolicy::new(1);
        config = config.with_context_window(10_000);
        let service = ConversationService::new(config);
        let mut managed = service.start_session(&work_dir).expect("start session");

        managed
            .session_mut()
            .push_message(ChatMessage::system("sys"));
        managed.session_mut().push_message(ChatMessage::user("u1"));
        managed.session_mut().push_message(ChatMessage::user("u2"));

        let full_usage = service.context_usage(&managed);
        let active_usage = service.active_context_usage(&managed);

        assert_eq!(active_usage.used_tokens, full_usage.used_tokens);
    }

    #[test]
    fn active_context_compresses_history_above_token_threshold() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let config =
            ConversationConfig::with_root(temp.path().join("runtime")).with_context_window(1_900);
        let service = ConversationService::new(config);
        let mut managed = service.start_session(&work_dir).expect("start session");

        managed
            .session_mut()
            .push_message(ChatMessage::system("sys"));
        managed
            .session_mut()
            .push_message(ChatMessage::user("old ".repeat(1_600)));
        managed
            .session_mut()
            .push_message(ChatMessage::user("middle context"));
        managed
            .session_mut()
            .push_message(ChatMessage::user("latest request"));

        let full_usage = service.context_usage(&managed);
        let active_usage = service.active_context_usage(&managed);

        assert!(
            full_usage.used_percent() >= 80,
            "fixture should exceed compression threshold"
        );
        assert!(
            active_usage.used_tokens < full_usage.used_tokens,
            "active context should be smaller after compression"
        );
    }
}
