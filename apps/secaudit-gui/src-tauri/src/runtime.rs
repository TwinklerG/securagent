use std::env;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use secaudit_agent::Agent;
use secaudit_conversation::{
    ConversationService, ManagedSession, SessionListItem, SessionPreviewRole, SessionStatus,
};
use secaudit_core::Config;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::dto::{
    AgentEvent, AgentWorkbench, ConversationPanel, FindingEvidence, FindingPreview,
    FindingSeverity, FindingStatus, GuiMessage, GuiSessionListItem, ProjectPanel, RunPanel,
    RunPhase, TraceEvent, TraceEventKind,
};
use crate::tools::tool_capabilities;

const EVENT_AGENT: &str = "agent-event";
const FALLBACK_WORK_DIR: &str = ".";
const MSG_CONFIG_HINT: &str = "请先通过 ~/.secaudit/config.json 或 SECAUDIT_API_KEY 配置 Agent 凭据。可选配置：SECAUDIT_API_BASE_URL、SECAUDIT_MODEL、SECAUDIT_MAX_ITERATIONS、SECAUDIT_STRATEGY。";
const ROOT_MARKER_CRATES: &str = "crates";
const ROOT_MARKER_JUSTFILE: &str = "justfile";
const RUN_ACTION_AUDIT: &str = "发送审计请求";
const RUN_ACTION_CONFIG: &str = "检查配置";
const RUN_ACTION_RUNNING: &str = "等待本轮完成";
const RUN_CAN_CANCEL: bool = false;
const RUN_LABEL_BLOCKED: &str = "配置缺失";
const RUN_LABEL_PENDING: &str = "运行中";
const RUN_LABEL_READY: &str = "就绪";
const RUN_PENDING_DETAIL: &str = "Agent 正在处理当前请求，实时事件会同步到运行轨迹。";
const RUN_PENDING_LABEL: &str = "Agent 运行中";
const RUN_STATUS_BLOCKED: &str = "缺少运行 Agent 所需配置，发送暂不可用。";
const RUN_STATUS_READY: &str = "Agent 已连接当前工作区，可以发送代码安全审计请求。";
const SESSION_PREVIEW_FALLBACK: &str = "暂无用户或 Agent 消息";
const SESSION_TRANSIENT_PREVIEW: &str = "尚未保存：发送第一条审计请求后进入历史。";
const SESSION_TRANSIENT_UPDATED_AT: &str = "未保存";
const SESSION_TRANSIENT_TITLE: &str = "当前新会话";
const TRACE_TITLE_ARCHIVE_SESSION_FAILED: &str = "归档会话失败";
const TIME_LABEL_FORMAT: &str = "%H:%M:%S";
const TRACE_DETAIL_MAX_CHARS: usize = 420;
const TRACE_LIMIT: usize = 120;
const TRACE_TITLE_TOOL_CONFIRM: &str = "工具确认请求";
const TRACE_TITLE_WORK_DIR_SWITCH_FAILED: &str = "切换工作区失败";
const FINDING_CONFIDENCE_PENDING: &str = "等待证据";
const FINDING_NEXT_ACTION_PENDING: &str = "发送审计请求，让 Agent 收集工具证据后再确认。";
const FINDING_REMEDIATION_PENDING: &str = "确认 CWE、影响位置和可复现证据后，再生成具体修复建议。";
const FINDING_SUMMARY_PENDING: &str =
    "当前工作台尚未从 Agent 输出中提取到可确认漏洞，只保留待验证的发现槽位。";

pub(crate) type RuntimeState = Mutex<GuiRuntime>;
type SharedTrace = Arc<StdMutex<TraceBuffer>>;

pub(crate) struct GuiRuntime {
    work_dir: PathBuf,
    conversation: ConversationService,
    session: ManagedSession,
    agent: Option<Agent>,
    config_error: Option<String>,
    run_phase: RunPhase,
    trace: SharedTrace,
}

struct TraceBuffer {
    events: Vec<TraceEvent>,
    next_trace_id: u64,
}

impl GuiRuntime {
    pub(crate) fn new(app: &AppHandle) -> Result<Self, String> {
        let work_dir = default_work_dir();
        let conversation = ConversationService::with_default_storage().map_err(to_error_text)?;
        let session = conversation
            .start_session(work_dir.as_path())
            .map_err(to_error_text)?;
        let trace = new_trace_buffer();
        let (agent, config_error) = build_agent(app, work_dir.clone(), &trace);
        let run_phase = idle_run_phase(agent.is_some());
        let mut runtime = Self {
            work_dir,
            conversation,
            session,
            agent,
            config_error,
            run_phase,
            trace,
        };
        runtime.push_trace(
            TraceEventKind::State,
            "工作台就绪",
            "已初始化工作区、会话服务和 Agent 配置状态。",
        );
        Ok(runtime)
    }

    pub(crate) fn snapshot(&self) -> AgentWorkbench {
        let management = self.conversation.management_info(&self.session);
        let active_session_id = self.session.id().to_owned();
        let sessions = self
            .conversation
            .list_sessions_with_preview(self.work_dir.as_path())
            .map(|items| session_list_items(items, &active_session_id))
            .unwrap_or_default();
        let sessions = with_current_session_item(sessions, &self.session);
        let messages = GuiMessage::from_agent_history(self.session.session().messages());

        AgentWorkbench {
            project: ProjectPanel {
                work_dir: display_path(&self.work_dir),
                storage_root: management.storage_root,
                config_ready: self.agent.is_some(),
                config_error: self.config_error.clone(),
            },
            conversation: ConversationPanel {
                active_session_id: self.session.id().to_owned(),
                message_count: self.session.session().messages().len(),
                messages,
                sessions,
            },
            run: self.run_panel(),
            tools: tool_capabilities(self.agent.as_ref(), &self.work_dir),
            trace: trace_snapshot(&self.trace),
            findings: finding_previews(),
        }
    }

    pub(crate) async fn chat(&mut self, message: String) -> Result<AgentWorkbench, String> {
        self.push_trace(TraceEventKind::State, "接收审计请求", &message);

        let Some(agent) = self.agent.as_mut() else {
            let message = self
                .config_error
                .clone()
                .unwrap_or_else(|| MSG_CONFIG_HINT.to_owned());
            self.push_trace(TraceEventKind::Error, "无法启动审计", &message);
            return Err(message);
        };

        self.run_phase = RunPhase::Running;
        let chat_result = self
            .conversation
            .chat(agent, &mut self.session, &message)
            .await;
        self.run_phase = self.current_idle_run_phase();

        match chat_result {
            Ok(_response) => {
                self.push_trace(
                    TraceEventKind::State,
                    "审计轮次完成",
                    "Agent 已返回本轮结果。",
                );
                Ok(self.snapshot())
            }
            Err(error) => Err(self.trace_error("审计轮次失败", error)),
        }
    }

    pub(crate) fn new_session(&mut self) -> Result<AgentWorkbench, String> {
        let session = self.conversation.start_session(self.work_dir.as_path());
        self.session = self.trace_result("创建会话失败", session)?;
        clear_trace(&self.trace);
        self.push_trace(TraceEventKind::State, "新会话", "已创建新的审计会话。");
        Ok(self.snapshot())
    }

    pub(crate) fn switch_session(&mut self, session_id: &str) -> Result<AgentWorkbench, String> {
        let session = self
            .conversation
            .load_session(self.work_dir.as_path(), session_id);
        self.session = self.trace_result("切换会话失败", session)?;
        self.push_trace(TraceEventKind::State, "切换会话", session_id);
        Ok(self.snapshot())
    }

    pub(crate) fn archive_session(&mut self, session_id: &str) -> Result<AgentWorkbench, String> {
        if session_id == self.session.id() {
            let message = "不能归档当前会话，请先切换到其他会话。".to_owned();
            self.push_trace(
                TraceEventKind::Error,
                TRACE_TITLE_ARCHIVE_SESSION_FAILED,
                &message,
            );
            return Err(message);
        }

        let result = self
            .conversation
            .archive_session(self.work_dir.as_path(), session_id);
        self.trace_result(TRACE_TITLE_ARCHIVE_SESSION_FAILED, result)?;
        self.push_trace(TraceEventKind::State, "归档会话", session_id);
        Ok(self.snapshot())
    }

    pub(crate) fn set_work_dir(
        &mut self,
        app: &AppHandle,
        work_dir: &str,
    ) -> Result<AgentWorkbench, String> {
        let work_dir = match resolve_work_dir(&self.work_dir, work_dir) {
            Ok(work_dir) => work_dir,
            Err(message) => {
                self.push_trace(
                    TraceEventKind::Error,
                    TRACE_TITLE_WORK_DIR_SWITCH_FAILED,
                    &message,
                );
                return Err(message);
            }
        };
        let session = self.conversation.start_session(work_dir.as_path());
        let session = self.trace_result(TRACE_TITLE_WORK_DIR_SWITCH_FAILED, session)?;
        let (agent, config_error) = build_agent(app, work_dir.clone(), &self.trace);
        self.work_dir = work_dir;
        self.session = session;
        self.agent = agent;
        self.config_error = config_error;
        self.run_phase = self.current_idle_run_phase();
        clear_trace(&self.trace);
        self.push_trace(
            TraceEventKind::State,
            "工作区已切换",
            &display_path(&self.work_dir),
        );
        Ok(self.snapshot())
    }

    fn push_trace(&mut self, kind: TraceEventKind, title: &str, detail: &str) {
        let _ = push_trace_event(&self.trace, kind, title, detail);
    }

    fn trace_error(&mut self, title: &str, error: impl Display) -> String {
        let message = to_error_text(error);
        self.push_trace(TraceEventKind::Error, title, &message);
        message
    }

    fn run_panel(&self) -> RunPanel {
        match self.run_phase {
            RunPhase::Running => running_run_panel(),
            RunPhase::Ready if self.agent.is_some() => ready_run_panel(),
            RunPhase::Ready | RunPhase::Blocked => {
                let status_detail = self
                    .config_error
                    .clone()
                    .unwrap_or_else(|| RUN_STATUS_BLOCKED.to_owned());
                blocked_run_panel(status_detail)
            }
        }
    }

    fn current_idle_run_phase(&self) -> RunPhase {
        idle_run_phase(self.agent.is_some())
    }

    fn trace_result<T, E: Display>(
        &mut self,
        title: &str,
        result: Result<T, E>,
    ) -> Result<T, String> {
        result.map_err(|error| self.trace_error(title, error))
    }
}

fn idle_run_phase(agent_ready: bool) -> RunPhase {
    if agent_ready {
        RunPhase::Ready
    } else {
        RunPhase::Blocked
    }
}

fn ready_run_panel() -> RunPanel {
    RunPanel {
        phase: RunPhase::Ready,
        label: RUN_LABEL_READY.to_owned(),
        status_detail: RUN_STATUS_READY.to_owned(),
        busy: false,
        can_send: true,
        can_cancel: RUN_CAN_CANCEL,
        primary_action_label: RUN_ACTION_AUDIT.to_owned(),
        pending_label: RUN_PENDING_LABEL.to_owned(),
        pending_detail: RUN_PENDING_DETAIL.to_owned(),
    }
}

fn running_run_panel() -> RunPanel {
    RunPanel {
        phase: RunPhase::Running,
        label: RUN_LABEL_PENDING.to_owned(),
        status_detail: RUN_PENDING_DETAIL.to_owned(),
        busy: true,
        can_send: false,
        can_cancel: RUN_CAN_CANCEL,
        primary_action_label: RUN_ACTION_RUNNING.to_owned(),
        pending_label: RUN_PENDING_LABEL.to_owned(),
        pending_detail: RUN_PENDING_DETAIL.to_owned(),
    }
}

fn blocked_run_panel(status_detail: String) -> RunPanel {
    RunPanel {
        phase: RunPhase::Blocked,
        label: RUN_LABEL_BLOCKED.to_owned(),
        status_detail,
        busy: false,
        can_send: false,
        can_cancel: RUN_CAN_CANCEL,
        primary_action_label: RUN_ACTION_CONFIG.to_owned(),
        pending_label: RUN_LABEL_PENDING.to_owned(),
        pending_detail: RUN_PENDING_DETAIL.to_owned(),
    }
}

impl TraceBuffer {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            next_trace_id: 1,
        }
    }
}

fn build_agent(
    app: &AppHandle,
    work_dir: PathBuf,
    trace: &SharedTrace,
) -> (Option<Agent>, Option<String>) {
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => return (None, Some(format!("{MSG_CONFIG_HINT} 原因：{error}"))),
    };

    let app_for_confirm = app.clone();
    let trace_for_confirm = Arc::clone(trace);
    let confirm = Arc::new(move |prompt: &str| {
        record_and_emit_agent_event(
            &app_for_confirm,
            &trace_for_confirm,
            TraceEventKind::ToolConfirm,
            TRACE_TITLE_TOOL_CONFIRM,
            prompt,
        );
        false
    });

    let mut agent = Agent::new(config, work_dir, confirm);
    bind_agent_events(&mut agent, app, trace);
    (Some(agent), None)
}

fn bind_agent_events(agent: &mut Agent, app: &AppHandle, trace: &SharedTrace) {
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_state_change(move |state| {
            record_and_emit_agent_event(
                &app,
                &trace,
                TraceEventKind::State,
                state.label(),
                "Agent 状态已更新",
            );
        });
    }
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_think(move |text| {
            emit_live_agent_event(&app, &trace, TraceEventKind::Think, "思考", text);
        });
    }
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_token(move |delta| {
            emit_live_agent_event(&app, &trace, TraceEventKind::Token, "流式输出", delta);
        });
    }
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_tool_call(move |name, args| {
            record_and_emit_agent_event(&app, &trace, TraceEventKind::ToolCall, name, args);
        });
    }
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_tool_result(move |name, result| {
            record_and_emit_agent_event(&app, &trace, TraceEventKind::ToolResult, name, result);
        });
    }
}

fn record_and_emit_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    emit_agent_event(app, trace, kind, title, detail, true);
}

fn emit_live_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    emit_agent_event(app, trace, kind, title, detail, false);
}

fn emit_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
    persist: bool,
) {
    let title = title.into();
    let detail = if persist {
        compact_detail(&detail.into())
    } else {
        detail.into()
    };
    let Some(trace) = next_trace_event(trace, kind, &title, &detail, persist) else {
        return;
    };
    let event = AgentEvent { trace };
    let _ = app.emit(EVENT_AGENT, event);
}

fn new_trace_buffer() -> SharedTrace {
    Arc::new(StdMutex::new(TraceBuffer::new()))
}

fn push_trace_event(
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: &str,
    detail: &str,
) -> Option<TraceEvent> {
    next_trace_event(trace, kind, title, &compact_detail(detail), true)
}

fn next_trace_event(
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: &str,
    detail: &str,
    persist: bool,
) -> Option<TraceEvent> {
    let Ok(mut trace) = trace.lock() else {
        return None;
    };

    let event = TraceEvent {
        id: trace.next_trace_id,
        kind,
        title: title.to_owned(),
        detail: detail.to_owned(),
        occurred_at: current_time_label(),
    };
    trace.next_trace_id += 1;
    if persist {
        trace.events.insert(0, event.clone());
        trace.events.truncate(TRACE_LIMIT);
    }
    Some(event)
}

fn trace_snapshot(trace: &SharedTrace) -> Vec<TraceEvent> {
    trace.lock().map_or_else(
        |_| Vec::new(),
        |trace| {
            trace
                .events
                .iter()
                .filter(|event| {
                    !matches!(event.kind, TraceEventKind::Think | TraceEventKind::Token)
                })
                .cloned()
                .collect()
        },
    )
}

fn clear_trace(trace: &SharedTrace) {
    if let Ok(mut trace) = trace.lock() {
        trace.events.clear();
    }
}

fn session_list_items(
    items: Vec<SessionListItem>,
    active_session_id: &str,
) -> Vec<GuiSessionListItem> {
    items
        .into_iter()
        .map(|item| GuiSessionListItem {
            can_archive: item.metadata.status == SessionStatus::Active
                && item.metadata.session_id != active_session_id,
            id: item.metadata.session_id,
            title: item.metadata.title,
            status: item.metadata.status.to_string(),
            updated_at: item.metadata.updated_at,
            message_count: item.metadata.message_count,
            preview: item.preview.map_or_else(
                || SESSION_PREVIEW_FALLBACK.to_owned(),
                |preview| {
                    let role = match preview.role {
                        SessionPreviewRole::User => "用户",
                        SessionPreviewRole::Assistant => "Agent",
                    };
                    format!("{role}: {}", preview.content)
                },
            ),
        })
        .collect()
}

fn with_current_session_item(
    mut sessions: Vec<GuiSessionListItem>,
    session: &ManagedSession,
) -> Vec<GuiSessionListItem> {
    if sessions.iter().any(|item| item.id == session.id()) {
        return sessions;
    }

    sessions.insert(
        0,
        GuiSessionListItem {
            id: session.id().to_owned(),
            title: SESSION_TRANSIENT_TITLE.to_owned(),
            status: SessionStatus::Active.to_string(),
            updated_at: SESSION_TRANSIENT_UPDATED_AT.to_owned(),
            message_count: session.session().messages().len(),
            preview: SESSION_TRANSIENT_PREVIEW.to_owned(),
            can_archive: false,
        },
    );
    sessions
}

fn finding_previews() -> Vec<FindingPreview> {
    let _ = FindingStatus::SUPPORTED;
    let _ = FindingSeverity::SUPPORTED;
    let status = FindingStatus::Candidate;
    let severity = FindingSeverity::Pending;

    vec![FindingPreview {
        id: "pending-agent-output".to_owned(),
        status,
        status_label: status.label().to_owned(),
        severity,
        severity_label: severity.label().to_owned(),
        confidence_label: FINDING_CONFIDENCE_PENDING.to_owned(),
        title: "候选发现等待 Agent 证据".to_owned(),
        location: "发送审计请求后生成".to_owned(),
        taxonomy: None,
        summary: FINDING_SUMMARY_PENDING.to_owned(),
        evidence: vec![
            FindingEvidence {
                label: "证据来源".to_owned(),
                source: "运行轨迹".to_owned(),
                detail: "等待工具调用、文件片段或扫描结果进入轨迹。".to_owned(),
            },
            FindingEvidence {
                label: "归因信息".to_owned(),
                source: "Agent 输出".to_owned(),
                detail: "等待模型给出 CWE、风险原因和影响范围。".to_owned(),
            },
        ],
        remediation: FINDING_REMEDIATION_PENDING.to_owned(),
        next_action: FINDING_NEXT_ACTION_PENDING.to_owned(),
    }]
}

fn default_work_dir() -> PathBuf {
    let current = env::current_dir().unwrap_or_else(|_| PathBuf::from(FALLBACK_WORK_DIR));
    discover_repo_root(&current).unwrap_or_else(|| canonicalize_or_original(current))
}

fn discover_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|path| {
            path.join(ROOT_MARKER_JUSTFILE).is_file() && path.join(ROOT_MARKER_CRATES).is_dir()
        })
        .map(canonicalize_or_original)
}

fn resolve_work_dir(base_dir: &Path, input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("工作区不能为空".to_owned());
    }

    let raw = PathBuf::from(trimmed);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        base_dir.join(raw)
    };
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("工作区不存在或不可访问：{error}"))?;
    if !resolved.is_dir() {
        return Err(format!("工作区不是目录：{}", resolved.display()));
    }

    Ok(resolved)
}

fn canonicalize_or_original(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    normalize_windows_verbatim_path(&raw)
}

fn normalize_windows_verbatim_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    }
}

fn compact_detail(detail: &str) -> String {
    let mut compact = String::new();
    let mut pending_space = false;
    for ch in detail.trim().chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !compact.is_empty() {
            compact.push(' ');
        }
        compact.push(ch);
        pending_space = false;
        if compact.chars().count() >= TRACE_DETAIL_MAX_CHARS {
            compact.push_str("...");
            break;
        }
    }
    compact
}

fn current_time_label() -> String {
    chrono::Local::now().format(TIME_LABEL_FORMAT).to_string()
}

fn to_error_text(error: impl Display) -> String {
    error.to_string()
}
