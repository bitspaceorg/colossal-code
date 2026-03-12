use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::App;

impl App {
    pub(crate) fn render_background_tasks(
        &self,
        frame: &mut Frame,
        task_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Background tasks ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to view · k to kill · Esc to close ").centered(),
            );

        let task_count_text = format!(" {} active shells", self.background_tasks.len());
        let items: Vec<ListItem> = self
            .background_tasks
            .iter()
            .enumerate()
            .map(|(idx, (_session_id, command, _log_file, _start_time))| {
                let is_selected = idx == self.background_tasks_selected;
                let max_cmd_len = task_area.width.saturating_sub(10) as usize;
                let display_cmd = if command.len() > max_cmd_len {
                    format!("{} …", &command[..max_cmd_len.saturating_sub(2)])
                } else {
                    command.clone()
                };

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(">  ", Style::default().fg(Color::Blue)),
                        Span::styled(display_cmd, Style::default().fg(Color::Blue)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("   "),
                        Span::styled(display_cmd, Style::default().fg(Color::White)),
                    ])
                };

                ListItem::new(line)
            })
            .collect();

        let inner = block.inner(task_area);
        frame.render_widget(block, task_area);

        let count_line = Line::from(Span::styled(
            task_count_text,
            Style::default().fg(Color::DarkGray),
        ));
        let count_para = Paragraph::new(count_line);
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(count_para, count_area);

        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_widget(List::new(items), list_area);
    }
}
