use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, ConversationMetadata};

fn trim_title(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return text.chars().take(max_width).collect();
    }

    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + ch_width + 3 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

fn title_line(conv: &ConversationMetadata, selected: bool, max_width: usize) -> Line<'static> {
    let title = trim_title(conv.display_title(), max_width);
    if selected {
        if conv.forked_from.is_some() {
            return Line::from(vec![
                Span::styled("> ⎇ ", Style::default().fg(Color::Green)),
                Span::styled(title, Style::default().fg(Color::Green)),
            ]);
        }
        return Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Green)),
            Span::styled(title, Style::default().fg(Color::Green)),
        ]);
    }
    if conv.forked_from.is_some() {
        return Line::from(vec![
            Span::raw("  ⎇ "),
            Span::styled(title, Style::default().fg(Color::White)),
        ]);
    }
    Line::from(vec![
        Span::raw("  "),
        Span::styled(title, Style::default().fg(Color::White)),
    ])
}

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
                let title_line =
                    title_line(conv, is_selected, inner.width.saturating_sub(6) as usize);

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

#[cfg(test)]
mod tests {
    use super::{title_line, trim_title};
    use crate::app::ConversationMetadata;
    use unicode_width::UnicodeWidthStr;

    fn metadata(title: Option<&str>, preview: &str) -> ConversationMetadata {
        ConversationMetadata {
            id: "1".to_string(),
            updated_at: std::time::SystemTime::now(),
            git_branch: None,
            message_count: 1,
            title: title.map(str::to_string),
            preview: preview.to_string(),
            file_path: "/tmp/test.json".into(),
            time_ago_str: "1m ago".to_string(),
            forked_from: None,
        }
    }

    #[test]
    fn trim_title_clips_without_duplication() {
        let text = "Confirming Deletion of Current Directory Contents";
        let trimmed = trim_title(text, 24);
        assert_eq!(trimmed, "Confirming Deletion o...");
        assert!(trimmed.width() <= 24);
    }

    #[test]
    fn title_line_uses_title_once() {
        let line = title_line(
            &metadata(
                Some("Confirming Deletion of Current Directory Contents"),
                "preview",
            ),
            true,
            24,
        );
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(rendered, "> Confirming Deletion o...");
        assert_eq!(rendered.matches("Confirming").count(), 1);
    }
}
