mod commands;
mod dto;
mod runtime;
mod tools;

use tauri::Manager;
use tokio::sync::Mutex;

use runtime::{CommandApprovalBroker, GuiRuntime};

/// 启动 Tauri 桌面应用。
///
/// # Errors
///
/// 初始化运行时或启动 Tauri 事件循环失败时返回错误。
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let approvals = CommandApprovalBroker::new();
            let runtime = GuiRuntime::new(app.handle(), approvals.clone())?;
            app.manage(approvals);
            app.manage(Mutex::new(runtime));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::init_workbench,
            commands::send_audit_message,
            commands::new_session,
            commands::switch_session,
            commands::archive_session,
            commands::set_work_dir,
            commands::resolve_command_approval
        ])
        .run(tauri::generate_context!())
}
