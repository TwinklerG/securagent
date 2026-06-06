//! headless 模式的事件轨迹记录。

use std::sync::{Arc, Mutex};

use secaudit_agent::Agent;
use serde::Serialize;

/// 工具调用记录。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// 工具名称。
    pub name: String,
    /// 工具参数。
    pub args: String,
    /// 工具结果。
    pub result: String,
}

/// 用户确认事件。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfirmEvent {
    /// 确认提示文案。
    pub prompt: String,
    /// 用户是否批准。
    pub approved: bool,
    /// 确认模式。
    pub mode: String,
    /// 决策来源（如 `auto_allow`、`stdin_prompt`）。
    pub source: String,
}

/// 轨迹快照。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TraceSnapshot {
    /// 工具调用记录。
    pub tool_calls: Vec<ToolCallRecord>,
    /// 状态变迁历史。
    pub state_history: Vec<String>,
    /// 思考事件列表。
    pub think_events: Vec<String>,
    /// 用户确认事件列表。
    pub confirm_events: Vec<ConfirmEvent>,
}

/// 轨迹记录器。
#[derive(Clone, Default)]
pub struct TraceRecorder {
    inner: Arc<Mutex<TraceSnapshot>>,
}

impl TraceRecorder {
    /// 创建记录器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 绑定 Agent 事件回调。
    pub fn attach(&self, agent: &mut Agent) {
        let state_recorder = self.clone();
        agent.on_state_change(move |state| {
            state_recorder.record_state_label(state.label());
        });

        let think_recorder = self.clone();
        agent.on_think(move |text| {
            think_recorder.record_think(text);
        });

        let call_recorder = self.clone();
        agent.on_tool_call(move |name, args| {
            call_recorder.record_tool_call(name, args);
        });

        let result_recorder = self.clone();
        agent.on_tool_result(move |name, result| {
            result_recorder.record_tool_result(name, result);
        });
    }

    /// 记录状态标签。
    pub fn record_state_label(&self, label: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.state_history.push(label.to_owned());
        }
    }

    /// 记录思考事件。
    pub fn record_think(&self, think: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.think_events.push(think.to_owned());
        }
    }

    /// 记录工具调用。
    pub fn record_tool_call(&self, name: &str, args: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.tool_calls.push(ToolCallRecord {
                name: name.to_owned(),
                args: args.to_owned(),
                result: String::new(),
            });
        }
    }

    /// 填充最近一次同名工具调用的执行结果。
    pub fn record_tool_result(&self, name: &str, result: &str) {
        if let Ok(mut trace) = self.inner.lock()
            && let Some(record) = trace
                .tool_calls
                .iter_mut()
                .rev()
                .find(|record| record.name == name)
        {
            result.clone_into(&mut record.result);
        }
    }

    /// 记录用户确认事件。
    pub fn record_confirm(&self, prompt: &str, approved: bool, mode: &str, source: &str) {
        if let Ok(mut trace) = self.inner.lock() {
            trace.confirm_events.push(ConfirmEvent {
                prompt: prompt.to_owned(),
                approved,
                mode: mode.to_owned(),
                source: source.to_owned(),
            });
        }
    }

    /// 导出轨迹快照。
    #[must_use]
    pub fn snapshot(&self) -> TraceSnapshot {
        self.inner
            .lock()
            .map_or_else(|_| TraceSnapshot::default(), |trace| trace.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::TraceRecorder;

    #[test]
    fn recorder_tracks_state_and_tool_trace() {
        let recorder = TraceRecorder::new();

        recorder.record_state_label("执行中");
        recorder.record_think("准备调用工具");
        recorder.record_tool_call("read_file", "{\"path\":\"src/main.rs\"}");
        recorder.record_tool_result("read_file", "ok");
        recorder.record_confirm("允许执行命令吗", true, "ask", "stdin_prompt");

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.state_history, vec!["执行中"]);
        assert_eq!(snapshot.think_events, vec!["准备调用工具"]);
        assert_eq!(snapshot.tool_calls.len(), 1);
        assert_eq!(snapshot.tool_calls[0].name, "read_file");
        assert_eq!(snapshot.tool_calls[0].result, "ok");
        assert_eq!(snapshot.confirm_events.len(), 1);
        assert!(snapshot.confirm_events[0].approved);
    }

    #[test]
    fn recorder_ignores_result_without_call() {
        let recorder = TraceRecorder::new();
        recorder.record_tool_result("missing", "ignored");

        let snapshot = recorder.snapshot();
        assert!(snapshot.tool_calls.is_empty());
    }
}
