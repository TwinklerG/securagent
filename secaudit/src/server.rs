// Web 会话式 API 服务器：支持多会话对话与 SSE 实时推送

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::task;
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use crate::agent::Agent;
use crate::config::Config;
use crate::llm::{ChatMessage, Role};
use crate::output::truncate_with_ellipsis;
use crate::session::Session;
use crate::tools::ConfirmFn;

// ─── SSE 事件名称常量 ────────────────────────────────────────────────────────

/// 状态变更事件名
const EVENT_STATE: &str = "state";
/// 思考过程事件名
const EVENT_THINK: &str = "think";
/// 工具调用事件名
const EVENT_TOOL_CALL: &str = "tool_call";
/// 工具结果事件名
const EVENT_TOOL_RESULT: &str = "tool_result";
/// Agent 回复事件名
const EVENT_MESSAGE: &str = "message";
/// 确认请求事件名
const EVENT_CONFIRM_REQUEST: &str = "confirm_request";
/// 错误事件名
const EVENT_ERROR: &str = "error";

/// 广播通道容量
const BROADCAST_CAPACITY: usize = 256;

/// 会话 ID 计数器起始值
const SESSION_ID_START: u64 = 1;

// ─── 请求/响应类型 ───────────────────────────────────────────────────────────

/// 创建会话请求
#[derive(Deserialize)]
struct CreateSessionRequest {
    /// 工作目录（审计根目录）
    work_dir: String,
}

/// 发送消息请求
#[derive(Deserialize)]
struct SendMessageRequest {
    /// 消息内容
    content: String,
}

/// 同步聊天请求
#[derive(Deserialize)]
struct ChatRequest {
    /// 消息内容
    content: String,
    /// 是否自动确认所有操作（默认 true）
    #[serde(default = "default_auto_confirm")]
    auto_confirm: bool,
}

/// 默认自动确认
fn default_auto_confirm() -> bool {
    true
}

/// 确认响应请求
#[derive(Deserialize)]
struct ConfirmResponse {
    /// 是否批准
    approved: bool,
}

/// 创建会话响应
#[derive(Serialize)]
struct SessionCreated {
    /// 会话 ID
    id: u64,
}

/// 通用状态响应
#[derive(Serialize)]
struct StatusResponse {
    /// 状态描述
    status: &'static str,
}

/// 同步聊天响应（`/chat` 端点）
#[derive(Serialize)]
struct ChatResponse {
    /// Agent 最终回复文本
    message: String,
    /// 工具调用记录
    tool_calls: Vec<ToolCallRecord>,
    /// 状态变迁历史
    state_history: Vec<String>,
    /// 处理耗时（毫秒）
    duration_ms: u64,
}

/// 工具调用记录
#[derive(Serialize)]
struct ToolCallRecord {
    /// 工具名称
    name: String,
    /// 调用参数
    args: String,
    /// 执行结果
    result: String,
}

/// 会话历史响应（`/history` 端点）
#[derive(Serialize)]
struct HistoryResponse {
    /// 会话 ID
    session_id: String,
    /// 对话消息列表
    messages: Vec<ChatMessage>,
    /// 工具调用总数
    tool_calls_count: usize,
    /// 创建时间
    created_at: String,
}

/// SSE 事件载荷（内部传输用）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SsePayload {
    /// 状态变更
    State { state: String },
    /// 思考内容
    Think { content: String },
    /// 工具调用
    ToolCall { name: String, args: String },
    /// 工具结果
    ToolResult { name: String, result: String },
    /// Agent 文本回复
    Message { content: String },
    /// 确认请求：Agent 需要用户批准才能执行某操作
    ConfirmRequest { prompt: String },
    /// 错误信息
    Error { message: String },
}

impl SsePayload {
    /// 获取对应的 SSE 事件名称
    fn event_name(&self) -> &'static str {
        match self {
            Self::State { .. } => EVENT_STATE,
            Self::Think { .. } => EVENT_THINK,
            Self::ToolCall { .. } => EVENT_TOOL_CALL,
            Self::ToolResult { .. } => EVENT_TOOL_RESULT,
            Self::Message { .. } => EVENT_MESSAGE,
            Self::ConfirmRequest { .. } => EVENT_CONFIRM_REQUEST,
            Self::Error { .. } => EVENT_ERROR,
        }
    }
}

// ─── 会话存储 ────────────────────────────────────────────────────────────────

/// 单个会话实例
struct SessionInstance {
    /// Agent 实例
    agent: Agent,
    /// 会话状态
    session: Session,
    /// SSE 事件广播发送端
    tx: broadcast::Sender<SsePayload>,
}

type SharedSession = Arc<Mutex<SessionInstance>>;

/// 共享应用状态
#[derive(Clone)]
struct AppState {
    /// 应用配置
    config: Config,
    /// 活跃会话（会话 ID → 会话实例）
    sessions: Arc<Mutex<HashMap<u64, SharedSession>>>,
    /// 下一个会话 ID
    next_id: Arc<Mutex<u64>>,
    /// 待处理的确认请求（会话 ID → 响应发送端）
    ///
    /// 与 `SessionInstance` 分离存储，避免 Agent 运行时持锁导致确认端点死锁。
    pending_confirms: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
}

impl AppState {
    async fn allocate_session_id(&self) -> u64 {
        let mut next_id = self.next_id.lock().await;
        let id = *next_id;
        *next_id += 1;
        id
    }

    async fn get_session(&self, id: u64) -> Option<SharedSession> {
        self.sessions.lock().await.get(&id).cloned()
    }

    async fn insert_session(&self, id: u64, session: SharedSession) {
        self.sessions.lock().await.insert(id, session);
    }

    async fn take_pending_confirm(&self, id: u64) -> Option<oneshot::Sender<bool>> {
        self.pending_confirms.lock().await.remove(&id)
    }
}

/// 创建 Web 模式的异步确认回调。
///
/// 当 Agent 需要用户确认时：
/// 1. 通过 SSE 广播 `ConfirmRequest` 事件
/// 2. 创建 oneshot channel 并存入 `pending_confirms`
/// 3. 阻塞等待用户通过 `/api/sessions/:id/confirm` 端点回复
fn web_confirm(
    session_id: u64,
    tx: broadcast::Sender<SsePayload>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
) -> ConfirmFn {
    Arc::new(move |prompt: &str| {
        let (resp_tx, resp_rx) = oneshot::channel();

        // 存储待处理的确认响应发送端
        if let Ok(mut map) = pending.try_lock() {
            map.insert(session_id, resp_tx);
        } else {
            return false;
        }

        // 通过 SSE 广播确认请求
        let _ = tx.send(SsePayload::ConfirmRequest {
            prompt: prompt.into(),
        });

        // 在同步回调中安全等待异步 channel 响应
        task::block_in_place(|| Handle::current().block_on(resp_rx).unwrap_or(false))
    })
}

// ─── 路由处理器 ──────────────────────────────────────────────────────────────

/// 状态值：正常
const STATUS_OK: &str = "ok";
/// 工具结果摘要最大长度
const TOOL_RESULT_SUMMARY_LEN: usize = 500;

fn json_error(message: impl Into<String>) -> Response {
    Json(serde_json::json!({ "error": message.into() })).into_response()
}

fn attach_sse_callbacks(agent: &mut Agent, tx: &broadcast::Sender<SsePayload>) {
    let tx_state = tx.clone();
    agent.on_state_change(move |s| {
        let _ = tx_state.send(SsePayload::State {
            state: s.label().into(),
        });
    });

    let tx_think = tx.clone();
    agent.on_think(move |text| {
        let _ = tx_think.send(SsePayload::Think {
            content: text.into(),
        });
    });

    let tx_tool = tx.clone();
    agent.on_tool_call(move |name, args| {
        let _ = tx_tool.send(SsePayload::ToolCall {
            name: name.into(),
            args: args.into(),
        });
    });

    let tx_result = tx.clone();
    agent.on_tool_result(move |name, result| {
        let summary = truncate_with_ellipsis(result, TOOL_RESULT_SUMMARY_LEN);
        let _ = tx_result.send(SsePayload::ToolResult {
            name: name.into(),
            result: summary,
        });
    });
}

/// `POST /api/sessions` — 创建新会话
async fn handle_create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let work_dir = PathBuf::from(&req.work_dir);

    if !work_dir.is_dir() {
        return json_error(format!("工作目录不存在：{}", req.work_dir));
    }

    let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
    let id = state.allocate_session_id().await;

    // 创建异步确认回调
    let confirm = web_confirm(id, tx.clone(), Arc::clone(&state.pending_confirms));

    let mut agent = Agent::new(state.config.clone(), work_dir.clone(), confirm);

    // 设置 SSE 回调
    attach_sse_callbacks(&mut agent, &tx);

    let session_instance = SessionInstance {
        agent,
        session: Session::new(work_dir),
        tx,
    };

    let instance = Arc::new(Mutex::new(session_instance));
    state.insert_session(id, instance).await;

    Json(SessionCreated { id }).into_response()
}

/// `POST /api/sessions/:id/messages` — 发送消息
async fn handle_send_message(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let Some(instance) = state.get_session(id).await else {
        return json_error("会话不存在");
    };

    let content = req.content;

    // 在后台处理消息，通过 SSE 推送结果
    tokio::spawn(async move {
        let mut inst = instance.lock().await;

        let SessionInstance { agent, session, tx } = &mut *inst;

        let tx = tx.clone();

        match agent.chat(session, &content).await {
            Ok(response) => {
                let _ = tx.send(SsePayload::Message { content: response });
            }
            Err(e) => {
                let _ = tx.send(SsePayload::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    Json(StatusResponse { status: STATUS_OK }).into_response()
}

/// `GET /api/sessions/:id/events` — SSE 事件流
async fn handle_session_events(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let Some(instance) = state.get_session(id).await else {
        return json_error("会话不存在");
    };

    let rx = instance.lock().await.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(payload) => {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            let event = Event::default().event(payload.event_name()).data(data);
            Some(Ok::<_, Infallible>(event))
        }
        Err(_) => None,
    });

    Sse::new(stream).into_response()
}

/// `POST /api/sessions/:id/confirm` — 回复确认请求
///
/// Agent 在执行危险操作（如非白名单命令、覆写文件）前通过 SSE 推送 `confirm_request`，
/// 前端收到后展示确认对话框，用户决定后调用此端点发送结果。
async fn handle_confirm(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(req): Json<ConfirmResponse>,
) -> impl IntoResponse {
    if let Some(tx) = state.take_pending_confirm(id).await {
        let _ = tx.send(req.approved);
        Json(StatusResponse { status: STATUS_OK }).into_response()
    } else {
        json_error("无待处理的确认请求")
    }
}

/// `POST /api/sessions/:id/chat` — 同步聊天（等待 Agent 完成后一次性返回）
///
/// 适用于 AI Agent 或脚本调用，无需解析 SSE 事件流。
/// `auto_confirm` 默认为 true，自动批准所有确认请求。
async fn handle_chat(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let Some(instance) = state.get_session(id).await else {
        return json_error("会话不存在");
    };

    let start = Instant::now();

    // 订阅事件广播，收集工具调用和状态变迁
    let rx = instance.lock().await.tx.subscribe();

    // 如果 auto_confirm，自动处理确认请求
    let pending = Arc::clone(&state.pending_confirms);
    let auto_confirm = req.auto_confirm;
    let confirm_session_id = id;

    // 后台自动确认任务
    let confirm_handle = if auto_confirm {
        let mut confirm_rx = instance.lock().await.tx.subscribe();
        Some(tokio::spawn(async move {
            loop {
                match confirm_rx.recv().await {
                    Ok(SsePayload::ConfirmRequest { .. }) => {
                        // 自动批准
                        if let Some(tx) = pending.lock().await.remove(&confirm_session_id) {
                            let _ = tx.send(true);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }))
    } else {
        None
    };

    // 同步执行 chat
    let chat_result = {
        let mut inst = instance.lock().await;
        let SessionInstance { agent, session, .. } = &mut *inst;
        agent.chat(session, &req.content).await
    };

    // 收集已广播的事件
    let mut tool_calls = Vec::new();
    let mut state_history = Vec::new();
    let mut stream = BroadcastStream::new(rx);

    while let Ok(Some(Ok(payload))) = timeout(Duration::from_millis(10), stream.next()).await {
        match payload {
            SsePayload::ToolCall { name, args } => {
                tool_calls.push(ToolCallRecord {
                    name,
                    args,
                    result: String::new(),
                });
            }
            SsePayload::ToolResult { name, result } => {
                // 填充最近一个同名工具调用的结果
                if let Some(record) = tool_calls.iter_mut().rev().find(|r| r.name == name) {
                    record.result.clone_from(&result);
                }
            }
            SsePayload::State { state } => {
                state_history.push(state);
            }
            _ => {}
        }
    }

    // 取消自动确认任务
    if let Some(handle) = confirm_handle {
        handle.abort();
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    match chat_result {
        Ok(message) => Json(
            serde_json::to_value(ChatResponse {
                message,
                tool_calls,
                state_history,
                duration_ms,
            })
            .unwrap_or_default(),
        )
        .into_response(),
        Err(e) => Json(serde_json::json!({
            "error": e.to_string(),
            "duration_ms": duration_ms,
        }))
        .into_response(),
    }
}

/// `GET /api/sessions/:id/history` — 获取会话完整历史
///
/// 返回对话消息和工具调用统计，便于调试和评估。
async fn handle_history(State(state): State<AppState>, Path(id): Path<u64>) -> impl IntoResponse {
    let Some(instance) = state.get_session(id).await else {
        return json_error("会话不存在");
    };

    let inst = instance.lock().await;
    let session = &inst.session;

    let messages = session.messages().to_vec();
    let tool_calls_count = messages
        .iter()
        .filter(|m| matches!(m.role, Role::Tool))
        .count();

    Json(HistoryResponse {
        session_id: session.id.clone(),
        messages,
        tool_calls_count,
        created_at: session.created_at.clone(),
    })
    .into_response()
}

/// `GET /api/health` — 健康检查
async fn handle_health() -> impl IntoResponse {
    Json(StatusResponse { status: STATUS_OK })
}

// ─── 服务器启动 ──────────────────────────────────────────────────────────────

/// 启动 Web 会话 API 服务器
pub async fn start(port: u16, config: Config) {
    let state = AppState {
        config,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        next_id: Arc::new(Mutex::new(SESSION_ID_START)),
        pending_confirms: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/api/sessions", post(handle_create_session))
        .route("/api/sessions/{id}/messages", post(handle_send_message))
        .route("/api/sessions/{id}/chat", post(handle_chat))
        .route("/api/sessions/{id}/history", get(handle_history))
        .route("/api/sessions/{id}/events", get(handle_session_events))
        .route("/api/sessions/{id}/confirm", post(handle_confirm))
        .route("/api/health", get(handle_health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("secaudit Web 服务器启动：http://{addr}");

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("端口绑定失败：{e}");
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("服务器错误：{e}");
    }
}
