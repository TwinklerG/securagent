use secaudit_agent::{ChatMessage, Role, TokenUsage};
use secaudit_conversation::{ContextTokenEstimator, ContextUsage};
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct AgentWorkbench {
    pub(crate) project: ProjectPanel,
    pub(crate) conversation: ConversationPanel,
    pub(crate) run: RunPanel,
    pub(crate) status: StatusPanel,
    pub(crate) tools: Vec<ToolCapability>,
    pub(crate) trace: Vec<TraceEvent>,
    pub(crate) findings: Vec<FindingPreview>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct ProjectPanel {
    pub(crate) work_dir: String,
    pub(crate) storage_root: String,
    pub(crate) config_ready: bool,
    pub(crate) config_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct ConversationPanel {
    pub(crate) active_session_id: String,
    pub(crate) message_count: usize,
    pub(crate) messages: Vec<GuiMessage>,
    pub(crate) sessions: Vec<GuiSessionListItem>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct RunPanel {
    pub(crate) phase: RunPhase,
    pub(crate) label: String,
    pub(crate) status_detail: String,
    pub(crate) busy: bool,
    pub(crate) can_send: bool,
    pub(crate) primary_action_label: String,
    pub(crate) pending_label: String,
    pub(crate) pending_detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum RunPhase {
    Ready,
    Running,
    Blocked,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct StatusPanel {
    pub(crate) agent_label: String,
    pub(crate) model: String,
    pub(crate) session_status: String,
    pub(crate) session_path: String,
    pub(crate) context: GuiContextUsage,
    pub(crate) active_context: GuiContextUsage,
    pub(crate) token_usage: GuiTokenUsage,
    pub(crate) message_count: usize,
    pub(crate) trace_count: usize,
    pub(crate) tool_count: usize,
    pub(crate) finding_count: usize,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct GuiContextUsage {
    pub(crate) window_tokens: u64,
    pub(crate) used_tokens: u64,
    pub(crate) free_tokens: u64,
    pub(crate) system_tokens: u64,
    pub(crate) tool_tokens: u64,
    pub(crate) message_tokens: u64,
    pub(crate) used_percent: u64,
    pub(crate) free_percent: u64,
    pub(crate) system_percent: u64,
    pub(crate) tool_percent: u64,
    pub(crate) message_percent: u64,
    pub(crate) estimator_label: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct GuiTokenUsage {
    pub(crate) prompt: u64,
    pub(crate) completion: u64,
    pub(crate) total: u64,
}

impl GuiContextUsage {
    #[must_use]
    pub(crate) fn from_context_usage(usage: &ContextUsage) -> Self {
        Self {
            window_tokens: usage.window_tokens,
            used_tokens: usage.used_tokens,
            free_tokens: usage.free_tokens,
            system_tokens: usage.system_tokens,
            tool_tokens: usage.tool_tokens,
            message_tokens: usage.message_tokens,
            used_percent: usage.used_percent(),
            free_percent: usage.free_percent(),
            system_percent: usage.system_percent(),
            tool_percent: usage.tool_percent(),
            message_percent: usage.message_percent(),
            estimator_label: context_estimator_label(usage.token_estimator).to_owned(),
        }
    }
}

impl GuiTokenUsage {
    #[must_use]
    pub(crate) fn from_token_usage(usage: TokenUsage) -> Self {
        Self {
            prompt: usage.prompt_tokens,
            completion: usage.completion_tokens,
            total: token_usage_total(usage),
        }
    }
}

fn context_estimator_label(estimator: ContextTokenEstimator) -> &'static str {
    match estimator {
        ContextTokenEstimator::Tiktoken => "tiktoken",
        ContextTokenEstimator::CharacterApproximation => "字符估算",
    }
}

fn token_usage_total(usage: TokenUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens.saturating_add(usage.completion_tokens)
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct ToolCapability {
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) risk: String,
    pub(crate) description: String,
    pub(crate) parameters: Vec<ToolParameter>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct ToolParameter {
    pub(crate) key: ToolParameterKey,
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) type_name: String,
    pub(crate) required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum ToolParameterKey {
    Command,
    Content,
    ContextLines,
    CweId,
    GlobFilter,
    Limit,
    MaxDepth,
    MaxResults,
    Offset,
    Path,
    Pattern,
    ProjectPath,
    Query,
    Recursive,
    Ruleset,
    TimeoutSecs,
    Other,
}

impl ToolParameterKey {
    pub(crate) const fn for_name(name: &str) -> Self {
        match name.as_bytes() {
            b"command" => Self::Command,
            b"content" => Self::Content,
            b"context_lines" => Self::ContextLines,
            b"cwe_id" => Self::CweId,
            b"glob_filter" => Self::GlobFilter,
            b"limit" => Self::Limit,
            b"max_depth" => Self::MaxDepth,
            b"max_results" => Self::MaxResults,
            b"offset" => Self::Offset,
            b"path" => Self::Path,
            b"pattern" => Self::Pattern,
            b"project_path" => Self::ProjectPath,
            b"query" => Self::Query,
            b"recursive" => Self::Recursive,
            b"ruleset" => Self::Ruleset,
            b"timeout_secs" => Self::TimeoutSecs,
            _ => Self::Other,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Command => "命令",
            Self::Content => "内容",
            Self::ContextLines => "上下文行",
            Self::CweId => "CWE",
            Self::GlobFilter => "文件过滤",
            Self::Limit => "行数",
            Self::MaxDepth => "最大深度",
            Self::MaxResults => "结果上限",
            Self::Offset => "起始行",
            Self::Path => "路径",
            Self::Pattern => "模式",
            Self::ProjectPath => "项目路径",
            Self::Query => "查询",
            Self::Recursive => "递归",
            Self::Ruleset => "规则集",
            Self::TimeoutSecs => "超时秒数",
            Self::Other => "参数",
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct FindingPreview {
    pub(crate) id: String,
    pub(crate) status: FindingStatus,
    pub(crate) status_label: String,
    pub(crate) severity: FindingSeverity,
    pub(crate) severity_label: String,
    pub(crate) confidence_label: String,
    pub(crate) title: String,
    pub(crate) location: String,
    pub(crate) taxonomy: Option<String>,
    pub(crate) summary: String,
    pub(crate) evidence: Vec<FindingEvidence>,
    pub(crate) remediation: String,
    pub(crate) next_action: String,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum FindingStatus {
    Candidate,
    Confirmed,
    Dismissed,
}

impl FindingStatus {
    pub(crate) const SUPPORTED: [Self; 3] = [Self::Candidate, Self::Confirmed, Self::Dismissed];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Candidate => "候选",
            Self::Confirmed => "已确认",
            Self::Dismissed => "已排除",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum FindingSeverity {
    Pending,
    Low,
    Medium,
    High,
    Critical,
}

impl FindingSeverity {
    pub(crate) const SUPPORTED: [Self; 5] = [
        Self::Pending,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Critical,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Pending => "待确认",
            Self::Low => "低危",
            Self::Medium => "中危",
            Self::High => "高危",
            Self::Critical => "严重",
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct FindingEvidence {
    pub(crate) label: String,
    pub(crate) source: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct GuiSessionListItem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) updated_at: String,
    pub(crate) message_count: usize,
    pub(crate) preview: String,
    pub(crate) can_archive: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct GuiMessage {
    pub(crate) role: GuiRole,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum GuiRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct TraceEvent {
    pub(crate) id: u64,
    pub(crate) kind: TraceEventKind,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) occurred_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub(crate) enum TraceEventKind {
    State,
    Think,
    Token,
    ToolCall,
    ToolConfirm,
    ToolResult,
    Error,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct AgentEvent {
    pub(crate) trace: TraceEvent,
    pub(crate) approval_request: Option<CommandApprovalRequest>,
    pub(crate) approval_resolution: Option<CommandApprovalResolution>,
    pub(crate) token_usage: Option<GuiTokenUsage>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct CommandApprovalRequest {
    pub(crate) id: u64,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct CommandApprovalResolution {
    pub(crate) id: u64,
    pub(crate) approved: bool,
    pub(crate) status_label: String,
}

impl GuiMessage {
    pub(crate) fn from_agent_history(messages: &[ChatMessage]) -> Vec<Self> {
        let mut display_messages = Vec::new();
        let mut pending_assistant: Option<String> = None;

        for message in messages {
            match message.role {
                Role::User => {
                    push_pending_assistant(&mut display_messages, &mut pending_assistant);
                    if let Some(content) = non_empty_content(message) {
                        display_messages.push(Self {
                            role: GuiRole::User,
                            content,
                        });
                    }
                }
                Role::Assistant if message.tool_calls.as_ref().is_none_or(Vec::is_empty) => {
                    if let Some(content) = non_empty_content(message) {
                        pending_assistant = Some(content);
                    }
                }
                Role::System | Role::Tool | Role::Assistant => {}
            }
        }

        push_pending_assistant(&mut display_messages, &mut pending_assistant);
        display_messages
    }
}

fn push_pending_assistant(messages: &mut Vec<GuiMessage>, pending: &mut Option<String>) {
    let Some(content) = pending.take() else {
        return;
    };
    messages.push(GuiMessage {
        role: GuiRole::Assistant,
        content,
    });
}

fn non_empty_content(message: &ChatMessage) -> Option<String> {
    let content = message.content.as_ref()?.trim();
    if content.is_empty() {
        None
    } else {
        Some(content.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use secaudit_agent::{ChatMessage, Role};

    use super::{GuiMessage, GuiRole};

    #[test]
    fn agent_history_keeps_one_assistant_reply_per_user_turn() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("审计项目"),
            assistant_with_tool("我先查看项目结构"),
            ChatMessage::tool_result("call-1", "tool output"),
            assistant_text("## 最终报告"),
            ChatMessage::user("继续"),
            assistant_with_tool("我再检查依赖"),
            assistant_text("第二轮结论"),
        ];

        let display = GuiMessage::from_agent_history(&messages);

        assert_eq!(display.len(), 4);
        assert_eq!(
            display.first().map(|message| message.role),
            Some(GuiRole::User)
        );
        assert_eq!(
            display.get(1).map(|message| message.content.as_str()),
            Some("## 最终报告")
        );
        assert_eq!(
            display.get(3).map(|message| message.content.as_str()),
            Some("第二轮结论")
        );
    }

    fn assistant_text(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: Some(content.to_owned()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        }
    }

    fn assistant_with_tool(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: Some(content.to_owned()),
            tool_calls: Some(Vec::new()),
            tool_call_id: None,
            usage: None,
        }
    }
}
