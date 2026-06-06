//! TUI 命令补全规则。

const COMMAND_CANDIDATES: [&str; 11] = [
    "/help",
    "/new",
    "/clear",
    "/sessions",
    "/session ",
    "/status",
    "/usage",
    "/context",
    "/tools",
    "/skills",
    "/exit",
];

pub(super) fn complete_command_input(text: &str) -> Option<&'static str> {
    if !text.starts_with('/') || text.contains('\n') {
        return None;
    }

    let current = text.trim();
    let mut candidates = COMMAND_CANDIDATES
        .iter()
        .copied()
        .filter(|cmd| cmd.starts_with(current))
        .collect::<Vec<_>>();

    candidates.sort_unstable();
    candidates.first().copied()
}

#[cfg(test)]
mod tests {
    use super::complete_command_input;

    #[test]
    fn completes_command_prefix_to_first_sorted_candidate() {
        assert_eq!(complete_command_input("/s"), Some("/session "));
    }

    #[test]
    fn completes_root_slash_using_sorted_candidates() {
        assert_eq!(complete_command_input("/"), Some("/clear"));
    }

    #[test]
    fn keeps_exact_command_when_no_longer_candidate_wins() {
        assert_eq!(complete_command_input("/status"), Some("/status"));
    }

    #[test]
    fn trims_command_before_matching() {
        assert_eq!(complete_command_input("  /s"), None);
        assert_eq!(complete_command_input("/session "), Some("/session "));
    }

    #[test]
    fn ignores_non_command_and_multiline_input() {
        assert_eq!(complete_command_input("hello"), None);
        assert_eq!(complete_command_input("/he\nx"), None);
    }

    #[test]
    fn ignores_unknown_command_prefix() {
        assert_eq!(complete_command_input("/unknown"), None);
    }
}
