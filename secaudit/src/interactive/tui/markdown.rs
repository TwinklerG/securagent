use std::iter::repeat_n;
use std::mem;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd, TextMergeStream,
};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const EMPTY_LINE: &str = "";
const UNORDERED_LIST_MARKER: &str = "- ";
const TASK_DONE_MARKER: &str = "[x] ";
const TASK_TODO_MARKER: &str = "[ ] ";
const LIST_INDENT_SPACES: usize = 2;
const CODE_FENCE: &str = "```";
const HORIZONTAL_RULE: &str = "────────────────";
const INLINE_CODE_LEFT: &str = "`";
const INLINE_CODE_RIGHT: &str = "`";
const TABLE_NEWLINE_REPLACEMENT: &str = " ";
const TABLE_MIN_COLUMN_WIDTH: usize = 1;
const TABLE_CELL_PADDING: usize = 1;
const HEADING_MARK: char = '#';
const SPACE_CHAR: char = ' ';
const TABLE_HORIZONTAL: char = '─';
const TABLE_VERTICAL: char = '│';
const TABLE_TOP_LEFT: char = '┌';
const TABLE_TOP_MID: char = '┬';
const TABLE_TOP_RIGHT: char = '┐';
const TABLE_MID_LEFT: char = '├';
const TABLE_MID_MID: char = '┼';
const TABLE_MID_RIGHT: char = '┤';
const TABLE_BOTTOM_LEFT: char = '└';
const TABLE_BOTTOM_MID: char = '┴';
const TABLE_BOTTOM_RIGHT: char = '┘';

/// 将 Markdown 渲染为 Ratatui 文本行，供聊天区复用。
#[must_use]
pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    let mut renderer = MarkdownRenderer::default();
    renderer.render(markdown)
}

#[derive(Clone, Copy, Default)]
struct ListState {
    next_index: Option<u64>,
}

#[derive(Default)]
struct MarkdownRenderer {
    lines: Vec<Line<'static>>,
    current_line: String,
    list_stack: Vec<ListState>,
    item_prefix_width_stack: Vec<usize>,
    in_code_block: bool,
    code_line_buffer: String,
    table: Option<TableBuilder>,
}

impl MarkdownRenderer {
    fn render(&mut self, markdown: &str) -> Vec<Line<'static>> {
        let parser = Parser::new_ext(markdown, markdown_options());
        for event in TextMergeStream::new(parser) {
            self.handle_event(event);
        }
        self.finalize()
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.handle_start(tag),
            Event::End(tag_end) => self.handle_end(tag_end),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.append_text(text.as_ref());
            }
            Event::Code(text) => self.append_inline_code(text.as_ref()),
            Event::SoftBreak | Event::HardBreak => self.append_line_break(),
            Event::Rule => self.render_rule(),
            Event::TaskListMarker(checked) => {
                self.append_text(if checked {
                    TASK_DONE_MARKER
                } else {
                    TASK_TODO_MARKER
                });
            }
            Event::FootnoteReference(label) => self.append_text(format!("[^{label}]").as_str()),
            Event::InlineMath(content) => self.append_text(format!("${content}$").as_str()),
            Event::DisplayMath(content) => {
                self.separate_block();
                self.push_plain_line(format!("$${content}$$"));
                self.push_blank_line_once();
            }
        }
    }

    fn handle_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => {
                self.append_text("> ");
            }
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(start_index) => self.start_list(start_index),
            Tag::Item => self.start_item(),
            Tag::FootnoteDefinition(label) => {
                self.separate_block();
                self.append_text(format!("[^{label}]: ").as_str());
            }
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.start_head();
                }
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.start_row();
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.start_cell();
                }
            }
            Tag::Paragraph
            | Tag::HtmlBlock
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Image { .. }
            | Tag::Link { .. } => {}
        }
    }

    fn handle_end(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => {
                let _ = self.list_stack.pop();
            }
            TagEnd::Item => self.end_item(),
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.end_head();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.end_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.end_cell();
                }
            }
            TagEnd::Paragraph | TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition => {
                self.end_paragraph();
            }
            TagEnd::HtmlBlock
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    fn finalize(&mut self) -> Vec<Line<'static>> {
        if self.in_code_block {
            self.end_code_block();
        }
        self.flush_current_line();
        self.trim_trailing_blank_lines();

        if self.lines.is_empty() {
            return vec![Line::from(String::new())];
        }
        mem::take(&mut self.lines)
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.separate_block();
        let level_num = heading_level_to_usize(level);
        self.current_line
            .push_str(repeat_char(HEADING_MARK, level_num).as_str());
        self.current_line.push(SPACE_CHAR);
    }

    fn end_heading(&mut self) {
        self.flush_current_line();
        self.push_blank_line_once();
    }

    fn start_list(&mut self, start_index: Option<u64>) {
        self.list_stack.push(ListState {
            next_index: start_index,
        });
    }

    fn start_item(&mut self) {
        self.flush_current_line();
        let depth = self.list_stack.len().saturating_sub(1);
        let indent_width = depth.saturating_mul(LIST_INDENT_SPACES);
        let indent = repeat_char(SPACE_CHAR, indent_width);
        let marker = self.next_list_marker();
        let prefix = format!("{indent}{marker}");
        self.item_prefix_width_stack
            .push(UnicodeWidthStr::width(prefix.as_str()));
        self.current_line.push_str(prefix.as_str());
    }

    fn end_item(&mut self) {
        self.flush_current_line();
        let _ = self.item_prefix_width_stack.pop();
    }

    fn next_list_marker(&mut self) -> String {
        if let Some(state) = self.list_stack.last_mut()
            && let Some(index) = state.next_index
        {
            state.next_index = Some(index.saturating_add(1));
            return format!("{index}. ");
        }
        UNORDERED_LIST_MARKER.to_owned()
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.separate_block();

        let fence = match kind {
            CodeBlockKind::Fenced(lang) => {
                let language = lang.trim();
                if language.is_empty() {
                    CODE_FENCE.to_owned()
                } else {
                    format!("{CODE_FENCE}{language}")
                }
            }
            CodeBlockKind::Indented => CODE_FENCE.to_owned(),
        };

        self.push_styled_line(fence, code_block_style());
        self.in_code_block = true;
        self.code_line_buffer.clear();
    }

    fn end_code_block(&mut self) {
        if !self.code_line_buffer.is_empty() {
            self.flush_code_line();
        }
        self.push_styled_line(CODE_FENCE.to_owned(), code_block_style());
        self.in_code_block = false;
        self.push_blank_line_once();
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.separate_block();
        self.table = Some(TableBuilder::new(alignments));
    }

    fn end_table(&mut self) {
        if let Some(table) = self.table.take() {
            for line in table.render_lines() {
                self.lines.push(line);
            }
            self.push_blank_line_once();
        }
    }

    fn end_paragraph(&mut self) {
        self.flush_current_line();
        if self.item_prefix_width_stack.is_empty() {
            self.push_blank_line_once();
        }
    }

    fn append_text(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut()
            && table.in_cell()
        {
            table.push_cell_text(text);
            return;
        }

        if self.in_code_block {
            self.append_code_text(text);
            return;
        }

        self.current_line.push_str(text);
    }

    fn append_inline_code(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut()
            && table.in_cell()
        {
            table.push_cell_text(text);
            return;
        }

        if self.in_code_block {
            self.append_code_text(text);
            return;
        }

        self.current_line.push_str(INLINE_CODE_LEFT);
        self.current_line.push_str(text);
        self.current_line.push_str(INLINE_CODE_RIGHT);
    }

    fn append_code_text(&mut self, text: &str) {
        for chunk in text.split_inclusive('\n') {
            if let Some(prefix) = chunk.strip_suffix('\n') {
                self.code_line_buffer.push_str(prefix);
                self.flush_code_line();
            } else {
                self.code_line_buffer.push_str(chunk);
            }
        }
    }

    fn append_line_break(&mut self) {
        if let Some(table) = self.table.as_mut()
            && table.in_cell()
        {
            table.push_cell_text(TABLE_NEWLINE_REPLACEMENT);
            return;
        }

        if self.in_code_block {
            self.flush_code_line();
            return;
        }

        self.flush_current_line();
        if let Some(prefix_width) = self.item_prefix_width_stack.last().copied() {
            self.current_line = repeat_char(SPACE_CHAR, prefix_width);
        }
    }

    fn render_rule(&mut self) {
        self.separate_block();
        self.push_plain_line(HORIZONTAL_RULE.to_owned());
        self.push_blank_line_once();
    }

    fn flush_current_line(&mut self) {
        if self.current_line.trim().is_empty() {
            self.current_line.clear();
            return;
        }
        let line = mem::take(&mut self.current_line);
        self.lines.push(Line::from(line));
    }

    fn flush_code_line(&mut self) {
        let line = mem::take(&mut self.code_line_buffer);
        self.push_styled_line(line, code_block_style());
    }

    fn separate_block(&mut self) {
        self.flush_current_line();
        if !self.lines.is_empty() {
            self.push_blank_line_once();
        }
    }

    fn push_blank_line_once(&mut self) {
        if self.lines.last().is_some_and(line_is_blank) {
            return;
        }
        self.lines.push(Line::from(String::new()));
    }

    fn trim_trailing_blank_lines(&mut self) {
        while self.lines.last().is_some_and(line_is_blank) {
            let _ = self.lines.pop();
        }
    }

    fn push_plain_line(&mut self, text: String) {
        self.lines.push(Line::from(text));
    }

    fn push_styled_line(&mut self, text: String, style: Style) {
        self.lines
            .push(Line::from(Span::styled(text, style)).style(style));
    }
}

#[derive(Default)]
struct TableBuilder {
    alignments: Vec<Alignment>,
    header_rows: Vec<Vec<String>>,
    body_rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
    row_open: bool,
    in_cell: bool,
}

impl TableBuilder {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            ..Self::default()
        }
    }

    fn start_head(&mut self) {
        self.in_head = true;
        self.start_row();
    }

    fn end_head(&mut self) {
        self.end_row();
        self.in_head = false;
    }

    fn start_row(&mut self) {
        if self.row_open {
            return;
        }
        self.current_row.clear();
        self.row_open = true;
    }

    fn end_row(&mut self) {
        if !self.row_open {
            return;
        }
        self.row_open = false;
        if self.current_row.is_empty() {
            return;
        }
        let row = mem::take(&mut self.current_row);
        if self.in_head {
            self.header_rows.push(row);
        } else {
            self.body_rows.push(row);
        }
    }

    fn start_cell(&mut self) {
        self.start_row();
        self.current_cell.clear();
        self.in_cell = true;
    }

    fn end_cell(&mut self) {
        if !self.in_cell {
            return;
        }
        let cell = normalize_table_cell(self.current_cell.as_str());
        self.current_row.push(cell);
        self.current_cell.clear();
        self.in_cell = false;
    }

    fn push_cell_text(&mut self, text: &str) {
        if self.in_cell {
            self.current_cell.push_str(text);
        }
    }

    fn in_cell(&self) -> bool {
        self.in_cell
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let column_count = self.column_count();
        if column_count == 0 {
            return Vec::new();
        }

        let widths = self.column_widths(column_count);
        let mut lines = Vec::new();
        lines.push(styled_table_border_line(build_table_border(
            TABLE_TOP_LEFT,
            TABLE_TOP_MID,
            TABLE_TOP_RIGHT,
            widths.as_slice(),
        )));

        for row in &self.header_rows {
            lines.push(Line::from(build_table_row(
                row.as_slice(),
                widths.as_slice(),
                self.alignments.as_slice(),
            )));
        }

        if !self.header_rows.is_empty() {
            lines.push(styled_table_border_line(build_table_border(
                TABLE_MID_LEFT,
                TABLE_MID_MID,
                TABLE_MID_RIGHT,
                widths.as_slice(),
            )));
        }

        for row in &self.body_rows {
            lines.push(Line::from(build_table_row(
                row.as_slice(),
                widths.as_slice(),
                self.alignments.as_slice(),
            )));
        }

        lines.push(styled_table_border_line(build_table_border(
            TABLE_BOTTOM_LEFT,
            TABLE_BOTTOM_MID,
            TABLE_BOTTOM_RIGHT,
            widths.as_slice(),
        )));
        lines
    }

    fn column_count(&self) -> usize {
        let mut count = self.alignments.len();
        for row in self.header_rows.iter().chain(&self.body_rows) {
            count = count.max(row.len());
        }
        count
    }

    fn column_widths(&self, column_count: usize) -> Vec<usize> {
        let mut widths = vec![TABLE_MIN_COLUMN_WIDTH; column_count];
        for row in self.header_rows.iter().chain(&self.body_rows) {
            for (index, width) in widths.iter_mut().enumerate() {
                let cell = row.get(index).map_or(EMPTY_LINE, String::as_str);
                *width = (*width).max(UnicodeWidthStr::width(cell));
            }
        }
        widths
    }
}

fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_SUPERSCRIPT);
    options.insert(Options::ENABLE_SUBSCRIPT);
    options
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .all(|span| span.content.as_ref().trim().is_empty())
}

fn heading_level_to_usize(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_block_style() -> Style {
    Style::default().fg(Color::LightCyan)
}

fn table_border_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn styled_table_border_line(text: String) -> Line<'static> {
    let style = table_border_style();
    Line::from(Span::styled(text, style)).style(style)
}

fn repeat_char(ch: char, count: usize) -> String {
    repeat_n(ch, count).collect()
}

fn normalize_table_cell(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(TABLE_NEWLINE_REPLACEMENT)
        .trim()
        .to_owned()
}

fn build_table_border(left: char, mid: char, right: char, widths: &[usize]) -> String {
    let mut result = String::new();
    result.push(left);

    let segment_width = widths
        .iter()
        .map(|width| width.saturating_add(TABLE_CELL_PADDING.saturating_mul(2)))
        .collect::<Vec<_>>();

    let mut first = true;
    for width in segment_width {
        if !first {
            result.push(mid);
        }
        first = false;
        result.push_str(repeat_char(TABLE_HORIZONTAL, width).as_str());
    }

    result.push(right);
    result
}

fn build_table_row(row: &[String], widths: &[usize], alignments: &[Alignment]) -> String {
    let mut result = String::new();
    result.push(TABLE_VERTICAL);

    for (index, width) in widths.iter().copied().enumerate() {
        let cell = row.get(index).map_or(EMPTY_LINE, String::as_str);
        let alignment = alignments.get(index).copied().unwrap_or(Alignment::Left);
        let cell_text = align_cell(cell, width, alignment);
        result.push(SPACE_CHAR);
        result.push_str(cell_text.as_str());
        result.push(SPACE_CHAR);
        result.push(TABLE_VERTICAL);
    }

    result
}

fn align_cell(text: &str, width: usize, alignment: Alignment) -> String {
    let text_width = UnicodeWidthStr::width(text);
    let padding = width.saturating_sub(text_width);
    let (left_padding, right_padding) = match alignment {
        Alignment::Left | Alignment::None => (0, padding),
        Alignment::Right => (padding, 0),
        Alignment::Center => {
            let left = padding / 2;
            (left, padding.saturating_sub(left))
        }
    };

    format!(
        "{}{}{}",
        repeat_char(SPACE_CHAR, left_padding),
        text,
        repeat_char(SPACE_CHAR, right_padding)
    )
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use super::render_markdown_lines;

    fn lines_to_plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    }

    #[test]
    fn supports_unordered_and_ordered_lists() {
        let markdown = "
- 一级
  - 二级
1. 第一项
2. 第二项
";
        let lines = render_markdown_lines(markdown);
        let plain = lines_to_plain(lines.as_slice()).join("\n");

        assert!(plain.contains("- 一级"));
        assert!(plain.contains("  - 二级"));
        assert!(plain.contains("1. 第一项"));
        assert!(plain.contains("2. 第二项"));
    }

    #[test]
    fn supports_fenced_code_blocks() {
        let markdown = r#"
```rust
fn main() {
    println!("ok");
}
```
"#;
        let lines = render_markdown_lines(markdown);
        let plain = lines_to_plain(lines.as_slice()).join("\n");

        assert!(plain.contains("```rust"));
        assert!(plain.contains("fn main() {"));
        assert!(plain.contains("println!(\"ok\");"));
        assert!(plain.contains("```"));
    }

    #[test]
    fn supports_tables() {
        let markdown = "
| Name | Count |
| :--- | ---: |
| rust | 3 |
| 表格 | 12 |
";
        let lines = render_markdown_lines(markdown);
        let plain = lines_to_plain(lines.as_slice()).join("\n");

        assert!(plain.contains("┌"));
        assert!(plain.contains("Name"));
        assert!(plain.contains("rust"));
        assert!(plain.contains("表格"));
        assert!(plain.contains("┘"));
    }
}
