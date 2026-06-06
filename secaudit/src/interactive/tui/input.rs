//! TUI 输入缓冲区与历史浏览状态。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const MAX_INPUT_LINES: usize = 6;
const INPUT_PLACEHOLDER: &str = "输入审计指令...";

#[derive(Default)]
pub(super) struct InputBuffer {
    lines: Vec<String>,
    pub(super) cursor_line: usize,
    cursor_col: usize,
}

impl InputBuffer {
    pub(super) fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        }
    }

    fn ensure_invariants(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len().saturating_sub(1);
        }

        let max_col = self.current_line_len();
        if self.cursor_col > max_col {
            self.cursor_col = max_col;
        }
    }

    fn current_line_len(&self) -> usize {
        self.lines
            .get(self.cursor_line)
            .map_or(0, |line| line.chars().count())
    }

    fn current_line_mut(&mut self) -> &mut String {
        self.ensure_invariants();
        &mut self.lines[self.cursor_line]
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lines.first().is_some_and(String::is_empty) && self.lines.len() == 1
    }

    pub(super) fn clear(&mut self) {
        self.lines.clear();
        self.lines.push(String::new());
        self.cursor_line = 0;
        self.cursor_col = 0;
    }

    pub(super) fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub(super) fn set_text(&mut self, text: &str) {
        let mut split: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
        if split.is_empty() {
            split.push(String::new());
        }
        self.lines = split;
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let col = self.cursor_col;
        let line = self.current_line_mut();
        let byte = char_to_byte_idx(line, col);
        line.insert(byte, ch);
        self.cursor_col += 1;
    }

    pub(super) fn backspace(&mut self) {
        self.ensure_invariants();

        if self.cursor_col > 0 {
            let cursor = self.cursor_col;
            let line = self.current_line_mut();
            let start = char_to_byte_idx(line, cursor - 1);
            let end = char_to_byte_idx(line, cursor);
            line.replace_range(start..end, "");
            self.cursor_col -= 1;
            return;
        }

        if self.cursor_line > 0 {
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.current_line_len();
            if let Some(prev) = self.lines.get_mut(self.cursor_line) {
                prev.push_str(&current);
            }
            self.cursor_col = prev_len;
        }
    }

    pub(super) fn newline(&mut self) {
        self.ensure_invariants();
        if self.lines.len() >= MAX_INPUT_LINES {
            return;
        }

        let cursor = self.cursor_col;
        let line = self.current_line_mut();
        let split = char_to_byte_idx(line, cursor);
        let tail = line.split_off(split);
        let insert_at = self.cursor_line + 1;
        self.lines.insert(insert_at, tail);
        self.cursor_line = insert_at;
        self.cursor_col = 0;
    }

    pub(super) fn move_left(&mut self) {
        self.ensure_invariants();

        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            return;
        }
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    pub(super) fn move_right(&mut self) {
        self.ensure_invariants();

        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
            return;
        }
        if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    pub(super) fn move_up(&mut self) {
        self.ensure_invariants();

        if self.cursor_line == 0 {
            return;
        }
        self.cursor_line -= 1;
        self.cursor_col = self.cursor_col.min(self.current_line_len());
    }

    pub(super) fn move_down(&mut self) {
        self.ensure_invariants();

        if self.cursor_line + 1 >= self.lines.len() {
            return;
        }
        self.cursor_line += 1;
        self.cursor_col = self.cursor_col.min(self.current_line_len());
    }

    pub(super) fn move_line_start(&mut self) {
        self.cursor_col = 0;
    }

    pub(super) fn move_line_end(&mut self) {
        self.ensure_invariants();
        self.cursor_col = self.current_line_len();
    }

    pub(super) fn cursor_display_col(&self) -> usize {
        let Some(line) = self.lines.get(self.cursor_line) else {
            return 0;
        };

        let byte_idx = char_to_byte_idx(line, self.cursor_col);
        line.get(..byte_idx).map_or(0, UnicodeWidthStr::width)
    }

    pub(super) fn visual_lines(&self) -> Vec<Line<'_>> {
        if self.lines.iter().all(String::is_empty) {
            return vec![Line::from(Span::styled(
                INPUT_PLACEHOLDER,
                Style::default().fg(Color::DarkGray),
            ))];
        }

        self.lines
            .iter()
            .map(|line| Line::from(line.as_str()))
            .collect::<Vec<_>>()
    }
}

fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    if char_idx == s.chars().count() {
        return s.len();
    }

    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(idx, _)| idx)
}

#[derive(Default)]
pub(super) struct History {
    entries: Vec<String>,
    browse_index: Option<usize>,
}

impl History {
    pub(super) fn push(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        if self.entries.last().is_some_and(|last| last == &entry) {
            self.browse_index = None;
            return;
        }
        self.entries.push(entry);
        self.browse_index = None;
    }

    pub(super) fn prev(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }

        let next = match self.browse_index {
            None => self.entries.len().saturating_sub(1),
            Some(0) => 0,
            Some(idx) => idx.saturating_sub(1),
        };
        self.browse_index = Some(next);
        self.entries.get(next).cloned()
    }

    pub(super) fn next(&mut self) -> Option<String> {
        match self.browse_index {
            None => None,
            Some(idx) if idx + 1 >= self.entries.len() => {
                self.browse_index = None;
                Some(String::new())
            }
            Some(idx) => {
                let next = idx + 1;
                self.browse_index = Some(next);
                self.entries.get(next).cloned()
            }
        }
    }

    pub(super) fn reset_browse(&mut self) {
        self.browse_index = None;
    }
}
