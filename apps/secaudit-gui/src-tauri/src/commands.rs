use tauri::{AppHandle, State};

use crate::dto::AgentWorkbench;
use crate::runtime::RuntimeState;

#[tauri::command]
pub(crate) async fn init_workbench(
    state: State<'_, RuntimeState>,
) -> Result<AgentWorkbench, String> {
    let runtime = state.lock().await;
    Ok(runtime.snapshot())
}

#[tauri::command]
pub(crate) async fn send_audit_message(
    state: State<'_, RuntimeState>,
    message: String,
) -> Result<AgentWorkbench, String> {
    let mut runtime = state.lock().await;
    runtime.chat(message).await
}

#[tauri::command]
pub(crate) async fn new_session(state: State<'_, RuntimeState>) -> Result<AgentWorkbench, String> {
    let mut runtime = state.lock().await;
    runtime.new_session()
}

#[tauri::command]
pub(crate) async fn switch_session(
    state: State<'_, RuntimeState>,
    session_id: String,
) -> Result<AgentWorkbench, String> {
    let mut runtime = state.lock().await;
    runtime.switch_session(&session_id)
}

#[tauri::command]
pub(crate) async fn archive_session(
    state: State<'_, RuntimeState>,
    session_id: String,
) -> Result<AgentWorkbench, String> {
    let mut runtime = state.lock().await;
    runtime.archive_session(&session_id)
}

#[tauri::command]
pub(crate) async fn set_work_dir(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    work_dir: String,
) -> Result<AgentWorkbench, String> {
    let mut runtime = state.lock().await;
    runtime.set_work_dir(&app, &work_dir)
}
