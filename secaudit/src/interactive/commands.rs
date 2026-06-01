//! 交互命令解析。

/// 交互输入分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInput {
    /// 内置命令
    Command(Command),
    /// 自然语言消息
    Chat(String),
    /// 空输入
    Empty,
}

/// 内置命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    NewSession,
    ListSessions,
    SwitchSession { selector: String },
    Status,
    Usage,
    Context,
    Tools,
    Skills,
    Exit,
}

/// 解析用户输入。
#[must_use]
pub fn parse(input: &str) -> UserInput {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return UserInput::Empty;
    }

    let command = match trimmed {
        "/help" => Some(Command::Help),
        "/clear" | "/new" => Some(Command::NewSession),
        "/sessions" => Some(Command::ListSessions),
        "/status" => Some(Command::Status),
        "/usage" => Some(Command::Usage),
        "/context" => Some(Command::Context),
        "/tools" => Some(Command::Tools),
        "/skills" => Some(Command::Skills),
        "/exit" => Some(Command::Exit),
        _ => parse_session_command(trimmed),
    };

    if let Some(cmd) = command {
        UserInput::Command(cmd)
    } else {
        UserInput::Chat(trimmed.to_owned())
    }
}

fn parse_session_command(trimmed: &str) -> Option<Command> {
    let selector = trimmed.strip_prefix("/session ")?.trim();
    if selector.is_empty() {
        return None;
    }
    Some(Command::SwitchSession {
        selector: selector.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Command, UserInput, parse};

    #[test]
    fn parse_empty_input() {
        assert_eq!(parse("  \n\t"), UserInput::Empty);
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(parse("/help"), UserInput::Command(Command::Help));
    }

    #[test]
    fn parse_new_session_commands() {
        assert_eq!(parse("/new"), UserInput::Command(Command::NewSession));
        assert_eq!(parse("/clear"), UserInput::Command(Command::NewSession));
    }

    #[test]
    fn parse_session_commands() {
        assert_eq!(
            parse("/sessions"),
            UserInput::Command(Command::ListSessions)
        );
        assert_eq!(
            parse("/session abc-123"),
            UserInput::Command(Command::SwitchSession {
                selector: "abc-123".to_owned()
            })
        );
        assert_eq!(
            parse("/session 2"),
            UserInput::Command(Command::SwitchSession {
                selector: "2".to_owned()
            })
        );
    }

    #[test]
    fn parse_exit_command_with_spaces() {
        assert_eq!(parse("  /exit  "), UserInput::Command(Command::Exit));
    }

    #[test]
    fn parse_skills_command() {
        assert_eq!(parse("/skills"), UserInput::Command(Command::Skills));
    }

    #[test]
    fn parse_chat_message() {
        assert_eq!(
            parse("分析 src/main.rs 的安全风险"),
            UserInput::Chat("分析 src/main.rs 的安全风险".to_owned())
        );
    }

    #[test]
    fn parse_status_family_commands() {
        assert_eq!(parse("/status"), UserInput::Command(Command::Status));
        assert_eq!(parse("/usage"), UserInput::Command(Command::Usage));
        assert_eq!(parse("/context"), UserInput::Command(Command::Context));
    }
}
