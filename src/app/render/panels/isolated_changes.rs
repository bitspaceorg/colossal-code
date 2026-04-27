use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::app::App;

impl App {
    pub(crate) fn render_isolated_conflicts_panel(
        &self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Red))
            .title(" Apply Blocked ")
            .title_bottom(Line::from(" Conflicting workspace paths · Esc to close ").centered());

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let count = self.isolated_changes.conflict_paths.len();
        let noun = if count == 1 { "path" } else { "paths" };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} conflicting {}", count, noun),
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        let items: Vec<ListItem> = self
            .isolated_changes
            .conflict_paths
            .iter()
            .map(|path| {
                ListItem::new(Line::from(vec![
                    Span::styled("• ", Style::default().fg(Color::Red)),
                    Span::styled(path.clone(), Style::default().fg(Color::White)),
                ]))
            })
            .collect();

        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_widget(List::new(items), list_area);
    }
}
