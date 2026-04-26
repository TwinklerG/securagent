mod commands;
mod tui;

use std::path::{Path, PathBuf};

use secaudit_agent::state::AgentState;
use secaudit_core::Config;

#[derive(Debug, Clone)]
struct ChatRequest {
    input: String,
}

#[derive(Debug)]
enum WorkerEvent {
    Ready {
        tool_names: Vec<String>,
        message_count: usize,
    },
    State(AgentState),
    Think(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        result: String,
    },
    ChatDone {
        response: Result<String, String>,
        message_count: usize,
    },
    Status {
        message_count: usize,
    },
}

#[derive(Debug)]
enum WorkerCommand {
    Chat(ChatRequest),
    ClearSession,
    QueryStatus,
    Shutdown,
}

fn build_worker_session(work_dir: &Path) -> secaudit_agent::Session {
    secaudit_agent::Session::new(work_dir.to_path_buf())
}

fn parse_user_input(input: &str) -> commands::UserInput {
    commands::parse(input)
}

pub async fn run(config: Config, work_dir: PathBuf) {
    tui::run(config, work_dir).await;
}
