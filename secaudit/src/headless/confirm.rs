//! headless 模式确认策略。

use std::io::{self, Write};
use std::sync::Arc;

use colored::Colorize;

use crate::ConfirmMode;

use super::trace::TraceRecorder;

const MSG_USER_DENIED: &str = "用户拒绝执行该命令";
const CONFIRM_SOURCE_AUTO_ALLOW: &str = "auto_allow";
const CONFIRM_SOURCE_AUTO_DENY: &str = "auto_deny";
const CONFIRM_SOURCE_STDIN_PROMPT: &str = "stdin_prompt";

/// 确认模式稳定字符串。
#[must_use]
pub(crate) const fn confirm_mode_name(mode: ConfirmMode) -> &'static str {
    match mode {
        ConfirmMode::Deny => "deny",
        ConfirmMode::Allow => "allow",
        ConfirmMode::Ask => "ask",
    }
}

/// 构造 headless 模式的确认回调。
pub(crate) fn build_confirm_callback(
    mode: ConfirmMode,
    recorder: TraceRecorder,
) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(move |prompt: &str| {
        let (approved, source) = match mode {
            ConfirmMode::Allow => (true, CONFIRM_SOURCE_AUTO_ALLOW),
            ConfirmMode::Deny => (false, CONFIRM_SOURCE_AUTO_DENY),
            ConfirmMode::Ask => ask_user_confirm(prompt),
        };

        recorder.record_confirm(prompt, approved, confirm_mode_name(mode), source);
        approved
    })
}

/// 判断错误是否来自用户拒绝确认。
#[must_use]
pub(crate) fn is_user_denied_error(error: &str) -> bool {
    error.contains(MSG_USER_DENIED)
}

fn ask_user_confirm(prompt: &str) -> (bool, &'static str) {
    eprint!("{} {} [y/N] ", "[确认]".yellow().bold(), prompt);
    let _ = Write::flush(&mut io::stderr());

    let mut input = String::new();
    let approved = match io::stdin().read_line(&mut input) {
        Ok(_) => matches!(input.trim().to_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    };
    (approved, CONFIRM_SOURCE_STDIN_PROMPT)
}

#[cfg(test)]
mod tests {
    use crate::ConfirmMode;

    use super::{build_confirm_callback, confirm_mode_name, is_user_denied_error};
    use crate::headless::TraceRecorder;

    #[test]
    fn confirm_mode_names_are_stable() {
        assert_eq!(confirm_mode_name(ConfirmMode::Deny), "deny");
        assert_eq!(confirm_mode_name(ConfirmMode::Allow), "allow");
        assert_eq!(confirm_mode_name(ConfirmMode::Ask), "ask");
    }

    #[test]
    fn auto_allow_callback_records_confirmation() {
        let recorder = TraceRecorder::new();
        let confirm = build_confirm_callback(ConfirmMode::Allow, recorder.clone());

        assert!(confirm("允许执行命令吗"));

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.confirm_events.len(), 1);
        assert_eq!(snapshot.confirm_events[0].mode, "allow");
        assert_eq!(snapshot.confirm_events[0].source, "auto_allow");
        assert!(snapshot.confirm_events[0].approved);
    }

    #[test]
    fn auto_deny_callback_records_confirmation() {
        let recorder = TraceRecorder::new();
        let confirm = build_confirm_callback(ConfirmMode::Deny, recorder.clone());

        assert!(!confirm("允许执行命令吗"));

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.confirm_events.len(), 1);
        assert_eq!(snapshot.confirm_events[0].mode, "deny");
        assert_eq!(snapshot.confirm_events[0].source, "auto_deny");
        assert!(!snapshot.confirm_events[0].approved);
    }

    #[test]
    fn user_denied_error_is_detected_by_message() {
        assert!(is_user_denied_error("工具错误：用户拒绝执行该命令"));
        assert!(!is_user_denied_error("命令执行超时"));
    }
}
