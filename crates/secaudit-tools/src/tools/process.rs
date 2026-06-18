//! 外部工具进程创建辅助。

use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn hidden_command(program: &str) -> Command {
    let mut command = Command::new(program);
    apply_hidden_window(&mut command);
    command
}

#[cfg(windows)]
fn apply_hidden_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_hidden_window(_command: &mut Command) {}
