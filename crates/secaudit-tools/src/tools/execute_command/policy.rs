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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandPolicyConfig {
    pub allowlist: Vec<String>,
    pub blocklist: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CommandPolicy {
    config: CommandPolicyConfig,
}

impl CommandPolicy {
    pub(super) fn new(config: CommandPolicyConfig) -> Self {
        Self { config }
    }

    /// 判断命令是否允许执行、需要确认或必须禁止。
    pub(super) fn decide(&self, command: &str) -> CommandDecision {
        if self.is_blocked(command) {
            CommandDecision::Block
        } else if self.is_safe(command) {
            CommandDecision::Allow
        } else {
            CommandDecision::RequireConfirmation
        }
    }

    fn is_blocked(&self, command: &str) -> bool {
        is_blocked_by_builtin_policy(command)
            || self
                .config
                .blocklist
                .iter()
                .any(|rule| command_matches_user_rule(command, rule))
    }

    fn is_safe(&self, command: &str) -> bool {
        is_safe_by_builtin_policy(command)
            || self
                .config
                .allowlist
                .iter()
                .any(|rule| command_matches_user_rule(command, rule))
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

const BLOCKED_COMMAND_NAMES: &[&str] = &["mkfs", "dd", "shutdown", "reboot", "poweroff", "halt"];

const BLOCKED_PATTERNS: &[&str] = &["rm -rf /", ":(){:|:&};:", ">/dev/"];

/// 检查命令是否命中黑名单。
fn is_blocked_by_builtin_policy(command: &str) -> bool {
    let lowered = command.trim().to_ascii_lowercase();
    if BLOCKED_PATTERNS
        .iter()
        .any(|blocked| lowered.contains(blocked))
    {
        return true;
    }

    lowered.split_whitespace().any(|token| {
        let token = command_token_basename(token);
        BLOCKED_COMMAND_NAMES
            .iter()
            .any(|name| token_matches(token, name))
    })
}

/// 检查命令是否在安全白名单中。
fn is_safe_by_builtin_policy(command: &str) -> bool {
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

fn command_matches_user_rule(command: &str, rule: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    let rule = rule.trim().to_ascii_lowercase();
    if rule.is_empty() {
        return false;
    }

    if rule.contains(char::is_whitespace) {
        return has_command_prefix(&command, &rule) || command.contains(&rule);
    }

    command.split_whitespace().any(|token| {
        let token = command_token_basename(token);
        token_matches(token, &rule)
    })
}

fn token_matches(token: &str, name: &str) -> bool {
    token == name
        || token
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn command_token_basename(token: &str) -> &str {
    let token = token.trim_matches(|ch: char| {
        matches!(ch, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\'' | ',')
    });
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

#[cfg(test)]
mod tests {
    use super::{CommandDecision, CommandPolicy};

    #[test]
    fn safe_commands_are_allowed() {
        let policy = CommandPolicy::default();

        assert_eq!(policy.decide("ls -la"), CommandDecision::Allow);
        assert_eq!(policy.decide("git log --oneline"), CommandDecision::Allow);
        assert_eq!(
            policy.decide("cargo clippy -- -D warnings"),
            CommandDecision::Allow
        );
        assert_eq!(
            policy.decide("python -m py_compile foo.py"),
            CommandDecision::Allow
        );
        assert_eq!(policy.decide("rg pattern ."), CommandDecision::Allow);
    }

    #[test]
    fn unknown_commands_require_confirmation() {
        let policy = CommandPolicy::default();

        assert_eq!(
            policy.decide("git logmalicious"),
            CommandDecision::RequireConfirmation
        );
        assert_eq!(
            policy.decide("python script.py"),
            CommandDecision::RequireConfirmation
        );
    }

    #[test]
    fn blocked_commands_are_rejected_even_when_prefixed() {
        let policy = CommandPolicy::default();

        assert_eq!(policy.decide("rm -rf /"), CommandDecision::Block);
        assert_eq!(
            policy.decide("sudo mkfs.ext4 /dev/sda"),
            CommandDecision::Block
        );
        assert_eq!(policy.decide("shutdown -h now"), CommandDecision::Block);
    }
}
