//! 非交互调试模式：记录轨迹并输出结构化 JSON。

mod confirm;
mod input;
mod printer;
mod response;
mod trace;

use std::fmt::Display;
use std::path::Path;
use std::process;

use colored::Colorize;
use secaudit_conversation::{ConversationService, ManagedSession};

use crate::OutputFormat;

pub(crate) use confirm::{build_confirm_callback, confirm_mode_name, is_user_denied_error};
pub(crate) use input::resolve_chat_messages;
pub(crate) use printer::{print_archived_session, print_response, print_session_list};
pub use response::{
    HeadlessResponse, HeadlessResponseContext, SessionSnapshot, TurnRecord, collect_session_metrics,
};
pub use trace::TraceRecorder;

/// 打开默认会话服务；失败时按 CLI 约定输出错误并退出。
pub(crate) fn open_conversation_service_or_exit() -> ConversationService {
    match ConversationService::with_default_storage() {
        Ok(service) => service,
        Err(error) => exit_session_error(error),
    }
}

/// 处理 headless 会话管理请求。
///
/// 返回 `true` 表示已处理管理请求，调用方无需继续执行 chat。
pub(crate) fn handle_session_management_request(
    list_sessions: bool,
    archive_session: Option<&str>,
    output_format: OutputFormat,
    work_dir: &Path,
) -> bool {
    if !list_sessions && archive_session.is_none() {
        return false;
    }

    let service = open_conversation_service_or_exit();

    if list_sessions {
        match service.list_sessions(work_dir) {
            Ok(sessions) => print_session_list(output_format, &sessions),
            Err(error) => exit_session_error(error),
        }
    }

    if let Some(session_id) = archive_session {
        match service.archive_session(work_dir, session_id) {
            Ok(metadata) => print_archived_session(output_format, &metadata),
            Err(error) => exit_session_error(error),
        }
    }

    true
}

/// 按可选 session id 加载既有会话；未指定时创建新会话。
pub(crate) fn load_or_start_session(
    service: &ConversationService,
    work_dir: &Path,
    session_id: Option<&str>,
) -> secaudit_conversation::Result<ManagedSession> {
    if let Some(session_id) = session_id {
        service.load_session(work_dir, session_id)
    } else {
        service.start_session(work_dir)
    }
}

fn exit_session_error(error: impl Display) -> ! {
    eprintln!("{}: {error}", "会话错误".red().bold());
    process::exit(1);
}
