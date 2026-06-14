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
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use colored::Colorize;
use secaudit_agent::strategy;
use secaudit_agent::{Agent, llm, to_multi_turn_sample};
use secaudit_conversation::ConversationService;
use secaudit_core::Config;
use secaudit_storage::LOGS_DIR;
use secaudit_storage::RUNTIME_DIR;
use tracing_appender::non_blocking;
use tracing_appender::rolling::hourly;

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

#[tokio::main]
async fn main() {
    let file_appender = hourly(
        Config::default_log_path().unwrap_or(PathBuf::from(RUNTIME_DIR).join(LOGS_DIR)),
        "secaudit-tui.log",
    );
    let (non_blocking, _guard) = non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(non_blocking)
        .init();

    let cli = Cli::parse();

    if cli.mode == Mode::Chat || cli.list_sessions || cli.archive_session.is_some() {
        if handle_headless_session_management(&cli) {
            return;
        }

        let config = load_config_or_exit();
        run_headless_chat(&cli, config).await;
    } else if let Some(target) = &cli.target {
        let config = load_config_or_exit();
        run_single_file(&cli, config, target).await;
    } else {
        let config = load_config_or_exit();
        run_interactive(config).await;
    }
}

fn load_config_or_exit() -> Config {
    match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}: {error}", "配置错误".red().bold());
            process::exit(1);
        }
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
#[expect(clippy::too_many_lines)]
async fn run_headless_chat(cli: &Cli, config: Config) {
    let work_dir = current_work_dir();
    let work_dir_display = work_dir.display().to_string();
    let inputs = headless::resolve_chat_messages(
        cli.messages_json.as_deref(),
        cli.message.as_deref(),
        read_stdin_to_string,
    );
    let conversation = headless::open_conversation_service_or_exit();

    let recorder = TraceRecorder::new();
    let confirm_mode = cli.confirm_mode;
    let confirm_mode_name = headless::confirm_mode_name(confirm_mode).to_owned();
    let confirm = headless::build_confirm_callback(confirm_mode, recorder.clone());

    let llm_client = llm::create_client(&config);
    let mut agent = Agent::new(config, work_dir.clone(), confirm);
    recorder.attach(&mut agent);
    let memory = match conversation.create_memory_store(&work_dir) {
        Ok(store) => Some(store),
        Err(e) => {
            eprintln!("[memory] 初始化失败: {e}");
            None
        }
    };
    let mut managed_session =
        match headless::load_or_start_session(&conversation, &work_dir, cli.session.as_deref()) {
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
            .chat(
                &mut agent,
                &mut managed_session,
                user_message,
                memory.as_ref(),
                Some(&llm_client),
            )
            .await
        {
            Ok(outcome) => {
                let duration_ms = turn_start.elapsed().as_millis() as u64;
                let message = outcome.response;
                if let Some(compression) = outcome.compression {
                    eprintln!(
                        "{}: 已压缩较早的 {} 条消息，{}% -> {}% context。",
                        "上下文压缩".cyan().bold(),
                        compression.covered_message_count,
                        compression.before_used_percent,
                        compression.after_used_percent
                    );
                }
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

    // 会话结束：最终化 L2 + L3
    if let (Some(mem), true) = (memory.as_ref(), failure.is_none())
        && let Err(e) = ConversationService::finalize_session(mem, managed_session.id())
    {
        eprintln!("[memory] 会话总结失败: {e}");
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

    headless::print_response(cli.output_format, &response);

    if let Some(error) = failure {
        if headless::is_user_denied_error(&error) {
            process::exit(EXIT_USER_DENIED);
        }
        process::exit(1);
    }
}

fn handle_headless_session_management(cli: &Cli) -> bool {
    let work_dir = current_work_dir();
    headless::handle_session_management_request(
        cli.list_sessions,
        cli.archive_session.as_deref(),
        cli.output_format,
        &work_dir,
    )
}

fn current_work_dir() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn read_stdin_to_string() -> Option<String> {
    let mut stdin = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut stdin)
        .ok()
        .map(|_| stdin)
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
    use super::{ConfirmMode, Mode, OutputFormat, detect_language};
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
}
