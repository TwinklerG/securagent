use secaudit_agent::{ChatMessage, Role};

/// 基于消息条数的上下文滑动窗口策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlidingWindowPolicy {
    /// 最多保留的非 system 消息数。
    pub max_context_messages: usize,
}

impl SlidingWindowPolicy {
    /// 创建策略。
    #[must_use]
    pub const fn new(max_context_messages: usize) -> Self {
        Self {
            max_context_messages,
        }
    }

    /// 返回裁剪后的上下文视图，不修改完整历史。
    #[must_use]
    pub fn apply(&self, messages: &[ChatMessage]) -> Vec<ChatMessage> {
        if self.max_context_messages == 0 {
            return messages
                .iter()
                .find(|message| matches!(message.role, Role::System))
                .cloned()
                .into_iter()
                .collect();
        }

        let system = messages
            .iter()
            .find(|message| matches!(message.role, Role::System))
            .cloned();

        let mut non_system: Vec<ChatMessage> = messages
            .iter()
            .filter(|message| !matches!(message.role, Role::System))
            .cloned()
            .collect();

        if non_system.len() > self.max_context_messages {
            let keep_from = non_system.len() - self.max_context_messages;
            non_system = non_system.split_off(keep_from);
        }

        while non_system
            .first()
            .is_some_and(|message| matches!(message.role, Role::Tool))
        {
            non_system.remove(0);
        }

        system.into_iter().chain(non_system).collect()
    }
}

impl Default for SlidingWindowPolicy {
    fn default() -> Self {
        Self::new(24)
    }
}

#[cfg(test)]
mod tests {
    use secaudit_agent::{ChatMessage, Role};

    use super::SlidingWindowPolicy;

    #[test]
    fn keeps_short_history_unchanged() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u1"),
            ChatMessage::user("u2"),
        ];

        let view = SlidingWindowPolicy::new(4).apply(&messages);

        assert_eq!(view.len(), 3);
        assert!(matches!(
            view.first().map(|msg| &msg.role),
            Some(Role::System)
        ));
    }

    #[test]
    fn keeps_system_and_latest_non_system_messages() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u1"),
            ChatMessage::user("u2"),
            ChatMessage::user("u3"),
            ChatMessage::user("u4"),
        ];

        let view = SlidingWindowPolicy::new(2).apply(&messages);

        assert_eq!(view.len(), 3);
        assert_eq!(
            view.first().and_then(|msg| msg.content.as_deref()),
            Some("sys")
        );
        assert_eq!(
            view.get(1).and_then(|msg| msg.content.as_deref()),
            Some("u3")
        );
        assert_eq!(
            view.get(2).and_then(|msg| msg.content.as_deref()),
            Some("u4")
        );
    }

    #[test]
    fn zero_keeps_only_system_message() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u1"),
            ChatMessage::user("u2"),
        ];

        let view = SlidingWindowPolicy::new(0).apply(&messages);

        assert_eq!(view.len(), 1);
        assert!(matches!(
            view.first().map(|msg| &msg.role),
            Some(Role::System)
        ));
    }

    #[test]
    fn drops_leading_tool_result_after_trimming() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u1"),
            ChatMessage::tool_result("call-1", "tool output"),
            ChatMessage::user("u2"),
        ];

        let view = SlidingWindowPolicy::new(2).apply(&messages);

        assert_eq!(view.len(), 2);
        assert!(matches!(
            view.first().map(|msg| &msg.role),
            Some(Role::System)
        ));
        assert_eq!(
            view.get(1).and_then(|msg| msg.content.as_deref()),
            Some("u2")
        );
    }
}
