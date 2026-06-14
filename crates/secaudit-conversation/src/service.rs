use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::slice;

use chrono::Utc;
use secaudit_agent::{Agent, ChatMessage, HttpLlmClient, Session};
use secaudit_memory::{FileMemoryStore, Finding, FindingStatus, MemoryStore, ProjectMemory};

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

/// Short-term sliding window: use the latest N memory records per session.
const SUMMARIES_WINDOW: usize = 8;

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
    pub async fn chat<M: MemoryStore>(
        &self,
        agent: &mut Agent,
        managed: &mut ManagedSession,
        user_message: &str,
        memory: Option<&M>,
        llm: Option<&HttpLlmClient>,
    ) -> Result<ChatOutcome> {
        self.chat_with_compression_callback(agent, managed, user_message, memory, llm, |_| {})
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
    pub async fn chat_with_compression_callback<M, F>(
        &self,
        agent: &mut Agent,
        managed: &mut ManagedSession,
        user_message: &str,
        memory: Option<&M>,
        llm: Option<&HttpLlmClient>,
        mut on_compression: F,
    ) -> Result<ChatOutcome>
    where
        M: MemoryStore,
        F: FnMut(&ContextCompressionEvent),
    {
        let memory_context = memory
            .map(|mem| build_memory_context(mem, managed.id()))
            .unwrap_or_default();

        agent.ensure_chat_system_prompt(managed.session_mut());
        let agent_input = agent.build_chat_agent_input(user_message, managed.id());
        let pending_agent_message = ChatMessage::user(agent_input.clone());
        let mut compressed = self
            .build_active_context_with_pending(managed, slice::from_ref(&pending_agent_message));
        self.apply_llm_summary(agent, &mut compressed).await;
        if !memory_context.is_empty() {
            compressed
                .messages
                .push(ChatMessage::system(memory_context));
        }
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

        if let Some(mem) = memory
            && let Some(client) = llm
        {
            let result = compress_response(client, &response).await;
            if !result.summary.is_empty() {
                mem.record_chat(managed.id(), &result.summary, result.importance)?;
            }
            if !result.findings.is_empty() {
                mem.merge_to_long_term(managed.id(), &result.findings)?;
            }
        }

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

    /// 创建项目关联的 Memory 存储。
    ///
    /// # Errors
    ///
    /// 项目初始化失败时返回错误。
    pub fn create_memory_store(&self, work_dir: &Path) -> Result<FileMemoryStore> {
        let project = self.config.storage.ensure_project(work_dir)?;
        Ok(FileMemoryStore::new(
            self.config.storage.memory_dir(&project.project_key),
        ))
    }

    /// 完成会话记忆最终化：将 L1 摘要拼接为 L2 文本总结，聚合 L3 项目知识。
    ///
    /// # Errors
    ///
    /// Memory 读写失败时返回错误。
    pub fn finalize_session<M: MemoryStore>(memory: &M, session_id: &str) -> Result<()> {
        let records = memory.recent_by_session(session_id, usize::MAX)?;
        if records.is_empty() {
            return Ok(());
        }

        let content: Vec<&str> = records
            .iter()
            .rev()
            .map(|r| r.content.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let summary_text = content.join("\n");

        if !summary_text.is_empty() {
            memory.finalize_long_term(session_id, &summary_text)?;
        }

        let all = memory.all_summaries()?;
        if let Some(current) = all.iter().find(|s| s.session_id == session_id)
            && !current.findings.is_empty()
        {
            aggregate_project_knowledge(memory, &current.findings)?;
        }

        memory.clear_short_term(session_id)?;

        Ok(())
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

fn build_memory_context<M: MemoryStore>(store: &M, session_id: &str) -> String {
    let mut parts = Vec::new();

    match store.recent_by_session(session_id, SUMMARIES_WINDOW) {
        Ok(records) => {
            if !records.is_empty() {
                let lines: Vec<String> = records
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("[{}] {}", i + 1, r.content))
                    .collect();
                parts.push(format!(
                    "【本会话最近 {} 条记录】\n{}",
                    records.len(),
                    lines.join("\n")
                ));
            }
        }
        Err(e) => tracing::warn!("[memory] short-term 读取失败: {e}"),
    }

    match store.all_summaries() {
        Ok(summaries) => {
            let others: Vec<_> = summaries
                .iter()
                .filter(|s| s.session_id != session_id)
                .collect();
            if !others.is_empty() {
                let lines: Vec<String> = others
                    .iter()
                    .map(|s| {
                        format!(
                            "会话 {}: {}",
                            &s.session_id[..s.session_id.len().min(8)],
                            s.content
                        )
                    })
                    .collect();
                parts.push(format!(
                    "【历史 {} 个会话总结】\n{}",
                    others.len(),
                    lines.join("\n")
                ));
            }
        }
        Err(e) => tracing::warn!("[memory] long-term 读取失败: {e}"),
    }

    match store.project_facts() {
        Ok(facts) => {
            if !facts.is_empty() {
                let lines: Vec<String> = facts
                    .iter()
                    .map(|f| format!("- {}: {}", f.key, f.content))
                    .collect();
                parts.push(format!("【项目知识】\n{}", lines.join("\n")));
            }
        }
        Err(e) => tracing::warn!("[memory] project facts 读取失败: {e}"),
    }

    if parts.is_empty() {
        return String::new();
    }

    format!(
        "【审计上下文 - 以下为系统内部参考信息，请勿直接复述】\n\n{}\n\n---\n基于以上上下文继续审计。",
        parts.join("\n\n")
    )
}

fn aggregate_project_knowledge<M: ProjectMemory>(memory: &M, findings: &[Finding]) -> Result<()> {
    if findings.is_empty() {
        return Ok(());
    }

    let mut cwe_counts: HashMap<String, usize> = HashMap::new();
    let mut file_findings: HashMap<String, Vec<String>> = HashMap::new();

    for finding in findings {
        if let Some(cwe) = &finding.cwe_id {
            *cwe_counts.entry(cwe.clone()).or_insert(0) += 1;
        }
        if let Some(file) = &finding.file_path {
            file_findings.entry(file.clone()).or_default().push(format!(
                "{} ({})",
                finding.cwe_id.as_deref().unwrap_or("?"),
                finding.status
            ));
        }
    }

    if !cwe_counts.is_empty() {
        let stats = serde_json::to_string(&cwe_counts).unwrap_or_default();
        if let Ok(existing) = memory.project_facts()
            && let Some(old_stats) = existing.iter().find(|f| f.key == "cwe_stats")
            && let Ok(mut merged) =
                serde_json::from_str::<HashMap<String, usize>>(&old_stats.content)
        {
            for (cwe, count) in &cwe_counts {
                *merged.entry(cwe.clone()).or_insert(0) += count;
            }
            let merged_json = serde_json::to_string(&merged).unwrap_or_default();
            memory.upsert_project_fact("cwe_stats", &merged_json)?;
        } else {
            memory.upsert_project_fact("cwe_stats", &stats)?;
        }
    }

    for (file, items) in &file_findings {
        let key = format!("file:{file}");
        let new_content = items.join("; ");

        let merged = if let Ok(facts) = memory.project_facts()
            && let Some(old) = facts.iter().find(|f| f.key == key)
        {
            let mut all: Vec<&str> = old.content.split("; ").filter(|s| !s.is_empty()).collect();
            for item in new_content.split("; ").filter(|s| !s.is_empty()) {
                if !all.contains(&item) {
                    all.push(item);
                }
            }
            all.join("; ")
        } else {
            new_content
        };

        memory.upsert_project_fact(&key, &merged)?;
    }

    Ok(())
}

struct CompressResult {
    summary: String,
    importance: f64,
    findings: Vec<Finding>,
}

async fn compress_response(client: &HttpLlmClient, response: &str) -> CompressResult {
    let prompt = r#"你是审计发现提取器。对审计回复，提取关键发现。

输出 JSON（不要输出其他内容）：
{
  "summary": "一行中文摘要",
  "importance": 2.0,
  "findings": [
    {"cwe_id": "CWE-89", "file_path": "src/auth.py", "line": 12, "status": "fixed"}
  ]
}

importance 取值: 含漏洞发现 -> 2.0~5.0, 纯信息 -> 1.0, 无关键发现 -> 0.3
status 取值: fixed / pending / confirmed / false_positive / needs_analysis
如无任何发现，findings 为空数组 []。"#;
    let messages = [
        ChatMessage::system(prompt),
        ChatMessage::user(response.to_owned()),
    ];
    let raw = client
        .chat(&messages, None)
        .await
        .map(|msg| msg.content.unwrap_or_default())
        .unwrap_or_default();

    if raw.is_empty() {
        tracing::warn!("[memory] compress_response LLM 返回空内容，本轮发现未记录");
    }

    parse_compress_json(&raw)
}

fn parse_compress_json(raw: &str) -> CompressResult {
    let json_str = raw
        .strip_prefix("```json")
        .and_then(|s| s.strip_suffix("```"))
        .or_else(|| {
            let s = raw.strip_prefix("```")?;
            s.strip_suffix("```")
        })
        .unwrap_or(raw)
        .trim();

    match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(val) => {
            let summary = val
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let importance = val
                .get("importance")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            let findings: Vec<Finding> = val
                .get("findings")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|finding| {
                            let status = finding
                                .get("status")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(FindingStatus::Pending);
                            Finding {
                                cwe_id: finding
                                    .get("cwe_id")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                file_path: finding
                                    .get("file_path")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                                line: finding
                                    .get("line")
                                    .and_then(serde_json::Value::as_u64)
                                    .map(|v| v as u32),
                                status,
                                timestamp: Utc::now().to_rfc3339(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            CompressResult {
                summary,
                importance,
                findings,
            }
        }
        Err(_) => CompressResult {
            summary: raw.to_owned(),
            importance: 0.3,
            findings: Vec::new(),
        },
    }
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
    use secaudit_memory::{
        FileMemoryStore, Finding, FindingStatus, LongTermMemory, ProjectMemory, ShortTermMemory,
    };
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

    #[test]
    fn parse_valid_compress_json() {
        let raw = r#"{"summary": "发现 SQL 注入", "importance": 3.0, "findings": [{"cwe_id": "CWE-89", "file_path": "src/auth.py", "line": 12, "status": "fixed"}]}"#;
        let result = super::parse_compress_json(raw);

        assert_eq!(result.summary, "发现 SQL 注入");
        assert!((result.importance - 3.0).abs() < f64::EPSILON);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].cwe_id.as_deref(), Some("CWE-89"));
        assert_eq!(result.findings[0].status, FindingStatus::Fixed);
    }

    #[test]
    fn parse_compress_json_with_markdown_fence() {
        let raw = "```json\n{\"summary\": \"ok\", \"importance\": 1.0, \"findings\": []}\n```";
        let result = super::parse_compress_json(raw);

        assert_eq!(result.summary, "ok");
        assert!(result.findings.is_empty());
    }

    #[test]
    fn parse_compress_json_fallback_on_invalid() {
        let raw = "这不是有效的 JSON";
        let result = super::parse_compress_json(raw);

        assert_eq!(result.summary, raw);
        assert!((result.importance - 0.3).abs() < f64::EPSILON);
        assert!(result.findings.is_empty());
    }

    #[test]
    fn parse_compress_json_unknown_status_defaults_to_pending() {
        let raw = r#"{"summary": "x", "importance": 1.0, "findings": [{"cwe_id": "CWE-89", "file_path": "a.py", "status": "unknown_status"}]}"#;
        let result = super::parse_compress_json(raw);

        assert_eq!(result.findings[0].status, FindingStatus::Pending);
    }

    #[test]
    fn aggregate_cwe_stats_accumulates() {
        let temp = TempDir::new().expect("create tempdir");
        let store = FileMemoryStore::new(temp.path().join("memory"));

        let findings = vec![
            Finding {
                cwe_id: Some("CWE-89".into()),
                file_path: Some("a.py".into()),
                line: None,
                status: FindingStatus::Fixed,
                timestamp: "t1".into(),
            },
            Finding {
                cwe_id: Some("CWE-89".into()),
                file_path: Some("b.py".into()),
                line: None,
                status: FindingStatus::Pending,
                timestamp: "t2".into(),
            },
            Finding {
                cwe_id: Some("CWE-79".into()),
                file_path: None,
                line: None,
                status: FindingStatus::Confirmed,
                timestamp: "t3".into(),
            },
        ];

        super::aggregate_project_knowledge(&store, &findings).expect("aggregate");
        let facts = store.project_facts().expect("read");

        assert!(facts.iter().any(|fact| fact.key == "cwe_stats"));
        assert!(facts.iter().any(|fact| fact.key == "file:a.py"));
    }

    #[test]
    fn full_pipeline_finalize_and_cross_session_context() {
        let temp = TempDir::new().expect("create tempdir");
        let store = FileMemoryStore::new(temp.path().join("memory"));

        let session_1 = "session-1";
        let session_2 = "session-2";

        store
            .record_chat(session_1, "CWE-89 SQL注入 @ auth.py (已修复)", 2.0)
            .expect("L1");
        store
            .merge_to_long_term(
                session_1,
                &[Finding {
                    cwe_id: Some("CWE-89".into()),
                    file_path: Some("auth.py".into()),
                    line: Some(12),
                    status: FindingStatus::Fixed,
                    timestamp: "t1".into(),
                }],
            )
            .expect("L2");
        store
            .record_chat(session_1, "CWE-79 XSS @ utils.py (待修复)", 2.0)
            .expect("L1");
        store
            .merge_to_long_term(
                session_1,
                &[Finding {
                    cwe_id: Some("CWE-79".into()),
                    file_path: Some("utils.py".into()),
                    line: Some(45),
                    status: FindingStatus::Pending,
                    timestamp: "t2".into(),
                }],
            )
            .expect("L2");

        ConversationService::finalize_session(&store, session_1).expect("finalize s1");

        let summaries = store.all_summaries().expect("read L2");
        assert_eq!(summaries.len(), 1);
        let s1_summary = &summaries[0];
        assert_eq!(s1_summary.session_id, session_1);
        assert!(s1_summary.content.contains("CWE-89"));
        assert!(s1_summary.content.contains("CWE-79"));
        assert_eq!(s1_summary.findings.len(), 2);

        let facts = store.project_facts().expect("read L3");
        assert!(facts.iter().any(|fact| fact.key == "cwe_stats"));
        assert!(facts.iter().any(|fact| fact.key.starts_with("file:")));

        let ctx = super::build_memory_context(&store, session_2);
        assert!(ctx.contains("历史"));
        assert!(ctx.contains("CWE-89"));
        assert!(ctx.contains("项目知识"));
        assert!(ctx.contains("cwe_stats"));
    }

    #[test]
    fn finalize_empty_session_returns_ok() {
        let temp = TempDir::new().expect("create tempdir");
        let store = FileMemoryStore::new(temp.path().join("memory"));

        let result = ConversationService::finalize_session(&store, "empty-session");
        result.expect("empty session finalize should succeed");
        let summaries = store.all_summaries().expect("read L2");
        assert!(summaries.is_empty());
    }
}
