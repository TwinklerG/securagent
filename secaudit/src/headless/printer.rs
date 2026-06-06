//! headless 模式 CLI 输出适配。

use std::process;

use colored::Colorize;
use secaudit_conversation::SessionMetadata;
use serde::Serialize;

use crate::OutputFormat;
use crate::output;

use super::response::HeadlessResponse;

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

/// 按 CLI 输出格式打印会话列表。
pub(crate) fn print_session_list(output_format: OutputFormat, sessions: &[SessionMetadata]) {
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

/// 按 CLI 输出格式打印归档结果。
pub(crate) fn print_archived_session(output_format: OutputFormat, session: &SessionMetadata) {
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

/// 按 CLI 输出格式打印 headless chat 响应。
pub(crate) fn print_response(output_format: OutputFormat, response: &HeadlessResponse) {
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
                context,
            } => {
                println!("{}", "secaudit chat 执行成功".green().bold());
                println!("工作目录：{}", context.work_dir);
                println!("确认模式：{}", context.confirm_mode);
                println!("轮次：{}", context.turns.len());
                println!("耗时：{} ms", context.duration_ms);
                if let Some(info) = &context.session_management {
                    println!("会话：{} ({})", info.project_key, info.status);
                    println!("会话文件：{}", info.session_path);
                }
                output::cli::print_separator();
                println!("{final_message}");
            }
            HeadlessResponse::Error { error, context } => {
                eprintln!("{}", "secaudit chat 执行失败".red().bold());
                eprintln!("工作目录：{}", context.work_dir);
                eprintln!("确认模式：{}", context.confirm_mode);
                eprintln!("轮次：{}", context.turns.len());
                eprintln!("耗时：{} ms", context.duration_ms);
                if let Some(info) = &context.session_management {
                    eprintln!("会话：{} ({})", info.project_key, info.status);
                    eprintln!("会话文件：{}", info.session_path);
                }
                eprintln!("错误：{error}");
            }
        },
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
