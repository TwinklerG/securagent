//! `secaudit-conversation` —— 共享会话服务、历史持久化与上下文窗口。

mod context_usage;
mod error;
mod model;
mod service;
mod sliding_window;
mod storage;

pub use context_usage::{
    ContextTokenEstimator, ContextUsage, ContextUsageEstimator, DEFAULT_CONTEXT_WINDOW_TOKENS,
};
pub use error::{Error, Result};
pub use model::{
    ManagedSession, ProjectKey, ProjectMetadata, SessionListItem, SessionManagementInfo,
    SessionMetadata, SessionPreview, SessionPreviewRole, SessionStatus, StoredSession,
    SummarySnapshot,
};
pub use service::{ConversationConfig, ConversationService};
pub use sliding_window::SlidingWindowPolicy;
pub use storage::ConversationLayout;
