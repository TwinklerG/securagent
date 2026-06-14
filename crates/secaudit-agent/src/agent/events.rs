//! Agent 事件总线与回调注册。

use super::state::AgentState;
use crate::llm::{TokenUsage, ToolCallResponse};

/// 子 Agent 生命周期事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentEvent {
    /// 子 Agent 已经被自动拉起。
    Started { name: String, reason: String },
    /// 子 Agent 完成并返回摘要。
    Completed { name: String, summary: String },
    /// 子 Agent 失败，主 Agent 会继续处理原始请求。
    Failed { name: String, error: String },
}

/// 状态变更回调
pub type StateCallback = Box<dyn Fn(&AgentState) + Send + Sync>;
/// 思考过程回调
pub type ThinkCallback = Box<dyn Fn(&str) + Send + Sync>;
/// 流式 token 增量回调（每次仅传当前 chunk 的文本片段）
pub type TokenCallback = Box<dyn Fn(&str) + Send + Sync>;
/// 真实 token usage 回调
pub type UsageCallback = Box<dyn Fn(TokenUsage) + Send + Sync>;
/// 工具调用回调（工具名, 参数）
pub type ToolCallCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
/// 工具结果回调（工具名, 结果）
pub type ToolResultCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
/// 子 Agent 事件回调。
pub type SubagentCallback = Box<dyn Fn(&SubagentEvent) + Send + Sync>;

/// 事件总线：管理 Agent 状态与回调，独立于 LLM/工具借用。
#[derive(Default)]
pub(crate) struct EventBus {
    /// 当前运行状态
    state: AgentState,
    /// 状态变更回调
    on_state_change: Option<StateCallback>,
    /// 思考过程回调
    on_think: Option<ThinkCallback>,
    /// 流式 token 增量回调
    on_token: Option<TokenCallback>,
    /// 真实 token usage 回调
    on_usage: Option<UsageCallback>,
    /// 工具调用回调
    on_tool_call: Option<ToolCallCallback>,
    /// 工具结果回调
    on_tool_result: Option<ToolResultCallback>,
    /// 子 Agent 事件回调
    on_subagent: Option<SubagentCallback>,
}

impl EventBus {
    /// 更新状态并触发回调
    pub(crate) fn set_state(&mut self, state: AgentState) {
        self.state = state;
        if let Some(cb) = &self.on_state_change {
            cb(&self.state);
        }
    }

    /// 设置状态变更回调。
    pub(crate) fn on_state_change<F: Fn(&AgentState) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_state_change = Some(Box::new(cb));
    }

    /// 设置思考过程回调。
    pub(crate) fn on_think<F: Fn(&str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_think = Some(Box::new(cb));
    }

    /// 设置流式 token 增量回调。
    pub(crate) fn on_token<F: Fn(&str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_token = Some(Box::new(cb));
    }

    /// 设置真实 token usage 回调。
    pub(crate) fn on_usage<F: Fn(TokenUsage) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_usage = Some(Box::new(cb));
    }

    /// 设置工具调用回调。
    pub(crate) fn on_tool_call<F: Fn(&str, &str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_tool_call = Some(Box::new(cb));
    }

    /// 设置工具结果回调。
    pub(crate) fn on_tool_result<F: Fn(&str, &str) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_tool_result = Some(Box::new(cb));
    }

    /// 设置子 Agent 事件回调。
    pub(crate) fn on_subagent<F: Fn(&SubagentEvent) + Send + Sync + 'static>(&mut self, cb: F) {
        self.on_subagent = Some(Box::new(cb));
    }

    /// 通知思考内容
    pub(crate) fn notify_think(&self, text: &str) {
        if let Some(cb) = &self.on_think {
            cb(text);
        }
    }

    /// 通知流式 token 增量
    pub(crate) fn notify_token(&self, delta: &str) {
        if let Some(cb) = &self.on_token {
            cb(delta);
        }
    }

    /// 通知真实 token usage。
    pub(crate) fn notify_usage(&self, usage: TokenUsage) {
        if let Some(cb) = &self.on_usage {
            cb(usage);
        }
    }

    /// 通知工具调用
    pub(crate) fn notify_tool_call(&self, name: &str, args: &str) {
        if let Some(cb) = &self.on_tool_call {
            cb(name, args);
        }
    }

    /// 批量通知工具调用。
    pub(crate) fn notify_tool_calls(&self, calls: &[ToolCallResponse]) {
        for call in calls {
            self.notify_tool_call(&call.function.name, &call.function.arguments);
        }
    }

    /// 通知工具结果
    pub(crate) fn notify_tool_result(&self, name: &str, result: &str) {
        if let Some(cb) = &self.on_tool_result {
            cb(name, result);
        }
    }

    /// 批量通知工具结果。
    pub(crate) fn notify_tool_results(&self, results: &[(String, String)]) {
        for (name, result) in results {
            self.notify_tool_result(name, result);
        }
    }

    /// 通知子 Agent 事件。
    pub(crate) fn notify_subagent(&self, event: &SubagentEvent) {
        if let Some(cb) = &self.on_subagent {
            cb(event);
        }
    }
}
