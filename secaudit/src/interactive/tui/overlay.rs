//! TUI 浮层绘制辅助。

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const CONFIRM_OVERLAY_WIDTH_PERCENT: u16 = 72;
const CONFIRM_OVERLAY_HEIGHT_PERCENT: u16 = 32;

pub(super) fn draw_confirmation_overlay(frame: &mut Frame<'_>, prompt: &str) {
    let area = centered_rect(
        CONFIRM_OVERLAY_WIDTH_PERCENT,
        CONFIRM_OVERLAY_HEIGHT_PERCENT,
        frame.area(),
    );
    let lines = vec![
        Line::from(Span::styled(
            "工具确认",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(String::new()),
        Line::from(prompt.to_owned()),
        Line::from(String::new()),
        Line::from(vec![
            Span::styled("Y", Style::default().fg(Color::LightGreen)),
            Span::raw(" 允许    "),
            Span::styled("N / Esc / Enter", Style::default().fg(Color::LightRed)),
            Span::raw(" 拒绝"),
        ]),
    ];

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirmation")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
}

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, outer: Rect) -> Rect {
    let top = (100u16.saturating_sub(percent_y)) / 2;
    let left = (100u16.saturating_sub(percent_x)) / 2;

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(top),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(top),
        ])
        .split(outer);

    let middle = match vertical.as_ref() {
        [_, mid, _] => *mid,
        _ => return outer,
    };

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(left),
        ])
        .split(middle);

    match horizontal.as_ref() {
        [_, mid, _] => *mid,
        _ => outer,
    }
}
