mod commands;
mod tui;

use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;

use secaudit_agent::state::AgentState;
use secaudit_agent::{ChatMessage as AgentChatMessage, Role};
use secaudit_conversation::{ConversationService, ManagedSession, SessionListItem};
use secaudit_core::Config;

#[derive(Debug, Clone)]
struct ChatRequest {
    input: String,
}

#[derive(Debug, Clone)]
struct SessionSnapshot {
    id: String,
    messages: Vec<DisplayMessage>,
    message_count: usize,
}

impl SessionSnapshot {
    fn from_managed(session: &ManagedSession) -> Self {
        let messages = session
            .session()
            .messages()
            .iter()
            .filter_map(DisplayMessage::from_agent_message)
            .collect::<Vec<_>>();

        Self {
            id: session.id().to_owned(),
            messages,
            message_count: session.session().messages().len(),
        }
    }
}

#[derive(Debug, Clone)]
struct DisplayMessage {
    role: DisplayRole,
    content: String,
}

impl DisplayMessage {
    fn from_agent_message(message: &AgentChatMessage) -> Option<Self> {
        let content = message.content.as_ref()?;
        let role = match &message.role {
            Role::User => DisplayRole::User,
            Role::Assistant => DisplayRole::Agent,
            Role::System | Role::Tool => return None,
        };

        Some(Self {
            role,
            content: content.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayRole {
    User,
    Agent,
}

#[derive(Debug)]
enum WorkerEvent {
    Ready {
        tool_names: Vec<String>,
        /// `(name, description)` 列表，来自 `SkillRegistry`
        skill_list: Vec<(String, String)>,
        session: SessionSnapshot,
    },
    State(AgentState),
    Think(String),
    Delta(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        result: String,
    },
    ChatDone {
        response: Result<String, String>,
        message_count: usize,
    },
    Status {
        message_count: usize,
    },
    NewSession {
        session: SessionSnapshot,
    },
    SessionLoaded {
        session: SessionSnapshot,
    },
    SessionList {
        sessions: Result<Vec<SessionListItem>, String>,
    },
    ConfirmRequest {
        prompt: String,
        response_tx: std_mpsc::Sender<bool>,
    },
    Error(String),
}

#[derive(Debug)]
enum WorkerCommand {
    Chat(ChatRequest),
    NewSession,
    ListSessions,
    SwitchSession { selector: String },
    QueryStatus,
    Shutdown,
}

fn build_worker_conversation() -> secaudit_conversation::Result<ConversationService> {
    ConversationService::with_default_storage()
}

fn start_worker_session(
    conversation: &ConversationService,
    work_dir: &Path,
) -> secaudit_conversation::Result<ManagedSession> {
    conversation.start_session(work_dir)
}

fn resolve_session_selector(
    conversation: &ConversationService,
    work_dir: &Path,
    selector: &str,
) -> Result<String, String> {
    if let Ok(index) = selector.parse::<usize>() {
        if index == 0 {
            return Err("会话序号从 1 开始。".to_owned());
        }
        let sessions = conversation
            .list_sessions(work_dir)
            .map_err(|e| format!("{e}"))?;
        return sessions
            .get(index - 1)
            .map(|session| session.session_id.clone())
            .ok_or_else(|| format!("会话序号不存在：{selector}"));
    }

    Ok(selector.to_owned())
}

fn parse_user_input(input: &str) -> commands::UserInput {
    commands::parse(input)
}

pub async fn run(config: Config, work_dir: PathBuf) {
    tui::run(config, work_dir).await;
}

#[cfg(test)]
mod tests {
    use std::fs;

    use secaudit_agent::{ChatMessage, Role};
    use secaudit_conversation::{
        ConversationConfig, ConversationService, SessionPreviewRole, SessionStatus,
    };
    use tempfile::TempDir;

    use super::{
        DisplayMessage, DisplayRole, SessionSnapshot, resolve_session_selector,
        start_worker_session,
    };

    #[test]
    fn worker_session_uses_conversation_storage() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));

        let mut managed = start_worker_session(&service, &work_dir).expect("start worker session");
        assert!(
            service
                .list_sessions(&work_dir)
                .expect("list sessions")
                .is_empty(),
            "new empty worker sessions should not be persisted"
        );

        managed
            .session_mut()
            .push_message(ChatMessage::user("hello"));
        service.save_session(&managed).expect("save worker session");

        let loaded = service
            .load_session(&work_dir, managed.id())
            .expect("load worker session");
        let sessions = service.list_sessions(&work_dir).expect("list sessions");

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions.first().map(|session| session.session_id.as_str()),
            Some(managed.id())
        );
        assert_eq!(
            sessions.first().map(|session| session.status),
            Some(SessionStatus::Active)
        );
        assert_eq!(loaded.session().messages().len(), 1);
        assert_eq!(
            loaded
                .session()
                .messages()
                .first()
                .and_then(|message| message.content.as_deref()),
            Some("hello")
        );
    }

    #[test]
    fn session_snapshot_keeps_user_and_agent_messages_for_tui() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));
        let mut managed = start_worker_session(&service, &work_dir).expect("start worker session");

        managed
            .session_mut()
            .push_message(ChatMessage::system("sys"));
        managed
            .session_mut()
            .push_message(ChatMessage::user("hello"));
        managed.session_mut().push_message(ChatMessage {
            role: Role::Assistant,
            content: Some("world".to_owned()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        });
        managed
            .session_mut()
            .push_message(ChatMessage::tool_result("call-1", "tool output"));

        let snapshot = SessionSnapshot::from_managed(&managed);

        assert_eq!(snapshot.message_count, 4);
        assert_eq!(snapshot.messages.len(), 2);
        assert!(matches!(
            snapshot.messages.first(),
            Some(DisplayMessage {
                role: DisplayRole::User,
                content,
            }) if content == "hello"
        ));
        assert!(matches!(
            snapshot.messages.get(1),
            Some(DisplayMessage {
                role: DisplayRole::Agent,
                content,
            }) if content == "world"
        ));
    }

    #[test]
    fn session_preview_lists_recent_user_or_agent_message() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));
        let mut managed = start_worker_session(&service, &work_dir).expect("start worker session");

        managed
            .session_mut()
            .push_message(ChatMessage::system("sys"));
        managed
            .session_mut()
            .push_message(ChatMessage::user("hello"));
        managed.session_mut().push_message(ChatMessage {
            role: Role::Assistant,
            content: Some("world\nnext line".to_owned()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        });
        service.save_session(&managed).expect("save session");

        let sessions = service
            .list_sessions_with_preview(&work_dir)
            .expect("list with previews");

        assert_eq!(sessions.len(), 1);
        assert!(matches!(
            sessions.first().and_then(|item| item.preview.as_ref()),
            Some(preview)
                if preview.role == SessionPreviewRole::Assistant
                    && preview.content == "world next line"
        ));
    }

    #[test]
    fn session_selector_supports_one_based_indices() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let service =
            ConversationService::new(ConversationConfig::with_root(temp.path().join("runtime")));
        let mut managed = start_worker_session(&service, &work_dir).expect("start worker session");
        managed
            .session_mut()
            .push_message(ChatMessage::user("hello"));
        service.save_session(&managed).expect("save session");

        let resolved =
            resolve_session_selector(&service, &work_dir, "1").expect("resolve first session");

        assert_eq!(resolved, managed.id());
    }
}
