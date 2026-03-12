use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::App;

impl App {
    pub(crate) fn render_resume_panel(
        &self,
        frame: &mut Frame,
        resume_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" Saved Conversations ")
            .title_bottom(
                Line::from(
                    " ↑/↓ to select · Enter to restore · d to delete · f to fork · Esc to close ",
                )
                .centered(),
            );

        let inner = block.inner(resume_area);
        frame.render_widget(block, resume_area);

        if self.resume_conversations.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No saved conversations found.",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::raw("Use /save to save your current conversation")),
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

        let fork_count = self
            .resume_conversations
            .iter()
            .filter(|c| c.forked_from.is_some())
            .count();
        let count_text = if fork_count > 0 {
            format!(
                " {} saved conversations ({} forks)",
                self.resume_conversations.len(),
                fork_count
            )
        } else {
            format!(" {} saved conversations", self.resume_conversations.len())
        };
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                count_text,
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        let lines_per_item = 2;
        let visible_height = inner.height.saturating_sub(2) as usize;
        let max_visible_items = visible_height / lines_per_item;

        let scroll_offset = if self.resume_selected >= max_visible_items {
            self.resume_selected.saturating_sub(max_visible_items - 1)
        } else {
            0
        };

        let visible_end = (scroll_offset + max_visible_items).min(self.resume_conversations.len());
        let visible_conversations = &self.resume_conversations[scroll_offset..visible_end];

        let items: Vec<ListItem> = visible_conversations
            .iter()
            .enumerate()
            .map(|(local_idx, conv)| {
                let actual_idx = scroll_offset + local_idx;
                let is_selected = actual_idx == self.resume_selected;
                let is_fork = conv.forked_from.is_some();

                let title_line = if is_selected {
                    if is_fork {
                        Line::from(vec![
                            Span::styled("> ⎇ ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled("> ", Style::default().fg(Color::Green)),
                            Span::styled(&conv.preview, Style::default().fg(Color::Green)),
                        ])
                    }
                } else if is_fork {
                    Line::from(vec![
                        Span::raw("  ⎇ "),
                        Span::styled(&conv.preview, Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(&conv.preview, Style::default().fg(Color::White)),
                    ])
                };

                let msg_count = format!("{} msgs", conv.message_count);
                let branch_str = conv
                    .git_branch
                    .as_ref()
                    .map(|b| format!(" • {}", b))
                    .unwrap_or_default();

                let metadata_line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} • {}{}", conv.time_ago_str, msg_count, branch_str),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                ListItem::new(vec![title_line, metadata_line])
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
