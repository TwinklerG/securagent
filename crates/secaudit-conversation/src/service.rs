use std::path::{Path, PathBuf};

use secaudit_agent::{Agent, ChatMessage, Session};

use crate::error::Result;
use crate::model::{
    ManagedSession, SessionListItem, SessionManagementInfo, SessionMetadata, SessionStatus,
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
        })
    }

    /// 使用显式根目录。
    #[must_use]
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            storage: ConversationLayout::new(root),
            sliding_window: SlidingWindowPolicy::default(),
        }
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
    ) -> Result<String> {
        let context_messages = self
            .config
            .sliding_window
            .apply(managed.session().messages());
        let context_original_len = context_messages.len();
        let mut context_session = session_with_messages(managed.session(), context_messages);

        let response = agent.chat(&mut context_session, user_message).await?;
        sync_new_messages(
            managed.session_mut(),
            &context_session,
            context_original_len,
        );
        if managed.status() == SessionStatus::Archived {
            managed.set_status(SessionStatus::Active);
        }
        self.config.storage.save_session(managed)?;
        Ok(response)
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
}

fn session_with_messages(template: &Session, messages: Vec<ChatMessage>) -> Session {
    Session {
        id: template.id.clone(),
        created_at: template.created_at.clone(),
        messages,
        work_dir: template.work_dir.clone(),
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
}
