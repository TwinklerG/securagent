#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() -> tauri::Result<()> {
    secaudit_gui_lib::run()
}
