use std::env;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError, Sender as StdSender};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use secaudit_agent::llm;
use secaudit_agent::{Agent, ChatMessage, HttpLlmClient, Role, SubagentEvent, TokenUsage};
use secaudit_conversation::{
    ContextCompressionEvent, ConversationConfig, ConversationService, ManagedSession,
    SessionListItem, SessionManagementInfo, SessionPreviewRole, SessionStatus,
};
use secaudit_core::Config;
use secaudit_memory::FileMemoryStore;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::dto::{
    AgentEvent, AgentWorkbench, CommandApprovalRequest, CommandApprovalResolution,
    ConversationPanel, FindingEvidence, FindingPreview, FindingSeverity, FindingStatus,
    GuiContextUsage, GuiMessage, GuiSessionListItem, GuiTokenUsage, ProjectPanel, RunPanel,
    RunPhase, StatusPanel, TraceEvent, TraceEventKind,
};
use crate::tools::tool_capabilities;

const EVENT_AGENT: &str = "agent-event";
const FALLBACK_WORK_DIR: &str = ".";
const MSG_CONFIG_HINT: &str = "请先通过 ~/.secaudit/config.json 或 SECAUDIT_API_KEY 配置 Agent 凭据。可选配置：SECAUDIT_API_BASE_URL、SECAUDIT_MODEL、SECAUDIT_MAX_ITERATIONS、SECAUDIT_STRATEGY。";
const STATUS_AGENT_BLOCKED: &str = "配置缺失";
const STATUS_AGENT_READY: &str = "就绪";
const STATUS_AGENT_RUNNING: &str = "运行中";
const STATUS_MODEL_FALLBACK: &str = "未配置";
const ROOT_MARKER_CRATES: &str = "crates";
const ROOT_MARKER_JUSTFILE: &str = "justfile";
const RUN_ACTION_AUDIT: &str = "发送审计请求";
const RUN_ACTION_CONFIG: &str = "检查配置";
const RUN_ACTION_RUNNING: &str = "等待本轮完成";
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
const COMMAND_APPROVAL_TIMEOUT_SECS: u64 = 300;
const COMMAND_APPROVAL_WAITING: &str = "等待用户确认。";
const TRACE_TITLE_TOOL_CONFIRM: &str = "工具确认请求";
const TRACE_TITLE_TOOL_CONFIRM_RESULT: &str = "工具确认结果";
const TRACE_TITLE_TOOL_CONFIRM_TIMEOUT: &str = "工具确认超时";
const TRACE_TITLE_WORK_DIR_SWITCH_FAILED: &str = "切换工作区失败";
const FINDING_CONFIDENCE_PENDING: &str = "等待证据";
const FINDING_CONFIDENCE_CANDIDATE: &str = "模型候选";
const FINDING_DEFAULT_LOCATION: &str = "Agent 输出中待定位";
const FINDING_DEFAULT_TAXONOMY: &str = "待归因";
const FINDING_NEXT_ACTION: &str = "复核证据、定位代码片段，再让 Agent 生成最小修复建议。";
const FINDING_REMEDIATION: &str = "确认 CWE、影响位置和可复现证据后，再生成具体修复建议。";
const FINDING_TITLE_MAX_CHARS: usize = 72;
const FINDING_EVIDENCE_MAX_CHARS: usize = 180;
const FINDING_MAX_ITEMS: usize = 5;

const FINDING_KEYWORDS: &[&str] = &[
    "cwe-",
    "漏洞",
    "风险",
    "注入",
    "越权",
    "命令执行",
    "路径穿越",
    "ssrf",
    "xss",
    "反序列化",
    "凭据",
    "敏感信息",
];
const LOCATION_MARKERS: &[&str] = &[
    ".rs", ".ts", ".tsx", ".js", ".jsx", ".vue", ".py", ".go", ".java", ".kt", ".php", ".rb",
    ".cs", ".c", ".cpp", ".h", ".toml", ".json", ".yaml", ".yml", ".env", "/", "\\",
];

pub(crate) type RuntimeState = Mutex<GuiRuntime>;
type SharedTrace = Arc<StdMutex<TraceBuffer>>;

pub(crate) struct GuiRuntime {
    work_dir: PathBuf,
    conversation: ConversationService,
    session: ManagedSession,
    agent: Option<Agent>,
    memory: Option<FileMemoryStore>,
    llm_client: Option<HttpLlmClient>,
    config_error: Option<String>,
    model: Option<String>,
    run_phase: RunPhase,
    trace: SharedTrace,
    approvals: CommandApprovalBroker,
}

#[derive(Debug, Clone, Copy)]
struct StatusCounts {
    trace: usize,
    tool: usize,
    finding: usize,
}

struct TraceBuffer {
    events: Vec<TraceEvent>,
    next_trace_id: u64,
}

#[derive(Clone)]
pub(crate) struct CommandApprovalBroker {
    inner: Arc<StdMutex<CommandApprovalState>>,
}

struct CommandApprovalState {
    next_approval_id: u64,
    pending: Option<PendingCommandApproval>,
    trace: Option<SharedTrace>,
}

struct PendingCommandApproval {
    id: u64,
    prompt: String,
    response_tx: StdSender<bool>,
}

#[derive(Default)]
struct AgentEventApproval {
    request: Option<CommandApprovalRequest>,
    resolution: Option<CommandApprovalResolution>,
}

impl CommandApprovalBroker {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(StdMutex::new(CommandApprovalState {
                next_approval_id: 1,
                pending: None,
                trace: None,
            })),
        }
    }

    fn attach_trace(&self, trace: SharedTrace) {
        if let Ok(mut state) = self.inner.lock() {
            state.trace = Some(trace);
        }
    }

    fn request(&self, app: &AppHandle, trace: &SharedTrace, prompt: &str) -> bool {
        let (response_tx, response_rx) = std_mpsc::channel();
        let Some(id) = self.register_pending(prompt.to_owned(), response_tx) else {
            record_and_emit_agent_event(
                app,
                trace,
                TraceEventKind::Error,
                TRACE_TITLE_TOOL_CONFIRM,
                "命令批准状态不可用，已按拒绝处理。",
            );
            return false;
        };

        let request = CommandApprovalRequest {
            id,
            prompt: prompt.to_owned(),
        };
        record_and_emit_agent_event_with_approval(
            app,
            trace,
            TraceEventKind::ToolConfirm,
            TRACE_TITLE_TOOL_CONFIRM,
            format!("{prompt}\n\n{COMMAND_APPROVAL_WAITING}"),
            AgentEventApproval {
                request: Some(request),
                resolution: None,
            },
        );

        match response_rx.recv_timeout(Duration::from_secs(COMMAND_APPROVAL_TIMEOUT_SECS)) {
            Ok(approved) => approved,
            Err(RecvTimeoutError::Timeout) => {
                if self.expire_pending(id) {
                    record_and_emit_agent_event_with_approval(
                        app,
                        trace,
                        TraceEventKind::ToolConfirm,
                        TRACE_TITLE_TOOL_CONFIRM_TIMEOUT,
                        format!("{prompt}\n\n批准请求超时，已按拒绝处理。"),
                        AgentEventApproval {
                            request: None,
                            resolution: Some(CommandApprovalResolution {
                                id,
                                approved: false,
                                status_label: "已超时拒绝".to_owned(),
                            }),
                        },
                    );
                }
                false
            }
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    pub(crate) fn resolve(&self, app: &AppHandle, id: u64, approved: bool) -> Result<(), String> {
        let pending = self.take_pending(id)?;
        let status_label = if approved { "已允许" } else { "已拒绝" }.to_owned();
        let detail = format!("{}\n\n用户选择：{status_label}。", pending.prompt);
        pending
            .response_tx
            .send(approved)
            .map_err(|send_error| format!("Agent 已不再等待该批准请求：{send_error}"))?;
        self.emit_resolution(
            app,
            pending.id,
            approved,
            status_label,
            TRACE_TITLE_TOOL_CONFIRM_RESULT,
            detail,
        );
        Ok(())
    }

    fn register_pending(&self, prompt: String, response_tx: StdSender<bool>) -> Option<u64> {
        let Ok(mut state) = self.inner.lock() else {
            return None;
        };
        if let Some(previous) = state.pending.take() {
            let _ = previous.response_tx.send(false);
        }
        let id = state.next_approval_id;
        state.next_approval_id = state.next_approval_id.checked_add(1).unwrap_or(1);
        state.pending = Some(PendingCommandApproval {
            id,
            prompt,
            response_tx,
        });
        Some(id)
    }

    fn take_pending(&self, id: u64) -> Result<PendingCommandApproval, String> {
        let Ok(mut state) = self.inner.lock() else {
            return Err("命令批准状态不可用。".to_owned());
        };

        match state.pending.take() {
            Some(pending) if pending.id == id => Ok(pending),
            Some(pending) => {
                state.pending = Some(pending);
                Err(format!("命令批准请求 {id} 已失效。"))
            }
            None => Err("当前没有待处理的命令批准请求。".to_owned()),
        }
    }

    fn expire_pending(&self, id: u64) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            state.pending = None;
            true
        } else {
            false
        }
    }

    fn emit_resolution(
        &self,
        app: &AppHandle,
        id: u64,
        approved: bool,
        status_label: String,
        title: &str,
        detail: String,
    ) {
        let trace = self.inner.lock().ok().and_then(|state| state.trace.clone());
        let Some(trace) = trace else {
            return;
        };
        record_and_emit_agent_event_with_approval(
            app,
            &trace,
            TraceEventKind::ToolConfirm,
            title,
            detail,
            AgentEventApproval {
                request: None,
                resolution: Some(CommandApprovalResolution {
                    id,
                    approved,
                    status_label,
                }),
            },
        );
    }
}

impl GuiRuntime {
    pub(crate) fn new(app: &AppHandle, approvals: CommandApprovalBroker) -> Result<Self, String> {
        let work_dir = default_work_dir();
        let config = load_agent_config();
        let conversation = build_conversation(config.as_ref().ok())?;
        let session = conversation
            .start_session(work_dir.as_path())
            .map_err(to_error_text)?;
        let trace = new_trace_buffer();
        approvals.attach_trace(Arc::clone(&trace));
        let model = config.as_ref().ok().map(|config| config.model.clone());
        let (agent, llm_client, config_error) =
            build_agent(app, work_dir.clone(), &trace, &approvals, config);
        let memory = conversation
            .create_memory_store(work_dir.as_path())
            .ok()
            .filter(|_| agent.is_some());
        let run_phase = idle_run_phase(agent.is_some());
        let mut runtime = Self {
            work_dir,
            conversation,
            session,
            agent,
            memory,
            llm_client,
            config_error,
            model,
            run_phase,
            trace,
            approvals,
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
        let tools = tool_capabilities(self.agent.as_ref(), &self.work_dir);
        let trace = trace_snapshot(&self.trace);
        let findings = finding_previews(self.session.session().messages());
        let status = self.status_panel(
            &management,
            StatusCounts {
                trace: trace.len(),
                tool: tools.len(),
                finding: findings.len(),
            },
        );

        AgentWorkbench {
            project: ProjectPanel {
                work_dir: display_path(&self.work_dir),
                storage_root: management.storage_root.clone(),
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
            status,
            tools,
            trace,
            findings,
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
            .chat(
                agent,
                &mut self.session,
                &message,
                self.memory.as_ref(),
                self.llm_client.as_ref(),
            )
            .await;
        self.run_phase = self.current_idle_run_phase();

        match chat_result {
            Ok(outcome) => {
                if let Some(compression) = outcome.compression {
                    self.push_trace(
                        TraceEventKind::ContextCompaction,
                        "上下文压缩",
                        &context_compression_detail(&compression),
                    );
                }
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
        if let Some(mem) = self.memory.as_ref() {
            let _ = ConversationService::finalize_session(mem, self.session.id())
                .inspect_err(|e| tracing::warn!("[memory] 会话总结失败: {e}"));
        }
        let session = self.conversation.start_session(self.work_dir.as_path());
        self.session = self.trace_result("创建会话失败", session)?;
        clear_trace(&self.trace);
        self.push_trace(TraceEventKind::State, "新会话", "已创建新的审计会话。");
        Ok(self.snapshot())
    }

    pub(crate) fn switch_session(&mut self, session_id: &str) -> Result<AgentWorkbench, String> {
        if let Some(mem) = self.memory.as_ref() {
            let _ = ConversationService::finalize_session(mem, self.session.id())
                .inspect_err(|e| tracing::warn!("[memory] 会话总结失败: {e}"));
        }
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
        if let Some(mem) = self.memory.as_ref() {
            let _ = ConversationService::finalize_session(mem, self.session.id())
                .inspect_err(|e| tracing::warn!("[memory] 会话总结失败: {e}"));
        }
        let config = load_agent_config();
        let conversation = self.trace_result(
            TRACE_TITLE_WORK_DIR_SWITCH_FAILED,
            build_conversation(config.as_ref().ok()),
        )?;
        let session = conversation.start_session(work_dir.as_path());
        let session = self.trace_result(TRACE_TITLE_WORK_DIR_SWITCH_FAILED, session)?;
        let model = config.as_ref().ok().map(|config| config.model.clone());
        let (agent, llm_client, config_error) =
            build_agent(app, work_dir.clone(), &self.trace, &self.approvals, config);
        let memory = conversation
            .create_memory_store(work_dir.as_path())
            .ok()
            .filter(|_| agent.is_some());
        self.work_dir = work_dir;
        self.conversation = conversation;
        self.session = session;
        self.agent = agent;
        self.memory = memory;
        self.llm_client = llm_client;
        self.config_error = config_error;
        self.model = model;
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

    fn status_panel(
        &self,
        management: &SessionManagementInfo,
        counts: StatusCounts,
    ) -> StatusPanel {
        let context = self.conversation.context_usage(&self.session);
        let active_context = self.conversation.active_context_usage(&self.session);

        StatusPanel {
            agent_label: self.agent_status_label().to_owned(),
            model: self
                .model
                .clone()
                .unwrap_or_else(|| STATUS_MODEL_FALLBACK.to_owned()),
            session_status: management.status.to_string(),
            session_path: management.session_path.clone(),
            token_usage: GuiTokenUsage::from_token_usage(context.cumulative_usage),
            context: GuiContextUsage::from_context_usage(&context),
            active_context: GuiContextUsage::from_context_usage(&active_context),
            message_count: self.session.session().messages().len(),
            trace_count: counts.trace,
            tool_count: counts.tool,
            finding_count: counts.finding,
        }
    }

    fn agent_status_label(&self) -> &'static str {
        match self.run_phase {
            RunPhase::Running => STATUS_AGENT_RUNNING,
            RunPhase::Ready if self.agent.is_some() => STATUS_AGENT_READY,
            RunPhase::Ready | RunPhase::Blocked => STATUS_AGENT_BLOCKED,
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
        primary_action_label: RUN_ACTION_AUDIT.to_owned(),
        pending_label: RUN_PENDING_LABEL.to_owned(),
        pending_detail: RUN_PENDING_DETAIL.to_owned(),
    }
}

fn context_compression_detail(event: &ContextCompressionEvent) -> String {
    format!(
        "已压缩较早的 {} 条消息，context: {} / {} tokens ({}%) -> {} / {} tokens ({}%)。",
        event.covered_message_count,
        event.before_used_tokens,
        event.window_tokens,
        event.before_used_percent,
        event.after_used_tokens,
        event.window_tokens,
        event.after_used_percent,
    )
}

fn running_run_panel() -> RunPanel {
    RunPanel {
        phase: RunPhase::Running,
        label: RUN_LABEL_PENDING.to_owned(),
        status_detail: RUN_PENDING_DETAIL.to_owned(),
        busy: true,
        can_send: false,
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
    approvals: &CommandApprovalBroker,
    config: Result<Config, String>,
) -> (Option<Agent>, Option<HttpLlmClient>, Option<String>) {
    let config = match config {
        Ok(config) => config,
        Err(error) => return (None, None, Some(format!("{MSG_CONFIG_HINT} 原因：{error}"))),
    };

    let llm_client = llm::create_client(&config);

    let app_for_confirm = app.clone();
    let trace_for_confirm = Arc::clone(trace);
    let approvals_for_confirm = approvals.clone();
    let confirm = Arc::new(move |prompt: &str| {
        approvals_for_confirm.request(&app_for_confirm, &trace_for_confirm, prompt)
    });

    let mut agent = Agent::new(config, work_dir, confirm);
    bind_agent_events(&mut agent, app, trace);
    (Some(agent), Some(llm_client), None)
}

fn load_agent_config() -> Result<Config, String> {
    Config::load().map_err(to_error_text)
}

fn build_conversation(config: Option<&Config>) -> Result<ConversationService, String> {
    let conversation_config = match config {
        Some(config) => ConversationConfig::default_storage()
            .map_err(to_error_text)?
            .with_context_model(config.context_window_tokens, config.model.clone()),
        None => ConversationConfig::default_storage().map_err(to_error_text)?,
    };
    Ok(ConversationService::new(conversation_config))
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
        agent.on_usage(move |usage| {
            emit_usage_agent_event(&app, &trace, usage);
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
    {
        let app = app.clone();
        let trace = Arc::clone(trace);
        agent.on_subagent(move |event| {
            let (title, detail) = subagent_trace_text(event);
            record_and_emit_agent_event(&app, &trace, TraceEventKind::Subagent, title, detail);
        });
    }
}

fn subagent_trace_text(event: &SubagentEvent) -> (String, String) {
    match event {
        SubagentEvent::Started { name, reason } => {
            (format!("子 Agent {name} 已拉起"), reason.clone())
        }
        SubagentEvent::Completed { name, summary } => {
            (format!("子 Agent {name} 已完成"), summary.clone())
        }
        SubagentEvent::Failed { name, error } => (format!("子 Agent {name} 失败"), error.clone()),
    }
}

fn record_and_emit_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    record_and_emit_agent_event_with_approval(
        app,
        trace,
        kind,
        title,
        detail,
        AgentEventApproval::default(),
    );
}

fn record_and_emit_agent_event_with_approval(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
    approval: AgentEventApproval,
) {
    emit_agent_event(app, trace, kind, title, detail, true, approval);
}

fn emit_live_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    emit_agent_event(
        app,
        trace,
        kind,
        title,
        detail,
        false,
        AgentEventApproval::default(),
    );
}

fn emit_usage_agent_event(app: &AppHandle, trace: &SharedTrace, usage: TokenUsage) {
    let Some(trace) = next_trace_event(trace, TraceEventKind::Token, "Token 用量", "", false)
    else {
        return;
    };
    let event = AgentEvent {
        trace,
        approval_request: None,
        approval_resolution: None,
        token_usage: Some(GuiTokenUsage::from_token_usage(usage)),
    };
    let _ = app.emit(EVENT_AGENT, event);
}

fn emit_agent_event(
    app: &AppHandle,
    trace: &SharedTrace,
    kind: TraceEventKind,
    title: impl Into<String>,
    detail: impl Into<String>,
    persist: bool,
    approval: AgentEventApproval,
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
    let event = AgentEvent {
        trace,
        approval_request: approval.request,
        approval_resolution: approval.resolution,
        token_usage: None,
    };
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

fn finding_previews(messages: &[ChatMessage]) -> Vec<FindingPreview> {
    let _ = FindingStatus::SUPPORTED;
    let _ = FindingSeverity::SUPPORTED;

    let mut findings = Vec::new();
    for (message_index, message) in messages.iter().enumerate().rev() {
        if findings.len() >= FINDING_MAX_ITEMS || !matches!(message.role, Role::Assistant) {
            continue;
        }
        let Some(content) = message.content.as_deref() else {
            continue;
        };
        collect_findings_from_assistant(content, message_index, &mut findings);
    }
    findings.truncate(FINDING_MAX_ITEMS);
    findings
}

fn collect_findings_from_assistant(
    content: &str,
    message_index: usize,
    findings: &mut Vec<FindingPreview>,
) {
    for (line_index, line) in content.lines().enumerate() {
        if findings.len() >= FINDING_MAX_ITEMS {
            return;
        }
        let candidate = normalize_finding_line(line);
        if !looks_like_finding(&candidate) {
            continue;
        }
        findings.push(finding_from_line(&candidate, message_index, line_index));
    }
}

fn finding_from_line(line: &str, message_index: usize, line_index: usize) -> FindingPreview {
    let status = FindingStatus::Candidate;
    let severity = infer_severity(line);
    let taxonomy = find_cwe(line).or_else(|| infer_taxonomy(line));
    let location = infer_location(line).unwrap_or_else(|| FINDING_DEFAULT_LOCATION.to_owned());
    let taxonomy_label = taxonomy
        .clone()
        .unwrap_or_else(|| FINDING_DEFAULT_TAXONOMY.to_owned());
    let confidence_label = if severity == FindingSeverity::Pending {
        FINDING_CONFIDENCE_PENDING
    } else {
        FINDING_CONFIDENCE_CANDIDATE
    };

    FindingPreview {
        id: format!("assistant-{message_index}-{line_index}"),
        status,
        status_label: status.label().to_owned(),
        severity,
        severity_label: severity.label().to_owned(),
        confidence_label: confidence_label.to_owned(),
        title: truncate_chars(line, FINDING_TITLE_MAX_CHARS),
        location,
        taxonomy,
        summary: format!("Agent 输出命中 {taxonomy_label} 相关候选风险，需要继续核验证据。"),
        evidence: vec![FindingEvidence {
            label: "Agent 输出".to_owned(),
            source: "助手回复".to_owned(),
            detail: truncate_chars(line, FINDING_EVIDENCE_MAX_CHARS),
        }],
        remediation: FINDING_REMEDIATION.to_owned(),
        next_action: FINDING_NEXT_ACTION.to_owned(),
    }
}

fn normalize_finding_line(line: &str) -> String {
    let line = line
        .trim()
        .trim_start_matches(['#', '-', '*', '>', ' ', '\t']);
    line.trim().to_owned()
}

fn looks_like_finding(line: &str) -> bool {
    if line.len() < 8 {
        return false;
    }
    let lower = line.to_lowercase();
    FINDING_KEYWORDS
        .iter()
        .any(|keyword| lower.contains(keyword))
}

fn infer_severity(line: &str) -> FindingSeverity {
    let lower = line.to_lowercase();
    if contains_any(&lower, &["critical", "严重", "致命"]) {
        FindingSeverity::Critical
    } else if contains_any(&lower, &["high", "高危", "高风险"]) {
        FindingSeverity::High
    } else if contains_any(&lower, &["medium", "中危", "中风险"]) {
        FindingSeverity::Medium
    } else if contains_any(&lower, &["low", "低危", "低风险"]) {
        FindingSeverity::Low
    } else {
        FindingSeverity::Pending
    }
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| text.contains(keyword))
}

fn find_cwe(line: &str) -> Option<String> {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .find_map(|token| {
            let upper = token.to_ascii_uppercase();
            upper.starts_with("CWE-").then_some(upper)
        })
}

fn infer_taxonomy(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    if contains_any(&lower, &["命令执行", "command injection"]) {
        Some("命令执行".to_owned())
    } else if contains_any(&lower, &["路径穿越", "path traversal"]) {
        Some("路径穿越".to_owned())
    } else if contains_any(&lower, &["ssrf"]) {
        Some("SSRF".to_owned())
    } else if contains_any(&lower, &["xss", "跨站脚本"]) {
        Some("XSS".to_owned())
    } else if contains_any(&lower, &["越权", "idor", "访问控制"]) {
        Some("访问控制".to_owned())
    } else if contains_any(&lower, &["凭据", "敏感信息", "secret"]) {
        Some("敏感信息泄露".to_owned())
    } else {
        None
    }
}

fn infer_location(line: &str) -> Option<String> {
    let mut code_segment = false;
    for segment in line.split('`') {
        if code_segment && looks_like_location(segment) {
            return Some(segment.trim().to_owned());
        }
        code_segment = !code_segment;
    }
    None
}

fn looks_like_location(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && LOCATION_MARKERS
            .iter()
            .any(|marker| trimmed.contains(marker))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
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

#[cfg(test)]
mod tests {
    use secaudit_agent::{ChatMessage, Role};
    use std::sync::mpsc;

    use super::{CommandApprovalBroker, FindingSeverity, finding_previews};

    #[test]
    fn findings_are_projected_from_assistant_output() {
        let messages = vec![assistant_message(
            "- 高危：`src/command.rs:42` 存在命令执行风险，可能对应 CWE-78",
        )];

        let findings = finding_previews(&messages);

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings.first().map(|finding| finding.severity),
            Some(FindingSeverity::High)
        );
        assert_eq!(
            findings
                .first()
                .and_then(|finding| finding.taxonomy.as_deref()),
            Some("CWE-78")
        );
        assert_eq!(
            findings.first().map(|finding| finding.location.as_str()),
            Some("src/command.rs:42")
        );
    }

    #[test]
    fn findings_stay_empty_without_candidate_output() {
        let messages = vec![assistant_message("本轮没有发现明确问题。")];

        assert!(finding_previews(&messages).is_empty());
    }

    #[test]
    fn command_approval_resolves_matching_request() {
        let broker = CommandApprovalBroker::new();
        let (response_tx, response_rx) = mpsc::channel();
        let id = broker
            .register_pending("允许执行 pwd 吗？".to_owned(), response_tx)
            .expect("应能注册待批准请求");

        let pending = broker.take_pending(id).expect("应能取出匹配请求");
        pending.response_tx.send(true).expect("应能发送批准结果");

        assert!(response_rx.recv().expect("应能接收批准结果"));
        assert!(broker.take_pending(id).is_err());
    }

    #[test]
    fn command_approval_keeps_pending_when_id_mismatches() {
        let broker = CommandApprovalBroker::new();
        let (response_tx, _response_rx) = mpsc::channel();
        let id = broker
            .register_pending("允许执行 pwd 吗？".to_owned(), response_tx)
            .expect("应能注册待批准请求");

        assert!(broker.take_pending(id + 1).is_err());
        let _pending = broker.take_pending(id).expect("原请求应仍然待处理");
    }

    fn assistant_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: Some(content.to_owned()),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
        }
    }
}
