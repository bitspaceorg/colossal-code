use std::time::Duration;

use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::app::App;

fn rewind_time_ago(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m ago", elapsed.as_secs() / 60)
    } else if elapsed.as_secs() < 86400 {
        format!("{}h ago", elapsed.as_secs() / 3600)
    } else {
        format!("{}d ago", elapsed.as_secs() / 86400)
    }
}

impl App {
    pub(crate) fn render_rewind_panel(
        &self,
        frame: &mut Frame,
        rewind_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Rewind Conversation ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to restore · Esc to close ").centered(),
            );

        let inner = block.inner(rewind_area);
        frame.render_widget(block, rewind_area);

        if self.rewind_points.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No rewind points available.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::raw(
                    "Rewind points are created automatically as you interact",
                )),
            ];
            let content_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };
            frame.render_widget(Paragraph::new(content), content_area);
            return;
        }

        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} rewind points", self.rewind_points.len()),
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        let lines_per_item = 2;
        let visible_height = inner.height.saturating_sub(2) as usize;
        let max_visible_items = visible_height / lines_per_item;
        let scroll_offset = if self.rewind_selected >= max_visible_items {
            self.rewind_selected.saturating_sub(max_visible_items - 1)
        } else {
            0
        };

        let visible_end = (scroll_offset + max_visible_items).min(self.rewind_points.len());
        let visible_points = &self.rewind_points[scroll_offset..visible_end];

        let items: Vec<ListItem> = visible_points
            .iter()
            .enumerate()
            .map(|(local_idx, point)| {
                let actual_idx = scroll_offset + local_idx;
                let is_selected = actual_idx == self.rewind_selected;

                let preview_line = if is_selected {
                    Line::from(vec![
                        Span::styled("> ", Style::default().fg(Color::Yellow)),
                        Span::styled(&point.preview, Style::default().fg(Color::Yellow)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(&point.preview, Style::default().fg(Color::White)),
                    ])
                };

                let elapsed = point.timestamp.elapsed().unwrap_or(Duration::from_secs(0));
                let time_ago = rewind_time_ago(elapsed);
                let total_insertions: usize =
                    point.file_changes.iter().map(|fc| fc.insertions).sum();
                let total_deletions: usize = point.file_changes.iter().map(|fc| fc.deletions).sum();
                let files_count = point.file_changes.len();

                let mut metadata_parts = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} msgs • {}", point.message_count, time_ago),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];

                if files_count > 0 {
                    metadata_parts.push(Span::styled(
                        format!(
                            " • {} file{}",
                            files_count,
                            if files_count == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::DarkGray),
                    ));

                    if total_insertions > 0 {
                        metadata_parts.push(Span::styled(
                            format!(" +{}", total_insertions),
                            Style::default().fg(Color::Green),
                        ));
                    }
                    if total_deletions > 0 {
                        metadata_parts.push(Span::styled(
                            format!(" -{}", total_deletions),
                            Style::default().fg(Color::Red),
                        ));
                    }
                }

                ListItem::new(vec![preview_line, Line::from(metadata_parts)])
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

#[cfg(test)]
mod tests {
    use super::rewind_time_ago;
    use std::time::Duration;

    #[test]
    fn rewind_time_ago_uses_expected_units() {
        assert_eq!(rewind_time_ago(Duration::from_secs(59)), "59s ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(60)), "1m ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(3600)), "1h ago");
        assert_eq!(rewind_time_ago(Duration::from_secs(172800)), "2d ago");
    }
}
