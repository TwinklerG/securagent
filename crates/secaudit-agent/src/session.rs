// 会话管理：对话历史与工作目录上下文

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::ChatMessage;

/// 交互式会话，持有对话历史和工作目录上下文。
#[derive(Serialize, Deserialize)]
pub struct Session {
    /// 会话唯一标识
    pub id: String,
    /// 创建时间（ISO 8601 格式）
    pub created_at: String,
    /// 对话历史
    pub messages: Vec<ChatMessage>,
    /// 工作目录（审计根目录）
    pub work_dir: PathBuf,
}

impl Session {
    /// 创建新会话，自动生成 UUID 和时间戳
    #[must_use]
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            messages: Vec::new(),
            work_dir,
        }
    }

    /// 添加消息到对话历史
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// 获取对话历史
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// 清空对话历史
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}
