//! `execute_command` 的命令安全策略。

/// 命令安全判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandDecision {
    /// 命令命中禁止策略。
    Block,
    /// 命令可自动放行。
    Allow,
    /// 命令未知，需要用户确认。
    RequireConfirmation,
}

/// 轻量命令策略。
///
/// 当前仍基于 shell 文本做前缀判定；后续若扩展到更细粒度的参数策略，
/// 应优先在这里替换为专门解析器，而不是让工具执行流程继续膨胀。
pub(super) struct CommandPolicy;

impl CommandPolicy {
    /// 判断命令是否允许执行、需要确认或必须禁止。
    pub(super) fn decide(command: &str) -> CommandDecision {
        if is_blocked(command) {
            CommandDecision::Block
        } else if is_safe(command) {
            CommandDecision::Allow
        } else {
            CommandDecision::RequireConfirmation
        }
    }
}

// —— 安全白名单（自动放行） ——

const SAFE_COMMANDS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "grep",
    "find",
    "file",
    "wc",
    "tree",
    "git log",
    "git diff",
    "git show",
    "git status",
    "cargo check",
    "cargo clippy",
    "cargo audit",
    "npm audit",
    "python -m py_compile",
    "semgrep",
    "rg",
    "fd",
];

// —— 危险黑名单（一律禁止） ——

const BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "mkfs",
    "dd",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    ":(){:|:&};:",
];

/// 检查命令是否命中黑名单。
fn is_blocked(command: &str) -> bool {
    let trimmed = command.trim();
    BLOCKED_COMMANDS
        .iter()
        .any(|blocked| trimmed.contains(blocked))
}

/// 检查命令是否在安全白名单中。
fn is_safe(command: &str) -> bool {
    let trimmed = command.trim();
    let first_word = trimmed.split_whitespace().next().unwrap_or_default();

    SAFE_COMMANDS.iter().any(|safe| {
        if safe.contains(' ') {
            has_command_prefix(trimmed, safe)
        } else {
            first_word == *safe
        }
    })
}

fn has_command_prefix(command: &str, safe_prefix: &str) -> bool {
    let Some(rest) = command.strip_prefix(safe_prefix) else {
        return false;
    };

    rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::{CommandDecision, CommandPolicy};

    #[test]
    fn safe_commands_are_allowed() {
        assert_eq!(CommandPolicy::decide("ls -la"), CommandDecision::Allow);
        assert_eq!(
            CommandPolicy::decide("git log --oneline"),
            CommandDecision::Allow
        );
        assert_eq!(
            CommandPolicy::decide("cargo clippy -- -D warnings"),
            CommandDecision::Allow
        );
        assert_eq!(
            CommandPolicy::decide("python -m py_compile foo.py"),
            CommandDecision::Allow
        );
        assert_eq!(
            CommandPolicy::decide("rg pattern ."),
            CommandDecision::Allow
        );
    }

    #[test]
    fn unknown_commands_require_confirmation() {
        assert_eq!(
            CommandPolicy::decide("curl http://example.com"),
            CommandDecision::RequireConfirmation
        );
        assert_eq!(
            CommandPolicy::decide("wget something"),
            CommandDecision::RequireConfirmation
        );
        assert_eq!(
            CommandPolicy::decide("git logmalicious"),
            CommandDecision::RequireConfirmation
        );
    }

    #[test]
    fn blocked_commands_are_rejected_even_when_prefixed() {
        assert_eq!(CommandPolicy::decide("rm -rf /"), CommandDecision::Block);
        assert_eq!(
            CommandPolicy::decide("sudo mkfs.ext4 /dev/sda"),
            CommandDecision::Block
        );
        assert_eq!(
            CommandPolicy::decide("shutdown -h now"),
            CommandDecision::Block
        );
    }
}
