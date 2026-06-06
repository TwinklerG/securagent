//! `secaudit-agent` —— 安全审计推理引擎（Agent、会话、轨迹与 Prompt 模板）。

pub mod agent;
pub mod llm;
pub mod prompt;
pub mod session;
pub mod trajectory;

pub mod config {
    pub use secaudit_core::Config;
}

pub mod error {
    pub use secaudit_core::Error;
}

pub mod tools {
    pub use secaudit_tools::{ConfirmFn, Tool, audit_tools, default_tools};
}

pub use agent::state;
pub use agent::strategy;
pub use agent::{Agent, AuditReport, Finding};
pub use llm::{
    ChatMessage, FunctionCall, HttpLlmClient, Role, TokenUsage, ToolCallResponse, ToolDefinition,
};
pub use session::Session;
pub use trajectory::to_multi_turn_sample;
