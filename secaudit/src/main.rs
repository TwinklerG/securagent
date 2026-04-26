#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI 模式需要直接输出到终端"
)]

mod agent;
mod config;
mod error;
mod interactive;
mod llm;
mod output;
mod prompt;
mod server;
mod session;
mod tools;
mod trajectory;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use clap::Parser;
use colored::Colorize;

use agent::strategy;

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

    /// 运行模式：auto（默认）、web
    #[arg(short, long, default_value = MODE_AUTO)]
    mode: String,

    /// 推理策略：react（默认）、reflexion（仅单文件模式）
    #[arg(short, long, default_value = strategy::STRATEGY_REACT)]
    strategy: String,

    /// Web 服务器端口（仅 web 模式）
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    port: u16,
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
/// Web 运行模式标识
const MODE_WEB: &str = "web";

/// Web 服务器默认端口
const DEFAULT_PORT: u16 = 8080;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    // 加载配置
    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: {e}", "配置错误".red().bold());
            process::exit(1);
        }
    };

    if cli.mode == MODE_WEB {
        server::start(cli.port, config).await;
    } else if let Some(target) = &cli.target {
        run_single_file(&cli, config, target).await;
    } else {
        run_interactive(config).await;
    }
}

/// 交互模式：进入 REPL 循环
async fn run_interactive(config: config::Config) {
    let work_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    interactive::run(config, work_dir).await;
}

/// 单文件审计模式
async fn run_single_file(cli: &Cli, mut config: config::Config, target: &str) {
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
    let work_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut agent = agent::Agent::new(config, work_dir, Arc::new(|_| true));
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
                let sample =
                    trajectory::to_multi_turn_sample(&report.messages, &report.findings, target);
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
