// 交互式 CLI REPL 模块

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use colored::Colorize;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::agent::Agent;
use crate::config::Config;
use crate::output;
use crate::session::Session;

/// REPL 提示符
const PROMPT: &str = "secaudit> ";

/// 退出命令
const CMD_EXIT: &str = "/exit";
/// 帮助命令
const CMD_HELP: &str = "/help";
/// 清空历史命令
const CMD_CLEAR: &str = "/clear";
/// 状态命令
const CMD_STATUS: &str = "/status";
/// 工具列表命令
const CMD_TOOLS: &str = "/tools";

/// 欢迎消息
const WELCOME_MSG: &str = "secaudit -- 安全代码审计 Agent（交互模式）";
/// 帮助提示
const HELP_HINT: &str = "输入安全审计指令，或键入 /help 查看帮助。";

/// 帮助信息
const HELP_TEXT: &str = "\
可用命令：
  /help    显示此帮助信息
  /clear   清空对话历史
  /status  显示当前会话状态
  /tools   列出可用工具
  /exit    退出

使用方式：
  直接输入自然语言指令，Agent 会自主使用工具完成任务。
  例如：
  - 审计当前项目的安全性
  - 查找所有 SQL 注入漏洞
  - 读取 src/main.rs 并分析安全问题
  - 检查项目依赖是否有已知漏洞";

/// 创建 CLI 模式的确认回调（通过 stdin 读取用户输入）
fn cli_confirm() -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(|prompt: &str| {
        eprint!("{} {} [y/N] ", "[确认]".yellow().bold(), prompt);
        let _ = io::stderr().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }

        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    })
}

/// 启动交互式 REPL
pub async fn run(config: Config, work_dir: PathBuf) {
    println!("{}", WELCOME_MSG.bold());
    println!("工作目录：{}", work_dir.display().to_string().cyan());
    println!("{HELP_HINT}");
    output::cli::print_separator();

    // 创建 Agent
    let mut agent = Agent::new(config, work_dir.clone(), cli_confirm());
    agent.on_state_change(output::cli::print_state);
    agent.on_think(output::cli::print_thinking);
    agent.on_tool_call(output::cli::print_tool_call);
    agent.on_tool_result(output::cli::print_tool_result);

    // 创建会话
    let mut session = Session::new(work_dir);

    // 初始化 readline
    let mut rl = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("{}: 初始化 readline 失败：{e}", "错误".red().bold());
            return;
        }
    };

    // REPL 循环
    loop {
        let readline = rl.readline(PROMPT);
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(input);

                // 处理会话命令
                if handle_command(input, &agent, &mut session) {
                    continue;
                }
                if input == CMD_EXIT {
                    println!("{}", "再见！".green());
                    break;
                }

                // 发送给 Agent
                output::cli::print_separator();
                match agent.chat(&mut session, input).await {
                    Ok(response) => {
                        output::cli::print_separator();
                        println!("{response}");
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", "Agent 错误".red().bold());
                    }
                }
                println!();
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
                println!("{}", "再见！".green());
                break;
            }
            Err(e) => {
                eprintln!("{}: {e}", "输入错误".red().bold());
                break;
            }
        }
    }
}

/// 处理会话命令，返回 true 表示已处理（不需要发送给 Agent）
fn handle_command(input: &str, agent: &Agent, session: &mut Session) -> bool {
    match input {
        CMD_HELP => {
            println!("{HELP_TEXT}");
            true
        }
        CMD_CLEAR => {
            session.clear();
            println!("{}", "对话历史已清空。".green());
            true
        }
        CMD_STATUS => {
            println!("工作目录：{}", agent.work_dir().display());
            println!("对话轮次：{}", session.messages().len());
            true
        }
        CMD_TOOLS => {
            println!("{}", "可用工具：".bold());
            for name in agent.tool_names() {
                println!("  - {name}");
            }
            true
        }
        _ => false,
    }
}
