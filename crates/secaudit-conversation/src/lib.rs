//! `secaudit-conversation` —— 共享会话服务、历史持久化与上下文窗口。

mod error;
mod model;
mod service;
mod sliding_window;
mod storage;

pub use error::{Error, Result};
pub use model::{
    ManagedSession, ProjectKey, ProjectMetadata, SessionListItem, SessionManagementInfo,
    SessionMetadata, SessionPreview, SessionPreviewRole, SessionStatus, StoredSession,
    SummarySnapshot,
};
pub use service::{ConversationConfig, ConversationService};
pub use sliding_window::SlidingWindowPolicy;
pub use storage::StorageLayout;
