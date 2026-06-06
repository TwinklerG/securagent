//! headless chat 输入消息解析。

const DEFAULT_CHAT_MESSAGE: &str = "请审计当前工作目录的安全风险，并给出高优先级问题清单。";

/// 解析 headless chat 输入消息。
///
/// 优先级：`--messages-json` > `--message` > stdin > 默认审计请求。
pub(crate) fn resolve_chat_messages<F>(
    messages_json: Option<&str>,
    message: Option<&str>,
    read_stdin: F,
) -> Vec<String>
where
    F: FnOnce() -> Option<String>,
{
    if let Some(messages_json) = messages_json
        && let Some(messages) = parse_message_list(messages_json)
    {
        return messages;
    }

    if let Some(message) = message {
        return vec![message.to_owned()];
    }

    let Some(stdin) = read_stdin() else {
        return default_chat_messages();
    };
    let trimmed = stdin.trim();
    if trimmed.is_empty() {
        return default_chat_messages();
    }

    parse_message_list(trimmed).unwrap_or_else(|| vec![trimmed.to_owned()])
}

fn parse_message_list(raw: &str) -> Option<Vec<String>> {
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(messages) => non_empty_messages(messages),
        Err(_) => Some(vec![raw.trim().to_owned()]),
    }
}

fn non_empty_messages(messages: Vec<String>) -> Option<Vec<String>> {
    let filtered = messages
        .into_iter()
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>();

    (!filtered.is_empty()).then_some(filtered)
}

fn default_chat_messages() -> Vec<String> {
    vec![DEFAULT_CHAT_MESSAGE.to_owned()]
}

#[cfg(test)]
mod tests {
    use super::resolve_chat_messages;

    #[test]
    fn resolve_messages_prefers_json_list() {
        let messages =
            resolve_chat_messages(Some("[\"a\",\"b\"]"), None, || Some("ignored".to_owned()));

        assert_eq!(messages, vec!["a", "b"]);
    }

    #[test]
    fn resolve_messages_falls_back_to_raw_invalid_json_arg() {
        let messages = resolve_chat_messages(Some("not json"), None, || None);

        assert_eq!(messages, vec!["not json"]);
    }

    #[test]
    fn resolve_messages_reads_stdin_json_when_no_arg_exists() {
        let messages = resolve_chat_messages(None, None, || {
            Some("[\" first \",\"\", \"second\"]".to_owned())
        });

        assert_eq!(messages, vec!["first", "second"]);
    }
}
