#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI 模式需要直接输出到终端"
)]

mod headless;
mod interactive;
mod output;

use std::env;
use std::fs;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use colored::Colorize;
use secaudit_agent::strategy;
use secaudit_agent::{Agent, to_multi_turn_sample};
use secaudit_conversation::{ConversationService, SessionMetadata};
use secaudit_core::Config;
use serde::Serialize;

use crate::headless::{
    HeadlessResponse, HeadlessResponseContext, SessionSnapshot, TraceRecorder, TurnRecord,
    collect_session_metrics,
};

/// secaudit -- 安全代码审计 LLM Agent
#[derive(Parser)]
#[command(version, about = "安全代码审计 LLM Agent")]
struct Cli {
    /// 待审计的源码文件路径（省略则进入交互模式）
    #[arg(value_name = "FILE")]
    target: Option<String>,

    /// 编程语言（不指定则自动检测）
    #[arg(short, long)]
    language: Option<String>,

    /// 输出格式：text（默认）、json、markdown
    #[arg(short = 'f', long, default_value = "text")]
    format: String,

    /// trajectory 导出路径（可选，仅单文件模式）
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,

    /// 运行模式：auto（默认）、chat
    #[arg(short, long, default_value = MODE_AUTO)]
    mode: Mode,

    /// 推理策略：react（默认）、reflexion（仅单文件模式）
    #[arg(short, long, default_value = strategy::STRATEGY_REACT)]
    strategy: String,

    /// chat 模式输入消息（单条）。未提供时可从 stdin 读取。
    #[arg(long)]
    message: Option<String>,

    /// chat 模式输入消息列表（JSON 数组字符串）。
    #[arg(long, conflicts_with = "message")]
    messages_json: Option<String>,

    /// chat 模式确认策略：deny（默认）/allow/ask。
    #[arg(long, default_value = CONFIRM_MODE_DENY)]
    confirm_mode: ConfirmMode,

    /// chat 模式输出格式：json（默认）或 text
    #[arg(long, default_value = OUTPUT_JSON)]
    output_format: OutputFormat,

    /// chat 模式：恢复并继续指定会话 ID。
    #[arg(long)]
    session: Option<String>,

    /// chat 模式：列出当前项目下的历史会话，不调用 LLM。
    #[arg(long, conflicts_with = "archive_session")]
    list_sessions: bool,

    /// chat 模式：归档指定会话 ID，不调用 LLM。
    #[arg(long, value_name = "ID", conflicts_with = "list_sessions")]
    archive_session: Option<String>,
}

/// 支持的文件扩展名到语言名的映射
const EXTENSION_MAP: &[(&[&str], &str)] = &[
    (&["py"], "python"),
    (&["js", "jsx", "ts", "tsx"], "javascript"),
    (&["rs"], "rust"),
    (&["java"], "java"),
    (&["go"], "go"),
    (&["rb"], "ruby"),
    (&["php"], "php"),
    (&["c", "h"], "c"),
    (&["cpp", "cc", "cxx", "hpp"], "cpp"),
];

/// 未知语言的默认名称
const UNKNOWN_LANGUAGE: &str = "unknown";

/// 自动模式标识（根据是否有文件参数决定行为）
const MODE_AUTO: &str = "auto";

/// chat 输出格式：JSON
const OUTPUT_JSON: &str = "json";

/// 确认策略默认值
const CONFIRM_MODE_DENY: &str = "deny";

/// 用户主动拒绝确认时的退出码
const EXIT_USER_DENIED: i32 = 130;

/// tool 调用拒绝的错误消息片段
const MSG_USER_DENIED: &str = "用户拒绝执行该命令";

#[derive(Serialize)]
struct SessionListOutput<'a> {
    status: &'static str,
    sessions: &'a [SessionMetadata],
}

#[derive(Serialize)]
struct ArchiveSessionOutput<'a> {
    status: &'static str,
    session: &'a SessionMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// 自动模式：FILE 参数存在则单文件审计，否则进入交互模式
    Auto,
    /// 非交互调试模式：执行单轮或多轮 chat 并返回结构化结果
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ConfirmMode {
    /// 默认拒绝所有确认请求
    Deny,
    /// 自动允许所有确认请求
    Allow,
    /// 每次确认请求都在 stdin 交互询问
    Ask,
}

impl ConfirmMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::Allow => "allow",
            Self::Ask => "ask",
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    if cli.mode == Mode::Chat || cli.list_sessions || cli.archive_session.is_some() {
        if handle_headless_session_management(&cli) {
            return;
        }

        // 加载配置
        let config = match Config::from_env() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: {e}", "配置错误".red().bold());
                process::exit(1);
            }
        };
        run_headless_chat(&cli, config).await;
    } else if let Some(target) = &cli.target {
        let config = match Config::from_env() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: {e}", "配置错误".red().bold());
                process::exit(1);
            }
        };
        run_single_file(&cli, config, target).await;
    } else {
        let config = match Config::from_env() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: {e}", "配置错误".red().bold());
                process::exit(1);
            }
        };
        run_interactive(config).await;
    }
}

/// 交互模式：进入 REPL 循环
async fn run_interactive(config: Config) {
    let work_dir = current_work_dir();
    interactive::run(config, work_dir).await;
}

/// 单文件审计模式
async fn run_single_file(cli: &Cli, mut config: Config, target: &str) {
    // 读取源码文件
    let code = match fs::read_to_string(target) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: 无法读取文件 {target}: {e}", "错误".red().bold());
            process::exit(1);
        }
    };

    // 检测语言
    let language = cli
        .language
        .clone()
        .unwrap_or_else(|| detect_language(target));

    // 启动信息
    println!("{}", "secaudit -- 安全代码审计 Agent".bold());
    println!("目标：{}", target.cyan());
    println!("语言：{}", language.cyan());
    println!("策略：{}", cli.strategy.cyan());
    output::cli::print_separator();

    // 创建 Agent 并设置回调
    config.reasoning_strategy.clone_from(&cli.strategy);
    let work_dir = current_work_dir();
    let mut agent = Agent::new(config, work_dir, Arc::new(|_| true));
    agent.on_state_change(output::cli::print_state);
    agent.on_think(output::cli::print_thinking);
    agent.on_tool_call(output::cli::print_tool_call);

    // 执行审计
    match agent.audit(&code, &language, target).await {
        Ok(report) => {
            output::cli::print_separator();

            match cli.format.as_str() {
                "json" => match output::report::to_json(&report) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("{}: {e}", "输出错误".red().bold());
                        process::exit(1);
                    }
                },
                "markdown" | "md" => {
                    println!("{}", output::report::to_markdown(&report));
                }
                _ => {
                    output::cli::print_report(&report);
                }
            }

            // 导出 trajectory JSON
            if let Some(output_path) = &cli.output {
                let sample = to_multi_turn_sample(&report.messages, &report.findings, target);
                match serde_json::to_string_pretty(&sample) {
                    Ok(json) => {
                        if let Err(e) = fs::write(output_path, json) {
                            eprintln!("{}: 写入 trajectory 文件失败：{e}", "错误".red().bold());
                        } else {
                            println!(
                                "{} trajectory 已导出至 {output_path}",
                                "完成".green().bold()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: trajectory 序列化失败：{e}", "错误".red().bold());
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{}: {e}", "审计失败".red().bold());
            process::exit(1);
        }
    }
}

/// 非交互 chat 模式：执行一轮或多轮对话并输出结构化结果。
async fn run_headless_chat(cli: &Cli, config: Config) {
    let work_dir = current_work_dir();
    let work_dir_display = work_dir.display().to_string();
    let inputs = resolve_chat_messages(cli);
    let conversation = match ConversationService::with_default_storage() {
        Ok(service) => service,
        Err(error) => {
            eprintln!("{}: {error}", "会话错误".red().bold());
            process::exit(1);
        }
    };

    let recorder = TraceRecorder::new();
    let confirm_mode = cli.confirm_mode;
    let confirm_mode_name = confirm_mode.as_str().to_owned();
    let confirm = build_confirm_callback(confirm_mode, recorder.clone());

    let mut agent = Agent::new(config, work_dir.clone(), confirm);
    recorder.attach(&mut agent);
    let mut managed_session = match load_or_start_session(&conversation, &work_dir, cli) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("{}: {error}", "会话错误".red().bold());
            process::exit(1);
        }
    };

    let mut turns = Vec::new();
    let start = Instant::now();
    let mut final_message = String::new();
    let mut failure: Option<String> = None;

    for (index, user_message) in inputs.iter().enumerate() {
        let turn_start = Instant::now();
        match conversation
            .chat(&mut agent, &mut managed_session, user_message)
            .await
        {
            Ok(message) => {
                let duration_ms = turn_start.elapsed().as_millis() as u64;
                final_message.clone_from(&message);
                turns.push(TurnRecord::success(
                    index + 1,
                    user_message.clone(),
                    message,
                    duration_ms,
                ));
            }
            Err(error) => {
                let error_message = error.to_string();
                let duration_ms = turn_start.elapsed().as_millis() as u64;
                turns.push(TurnRecord::error(
                    index + 1,
                    user_message.clone(),
                    error_message.clone(),
                    duration_ms,
                ));
                failure = Some(error_message);
                break;
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let trace = recorder.snapshot();
    let session_snapshot = SessionSnapshot::from_session(managed_session.session());
    let metrics = collect_session_metrics(&session_snapshot.messages);
    let session_management = Some(conversation.management_info(&managed_session));

    let context = HeadlessResponseContext {
        turns,
        trace,
        session: session_snapshot,
        metrics,
        duration_ms,
        work_dir: work_dir_display,
        confirm_mode: confirm_mode_name,
        session_management,
    };

    let response = if let Some(error) = &failure {
        HeadlessResponse::error(error.clone(), context)
    } else {
        HeadlessResponse::success(final_message, context)
    };

    print_headless_response(cli.output_format, &response);

    if let Some(error) = failure {
        if error.contains(MSG_USER_DENIED) {
            process::exit(EXIT_USER_DENIED);
        }
        process::exit(1);
    }
}

fn handle_headless_session_management(cli: &Cli) -> bool {
    if !cli.list_sessions && cli.archive_session.is_none() {
        return false;
    }

    let work_dir = current_work_dir();
    let service = match ConversationService::with_default_storage() {
        Ok(service) => service,
        Err(error) => {
            eprintln!("{}: {error}", "会话错误".red().bold());
            process::exit(1);
        }
    };

    if cli.list_sessions {
        match service.list_sessions(&work_dir) {
            Ok(sessions) => print_session_list(cli.output_format, &sessions),
            Err(error) => {
                eprintln!("{}: {error}", "会话错误".red().bold());
                process::exit(1);
            }
        }
    }

    if let Some(session_id) = &cli.archive_session {
        match service.archive_session(&work_dir, session_id) {
            Ok(metadata) => print_archived_session(cli.output_format, &metadata),
            Err(error) => {
                eprintln!("{}: {error}", "会话错误".red().bold());
                process::exit(1);
            }
        }
    }

    true
}

fn load_or_start_session(
    service: &ConversationService,
    work_dir: &Path,
    cli: &Cli,
) -> secaudit_conversation::Result<secaudit_conversation::ManagedSession> {
    if let Some(session_id) = &cli.session {
        service.load_session(work_dir, session_id)
    } else {
        service.start_session(work_dir)
    }
}

fn print_session_list(output_format: OutputFormat, sessions: &[SessionMetadata]) {
    match output_format {
        OutputFormat::Json => {
            let output = SessionListOutput {
                status: "success",
                sessions,
            };
            print_json_or_exit(&output);
        }
        OutputFormat::Text => {
            println!("{}", "secaudit chat 会话列表".green().bold());
            if sessions.is_empty() {
                println!("当前项目没有历史会话。");
                return;
            }
            for session in sessions {
                println!(
                    "{}  {}  {}  messages={}  {}",
                    session.session_id,
                    session.status,
                    session.updated_at,
                    session.message_count,
                    session.title
                );
            }
        }
    }
}

fn print_archived_session(output_format: OutputFormat, session: &SessionMetadata) {
    match output_format {
        OutputFormat::Json => {
            let output = ArchiveSessionOutput {
                status: "success",
                session,
            };
            print_json_or_exit(&output);
        }
        OutputFormat::Text => {
            println!("{}", "secaudit chat 会话已归档".green().bold());
            println!("会话：{}", session.session_id);
            println!("状态：{}", session.status);
            println!("更新时间：{}", session.updated_at);
        }
    }
}

fn print_json_or_exit<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("{}: JSON 序列化失败：{error}", "错误".red().bold());
            process::exit(1);
        }
    }
}

fn current_work_dir() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_chat_messages(cli: &Cli) -> Vec<String> {
    if let Some(messages_json) = &cli.messages_json {
        match serde_json::from_str::<Vec<String>>(messages_json) {
            Ok(messages) => {
                let filtered: Vec<String> = messages
                    .into_iter()
                    .map(|message| message.trim().to_owned())
                    .filter(|message| !message.is_empty())
                    .collect();
                if !filtered.is_empty() {
                    return filtered;
                }
            }
            Err(_) => {
                return vec![messages_json.trim().to_owned()];
            }
        }
    }

    if let Some(message) = &cli.message {
        return vec![message.clone()];
    }

    let mut stdin = String::new();
    match io::stdin().read_to_string(&mut stdin) {
        Ok(_) => {
            let trimmed = stdin.trim();
            if trimmed.is_empty() {
                vec!["请审计当前工作目录的安全风险，并给出高优先级问题清单。".to_owned()]
            } else {
                let parsed_json = serde_json::from_str::<Vec<String>>(trimmed);
                if let Ok(messages) = parsed_json {
                    let filtered: Vec<String> = messages
                        .into_iter()
                        .map(|message| message.trim().to_owned())
                        .filter(|message| !message.is_empty())
                        .collect();
                    if !filtered.is_empty() {
                        return filtered;
                    }
                }

                vec![trimmed.to_owned()]
            }
        }
        Err(_) => vec!["请审计当前工作目录的安全风险，并给出高优先级问题清单。".to_owned()],
    }
}

fn build_confirm_callback(
    mode: ConfirmMode,
    recorder: TraceRecorder,
) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |prompt: &str| {
        let (approved, source) = match mode {
            ConfirmMode::Allow => (true, "auto_allow"),
            ConfirmMode::Deny => (false, "auto_deny"),
            ConfirmMode::Ask => {
                eprint!("{} {} [y/N] ", "[确认]".yellow().bold(), prompt);
                let _ = io::Write::flush(&mut io::stderr());

                let mut input = String::new();
                let approved = match io::stdin().read_line(&mut input) {
                    Ok(_) => matches!(input.trim().to_lowercase().as_str(), "y" | "yes"),
                    Err(_) => false,
                };
                (approved, "stdin_prompt")
            }
        };

        recorder.record_confirm(prompt, approved, mode.as_str(), source);
        approved
    })
}

fn print_headless_response(output_format: OutputFormat, response: &HeadlessResponse) {
    match output_format {
        OutputFormat::Json => match serde_json::to_string_pretty(response) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("{}: 非交互结果序列化失败：{e}", "错误".red().bold());
                process::exit(1);
            }
        },
        OutputFormat::Text => match response {
            HeadlessResponse::Success {
                final_message,
                duration_ms,
                work_dir,
                confirm_mode,
                turns,
                session_management,
                ..
            } => {
                println!("{}", "secaudit chat 执行成功".green().bold());
                println!("工作目录：{work_dir}");
                println!("确认模式：{confirm_mode}");
                println!("轮次：{}", turns.len());
                println!("耗时：{duration_ms} ms");
                if let Some(info) = session_management {
                    println!("会话：{} ({})", info.project_key, info.status);
                    println!("会话文件：{}", info.session_path);
                }
                output::cli::print_separator();
                println!("{final_message}");
            }
            HeadlessResponse::Error {
                error,
                duration_ms,
                work_dir,
                confirm_mode,
                turns,
                session_management,
                ..
            } => {
                eprintln!("{}", "secaudit chat 执行失败".red().bold());
                eprintln!("工作目录：{work_dir}");
                eprintln!("确认模式：{confirm_mode}");
                eprintln!("轮次：{}", turns.len());
                eprintln!("耗时：{duration_ms} ms");
                if let Some(info) = session_management {
                    eprintln!("会话：{} ({})", info.project_key, info.status);
                    eprintln!("会话文件：{}", info.session_path);
                }
                eprintln!("错误：{error}");
            }
        },
    }
}

/// 根据文件扩展名检测编程语言
fn detect_language(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    for (extensions, lang) in EXTENSION_MAP {
        if extensions.contains(&ext) {
            return (*lang).into();
        }
    }

    UNKNOWN_LANGUAGE.into()
}

#[cfg(test)]
mod tests {
    use super::{ConfirmMode, Mode, OutputFormat, detect_language, resolve_chat_messages};
    use clap::ValueEnum;

    #[test]
    fn detect_language_uses_extension_map() {
        assert_eq!(detect_language("foo.rs"), "rust");
        assert_eq!(detect_language("foo.unknown"), "unknown");
    }

    #[test]
    fn mode_and_output_format_values_are_stable() {
        let modes = Mode::value_variants();
        assert!(modes.contains(&Mode::Auto));
        assert!(modes.contains(&Mode::Chat));

        let formats = OutputFormat::value_variants();
        assert!(formats.contains(&OutputFormat::Json));
        assert!(formats.contains(&OutputFormat::Text));
    }

    #[test]
    fn confirm_mode_values_are_stable() {
        let modes = ConfirmMode::value_variants();
        assert!(modes.contains(&ConfirmMode::Deny));
        assert!(modes.contains(&ConfirmMode::Allow));
        assert!(modes.contains(&ConfirmMode::Ask));
    }

    #[test]
    fn resolve_messages_prefers_json_list() {
        let cli = super::Cli {
            target: None,
            language: None,
            format: "text".to_owned(),
            output: None,
            mode: Mode::Chat,
            strategy: "react".to_owned(),
            message: None,
            messages_json: Some("[\"a\",\"b\"]".to_owned()),
            confirm_mode: ConfirmMode::Deny,
            output_format: OutputFormat::Json,
            session: None,
            list_sessions: false,
            archive_session: None,
        };

        assert_eq!(resolve_chat_messages(&cli), vec!["a", "b"]);
    }
}
