// Web SSE 服务器：通过 Server-Sent Events 实时推送审计进度

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use crate::agent::Agent;
use crate::config::Config;

// ─── SSE 事件名称常量 ────────────────────────────────────────────────────────

/// 状态变更事件名
const EVENT_STATE: &str = "state";
/// 思考过程事件名
const EVENT_THINK: &str = "think";
/// 工具调用事件名
const EVENT_TOOL_CALL: &str = "tool_call";
/// 审计报告事件名
const EVENT_REPORT: &str = "report";
/// 错误事件名
const EVENT_ERROR: &str = "error";

/// 广播通道容量
const BROADCAST_CAPACITY: usize = 256;

// ─── 请求/响应类型 ───────────────────────────────────────────────────────────

/// 审计请求体
#[derive(Deserialize)]
struct AuditRequest {
    /// 待审计的源代码
    code: String,
    /// 编程语言（可选，不指定则使用默认值）
    language: Option<String>,
}

/// 通用状态响应
#[derive(Serialize)]
struct StatusResponse {
    /// 状态描述
    status: &'static str,
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
    /// 审计报告
    Report { data: serde_json::Value },
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
            Self::Report { .. } => EVENT_REPORT,
            Self::Error { .. } => EVENT_ERROR,
        }
    }
}

// ─── 应用状态 ────────────────────────────────────────────────────────────────

/// 共享应用状态
#[derive(Clone)]
struct AppState {
    /// SSE 事件广播发送端
    tx: broadcast::Sender<SsePayload>,
    /// 应用配置
    config: Config,
}

// ─── 路由处理器 ──────────────────────────────────────────────────────────────

/// 默认语言名称
const DEFAULT_LANGUAGE: &str = "unknown";
/// Web 模式的默认审计目标名称
const WEB_TARGET: &str = "<web-input>";

/// 状态值：已启动
const STATUS_STARTED: &str = "started";
/// 状态值：正常
const STATUS_OK: &str = "ok";

/// `POST /api/audit` — 启动审计任务
async fn handle_audit(
    State(state): State<AppState>,
    Json(req): Json<AuditRequest>,
) -> impl IntoResponse {
    let tx = state.tx.clone();
    let config = state.config.clone();
    let language = req.language.unwrap_or_else(|| DEFAULT_LANGUAGE.into());
    let code = req.code;

    // 在后台任务中执行审计，通过广播通道推送事件
    tokio::spawn(async move {
        let tx_state = tx.clone();
        let tx_think = tx.clone();
        let tx_tool = tx.clone();

        let mut agent = Agent::new(config);

        agent.on_state_change(move |s| {
            let _ = tx_state.send(SsePayload::State {
                state: s.label().into(),
            });
        });

        agent.on_think(move |text| {
            let _ = tx_think.send(SsePayload::Think {
                content: text.into(),
            });
        });

        agent.on_tool_call(move |name, args| {
            let _ = tx_tool.send(SsePayload::ToolCall {
                name: name.into(),
                args: args.into(),
            });
        });

        match agent.audit(&code, &language, WEB_TARGET).await {
            Ok(report) => {
                // 序列化报告为 JSON Value
                let data = serde_json::to_value(&report).unwrap_or_default();
                let _ = tx.send(SsePayload::Report { data });
            }
            Err(e) => {
                let _ = tx.send(SsePayload::Error {
                    message: e.to_string(),
                });
            }
        }
    });

    Json(StatusResponse {
        status: STATUS_STARTED,
    })
}

/// `GET /api/events` — SSE 事件流端点
async fn handle_events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(payload) => {
            let data = serde_json::to_string(&payload).unwrap_or_default();
            let event = Event::default().event(payload.event_name()).data(data);
            Some(Ok(event))
        }
        // 接收端滞后导致消息丢失，跳过
        Err(_) => None,
    });

    Sse::new(stream)
}

/// `GET /api/health` — 健康检查
async fn handle_health() -> impl IntoResponse {
    Json(StatusResponse { status: STATUS_OK })
}

// ─── 服务器启动 ──────────────────────────────────────────────────────────────

/// 启动 Web SSE 服务器
///
/// 在指定端口监听，提供审计 API 和 SSE 事件推送。
pub async fn start(port: u16, config: Config) {
    let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);

    let state = AppState { tx, config };

    let app = Router::new()
        .route("/api/audit", post(handle_audit))
        .route("/api/events", get(handle_events))
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
