//! TUI 主界面布局切分。

use ratatui::layout::{Constraint, Direction, Layout, Rect};

const HEADER_HEIGHT: u16 = 7;
const INPUT_HEIGHT: u16 = 4;
const FOOTER_HEIGHT: u16 = 1;
const MIN_CENTER_HEIGHT: u16 = 4;
const WORKSPACE_WIDTH_PERCENT: u16 = 68;
const RUNTIME_WIDTH_PERCENT: u16 = 32;
const MIN_CHAT_WIDTH: u16 = 32;
const EVENT_PANEL_WIDTH_EXPANDED: u16 = 34;
const EVENT_PANEL_WIDTH_COLLAPSED: u16 = 1;

#[derive(Clone, Copy)]
pub(super) struct TuiAreas {
    pub(super) workspace: Rect,
    pub(super) runtime: Rect,
    pub(super) chat: Rect,
    pub(super) event: Rect,
    pub(super) input: Rect,
    pub(super) footer: Rect,
}

pub(super) fn split_tui_areas(area: Rect, event_panel_collapsed: bool) -> Option<TuiAreas> {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(MIN_CENTER_HEIGHT),
            Constraint::Length(INPUT_HEIGHT),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .split(area);

    let [header, center, input, footer] = root.as_ref() else {
        return None;
    };

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(WORKSPACE_WIDTH_PERCENT),
            Constraint::Percentage(RUNTIME_WIDTH_PERCENT),
        ])
        .split(*header);

    let [workspace, runtime] = header_chunks.as_ref() else {
        return None;
    };

    let event_width = if event_panel_collapsed {
        EVENT_PANEL_WIDTH_COLLAPSED
    } else {
        EVENT_PANEL_WIDTH_EXPANDED
    };
    let center_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(MIN_CHAT_WIDTH),
            Constraint::Length(event_width),
        ])
        .split(*center);

    let [chat, event] = center_chunks.as_ref() else {
        return None;
    };

    Some(TuiAreas {
        workspace: *workspace,
        runtime: *runtime,
        chat: *chat,
        event: *event,
        input: *input,
        footer: *footer,
    })
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{EVENT_PANEL_WIDTH_COLLAPSED, EVENT_PANEL_WIDTH_EXPANDED, split_tui_areas};

    #[test]
    fn split_tui_areas_reserves_event_panel_widths() {
        let area = Rect::new(0, 0, 120, 40);

        let expanded = split_tui_areas(area, false).expect("split expanded areas");
        let collapsed = split_tui_areas(area, true).expect("split collapsed areas");

        assert_eq!(expanded.event.width, EVENT_PANEL_WIDTH_EXPANDED);
        assert_eq!(collapsed.event.width, EVENT_PANEL_WIDTH_COLLAPSED);
        assert_eq!(expanded.chat.x, collapsed.chat.x);
        assert!(collapsed.chat.width > expanded.chat.width);
    }

    #[test]
    fn split_tui_areas_keeps_vertical_bands_ordered() {
        let area = Rect::new(10, 5, 100, 30);

        let areas = split_tui_areas(area, false).expect("split areas");

        assert_eq!(areas.workspace.y, area.y);
        assert_eq!(areas.runtime.y, area.y);
        assert!(areas.chat.y > areas.workspace.y);
        assert!(areas.input.y > areas.chat.y);
        assert!(areas.footer.y > areas.input.y);
    }
}
