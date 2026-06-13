use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

use secaudit_agent::{ChatMessage, Role, Session};
use secaudit_storage::{ACTIVE_DIR, ARCHIVED_DIR};
use serde::{Deserialize, Serialize};

/// 当前持久化 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;
const SESSION_PREVIEW_MAX_CHARS: usize = 160;
const ELLIPSIS: &str = "...";

/// 可读、可人工复原的项目键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectKey(String);

impl ProjectKey {
    /// 从工作目录生成项目键。
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        let display = path.to_string_lossy();
        let encoded = display.chars().map(encode_project_key_char).collect();
        Self(encoded)
    }

    /// 返回项目键字符串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn with_suffix(&self, suffix: &str) -> Self {
        Self(format!("{}--{suffix}", self.0))
    }
}

impl Display for ProjectKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn encode_project_key_char(ch: char) -> char {
    if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
        ch
    } else {
        '-'
    }
}

/// 项目级元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMetadata {
    /// schema 版本。
    pub schema_version: u32,
    /// 项目键。
    pub project_key: ProjectKey,
    /// 规范化项目路径。
    pub canonical_path: PathBuf,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

/// 会话状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// 活跃会话。
    Active,
    /// 已归档会话。
    Archived,
}

impl SessionStatus {
    /// 稳定字符串表示。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => ACTIVE_DIR,
            Self::Archived => ARCHIVED_DIR,
        }
    }

    /// 存储子目录名。
    #[must_use]
    pub const fn directory(self) -> &'static str {
        self.as_str()
    }

    /// 另一种持久化状态。
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Active => Self::Archived,
            Self::Archived => Self::Active,
        }
    }
}

impl Display for SessionStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 摘要压缩预留字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummarySnapshot {
    /// 摘要文本。
    pub content: String,
    /// 摘要覆盖到的消息数量。
    pub covered_message_count: usize,
    /// 摘要更新时间。
    pub updated_at: String,
}

/// 磁盘上的完整会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// schema 版本。
    pub schema_version: u32,
    /// 会话 ID。
    pub id: String,
    /// 项目键。
    pub project_key: ProjectKey,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
    /// 会话状态。
    pub status: SessionStatus,
    /// 会话标题。
    pub title: String,
    /// 工作目录。
    pub work_dir: PathBuf,
    /// 完整消息历史。
    pub messages: Vec<ChatMessage>,
    /// 摘要压缩预留。
    pub summary: Option<SummarySnapshot>,
}

impl StoredSession {
    /// 从 Agent 会话构造存储会话。
    #[must_use]
    pub fn from_session(
        session: &Session,
        project_key: ProjectKey,
        status: SessionStatus,
        title: String,
        updated_at: String,
        summary: Option<SummarySnapshot>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: session.id.clone(),
            project_key,
            created_at: session.created_at.clone(),
            updated_at,
            status,
            title,
            work_dir: session.work_dir.clone(),
            messages: session.messages().to_vec(),
            summary,
        }
    }

    /// 转回 Agent 会话。
    #[must_use]
    pub fn into_agent_session(self) -> Session {
        Session {
            id: self.id,
            created_at: self.created_at,
            messages: self.messages,
            work_dir: self.work_dir,
        }
    }
}

/// 会话索引投影。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMetadata {
    /// schema 版本。
    pub schema_version: u32,
    /// 会话 ID。
    pub session_id: String,
    /// 会话状态。
    pub status: SessionStatus,
    /// 会话标题。
    pub title: String,
    /// 创建时间。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
    /// 消息数量。
    pub message_count: usize,
    /// 相对会话文件路径。
    pub file: String,
}

impl SessionMetadata {
    /// 从完整会话构造索引投影。
    #[must_use]
    pub fn from_stored_session(session: &StoredSession) -> Self {
        let file = format!("{}/{}.json", session.status.directory(), session.id);
        Self {
            schema_version: SCHEMA_VERSION,
            session_id: session.id.clone(),
            status: session.status,
            title: session.title.clone(),
            created_at: session.created_at.clone(),
            updated_at: session.updated_at.clone(),
            message_count: session.messages.len(),
            file,
        }
    }
}

/// 会话列表预览中的消息角色。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPreviewRole {
    /// 用户消息。
    User,
    /// 助手消息。
    Assistant,
}

impl SessionPreviewRole {
    /// 面向终端/UI 的短标签。
    #[must_use]
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
        }
    }
}

/// 会话列表预览。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPreview {
    /// 预览来源角色。
    pub role: SessionPreviewRole,
    /// 压缩到单行后的预览文本。
    pub content: String,
}

impl SessionPreview {
    pub(crate) fn from_messages(messages: &[ChatMessage]) -> Option<Self> {
        messages.iter().rev().find_map(Self::from_message)
    }

    /// 格式化为入口层可直接展示的单行文本。
    #[must_use]
    pub fn display_text(&self, max_chars: usize) -> String {
        format!(
            "{}: {}",
            self.role.display_label(),
            truncate_chars(&self.content, max_chars)
        )
    }

    fn from_message(message: &ChatMessage) -> Option<Self> {
        let role = match &message.role {
            Role::User => SessionPreviewRole::User,
            Role::Assistant => SessionPreviewRole::Assistant,
            Role::System | Role::Tool => return None,
        };
        let content = compact_preview(message.content.as_deref()?)?;

        Some(Self { role, content })
    }
}

/// 会话列表项，包含元数据与面向 UI 的最近消息预览。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionListItem {
    /// 会话元数据。
    pub metadata: SessionMetadata,
    /// 最近一条用户或助手消息的单行预览。
    pub preview: Option<SessionPreview>,
}

impl SessionListItem {
    /// 从完整会话构造列表投影。
    #[must_use]
    pub fn from_stored_session(session: &StoredSession) -> Self {
        Self {
            metadata: SessionMetadata::from_stored_session(session),
            preview: SessionPreview::from_messages(&session.messages),
        }
    }

    /// 面向入口层的会话预览文本。
    #[must_use]
    pub fn preview_text(&self, max_chars: usize) -> String {
        self.preview.as_ref().map_or_else(
            || "无用户/助手消息".to_owned(),
            |preview| preview.display_text(max_chars),
        )
    }
}

/// 入口层使用的托管会话。
pub struct ManagedSession {
    session: Session,
    project_key: ProjectKey,
    status: SessionStatus,
    title: String,
    summary: Option<SummarySnapshot>,
}

pub(crate) fn has_persistable_messages(messages: &[ChatMessage]) -> bool {
    SessionPreview::from_messages(messages).is_some()
}

fn compact_preview(text: &str) -> Option<String> {
    let mut preview = String::new();
    let mut pending_space = false;

    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }

        if pending_space && !preview.is_empty() {
            preview.push(' ');
        }
        preview.push(ch);
        pending_space = false;
    }

    if preview.is_empty() {
        None
    } else {
        Some(truncate_chars(&preview, SESSION_PREVIEW_MAX_CHARS))
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}{ELLIPSIS}")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use secaudit_agent::ChatMessage;

    use super::{
        ProjectKey, SCHEMA_VERSION, SessionListItem, SessionPreviewRole, SessionStatus,
        StoredSession,
    };

    fn stored_with_messages(messages: Vec<ChatMessage>) -> StoredSession {
        StoredSession {
            schema_version: SCHEMA_VERSION,
            id: "session-1".to_owned(),
            project_key: ProjectKey::from_path(Path::new("/tmp/project")),
            created_at: "2026-05-22T00:00:00Z".to_owned(),
            updated_at: "2026-05-22T00:00:00Z".to_owned(),
            status: SessionStatus::Active,
            title: "未命名会话".to_owned(),
            work_dir: PathBuf::from("/tmp/project"),
            messages,
            summary: None,
        }
    }

    #[test]
    fn list_item_preview_prefers_latest_user_or_assistant_message() {
        let item = SessionListItem::from_stored_session(&stored_with_messages(vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hello"),
            ChatMessage::tool_result("call-1", "tool"),
            ChatMessage::user("latest\nmessage"),
        ]));

        assert!(matches!(
            item.preview.as_ref(),
            Some(preview)
                if preview.role == SessionPreviewRole::User
                    && preview.content == "latest message"
        ));
        assert_eq!(item.preview_text(80), "User: latest message");
    }

    #[test]
    fn list_item_preview_text_has_empty_fallback_and_truncation() {
        let empty = SessionListItem::from_stored_session(&stored_with_messages(vec![
            ChatMessage::system("sys"),
            ChatMessage::tool_result("call-1", "tool"),
        ]));
        let long =
            SessionListItem::from_stored_session(&stored_with_messages(vec![ChatMessage::user(
                "abcdef",
            )]));

        assert_eq!(empty.preview_text(80), "无用户/助手消息");
        assert_eq!(long.preview_text(3), "User: abc...");
    }
}

impl ManagedSession {
    /// 创建托管会话。
    #[must_use]
    pub fn new(
        session: Session,
        project_key: ProjectKey,
        status: SessionStatus,
        title: String,
        summary: Option<SummarySnapshot>,
    ) -> Self {
        Self {
            session,
            project_key,
            status,
            title,
            summary,
        }
    }

    /// 不可变访问内部 Agent 会话。
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// 可变访问内部 Agent 会话。
    #[must_use]
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// 会话 ID。
    #[must_use]
    pub fn id(&self) -> &str {
        &self.session.id
    }

    /// 项目键。
    #[must_use]
    pub fn project_key(&self) -> &ProjectKey {
        &self.project_key
    }

    /// 会话状态。
    #[must_use]
    pub const fn status(&self) -> SessionStatus {
        self.status
    }

    /// 设置状态。
    pub fn set_status(&mut self, status: SessionStatus) {
        self.status = status;
    }

    /// 会话标题。
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 当前会话摘要。
    #[must_use]
    pub const fn summary(&self) -> Option<&SummarySnapshot> {
        self.summary.as_ref()
    }

    /// 更新当前会话摘要。
    pub fn set_summary(&mut self, summary: Option<SummarySnapshot>) {
        self.summary = summary;
    }

    /// 从当前内存状态生成磁盘会话。
    #[must_use]
    pub fn to_stored_session(&self, updated_at: String) -> StoredSession {
        StoredSession::from_session(
            &self.session,
            self.project_key.clone(),
            self.status,
            self.title.clone(),
            updated_at,
            self.summary.clone(),
        )
    }

    /// 从磁盘会话恢复。
    #[must_use]
    pub fn from_stored(stored: StoredSession) -> Self {
        let project_key = stored.project_key.clone();
        let status = stored.status;
        let title = stored.title.clone();
        let summary = stored.summary.clone();
        Self::new(
            stored.into_agent_session(),
            project_key,
            status,
            title,
            summary,
        )
    }
}

/// 追加到 headless JSON 的会话管理信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionManagementInfo {
    /// 项目键。
    pub project_key: String,
    /// 会话状态。
    pub status: SessionStatus,
    /// 会话文件路径。
    pub session_path: String,
    /// 持久化根目录。
    pub storage_root: String,
}
