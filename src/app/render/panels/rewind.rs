use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{
    App,
    state::message::{RewindFocus, RewindRestoreScope},
};

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

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let [vertical] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [horizontal] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(vertical);
    horizontal
}

impl App {
    pub(crate) fn render_rewind_modal(&self, frame: &mut Frame) {
        if !self.show_rewind {
            return;
        }

        let area = centered_rect(frame.area(), 110, 28);
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Rewind Restore ")
            .title_bottom(
                Line::from(" Tab/←/→ focus · ↑/↓ select · Enter restore · Esc close ").centered(),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let panes = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(inner);
        self.render_rewind_points_list(frame, panes[0]);
        self.render_rewind_restore_details(frame, panes[1]);
    }

    fn render_rewind_points_list(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(
                Style::default().fg(if self.rewind_focus == RewindFocus::Points {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            )
            .title(" Rewind Points ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.rewind_points.is_empty() {
            frame.render_widget(
                Paragraph::new("No rewind points available")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let visible_height = (inner.height as usize) / 2;
        let selected = self
            .rewind_selected
            .min(self.rewind_points.len().saturating_sub(1));
        let scroll_offset = if selected >= visible_height {
            selected.saturating_sub(visible_height.saturating_sub(1))
        } else {
            0
        };
        let end = (scroll_offset + visible_height).min(self.rewind_points.len());
        let items: Vec<ListItem> = self.rewind_points[scroll_offset..end]
            .iter()
            .enumerate()
            .map(|(display_idx, point)| {
                let actual_idx = scroll_offset + display_idx;
                let is_selected = actual_idx == selected;
                let preview_line = Line::from(vec![
                    Span::styled(
                        if is_selected { "> " } else { "  " },
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        &point.preview,
                        if is_selected {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                ]);
                let elapsed = point.timestamp.elapsed().unwrap_or(Duration::from_secs(0));
                let files = point.file_changes.len();
                let additions: usize = point.file_changes.iter().map(|item| item.insertions).sum();
                let deletions: usize = point.file_changes.iter().map(|item| item.deletions).sum();
                let meta = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!(
                            "{} msgs • {}",
                            point.message_count,
                            rewind_time_ago(elapsed)
                        ),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!(" • {} file{}", files, if files == 1 { "" } else { "s" }),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!(" +{}", additions),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(format!(" -{}", deletions), Style::default().fg(Color::Red)),
                ]);
                ListItem::new(vec![preview_line, meta])
            })
            .collect();

        frame.render_widget(List::new(items), inner);
    }

    fn render_rewind_restore_details(&self, frame: &mut Frame, area: Rect) {
        let sections = Layout::vertical([
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(1),
        ])
        .split(area);
        self.render_rewind_scope_selector(frame, sections[0]);
        self.render_rewind_metadata(frame, sections[1]);
        self.render_rewind_diff_preview(frame, sections[2]);
    }

    fn render_rewind_scope_selector(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(
                Style::default().fg(if self.rewind_focus == RewindFocus::Scope {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            )
            .title(" Restore Scope ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let scopes = [
            RewindRestoreScope::CodeAndConversation,
            RewindRestoreScope::ConversationOnly,
            RewindRestoreScope::CodeOnly,
        ];
        let items: Vec<ListItem> = scopes
            .into_iter()
            .map(|scope| {
                let selected = scope == self.rewind_restore_scope;
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if selected { "> " } else { "  " },
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        scope.label(),
                        if selected {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                ]))
            })
            .collect();
        frame.render_widget(List::new(items), inner);
    }

    fn render_rewind_metadata(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Restore Details ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(point) = self.rewind_points.get(self.rewind_selected) else {
            return;
        };
        let elapsed = point.timestamp.elapsed().unwrap_or(Duration::from_secs(0));
        let lines = vec![
            Line::from(vec![
                Span::styled("Target: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&point.preview, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("When: ", Style::default().fg(Color::DarkGray)),
                Span::styled(rewind_time_ago(elapsed), Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("Fork: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "restoring creates a forked timeline",
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Line::from(vec![
                Span::styled("Code restore: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "restores isolated/private state; real workspace stays unchanged until apply",
                    Style::default().fg(Color::White),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_rewind_diff_preview(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Preview ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(point) = self.rewind_points.get(self.rewind_selected) else {
            return;
        };

        if !self.rewind_restore_scope.restores_code() {
            frame.render_widget(
                Paragraph::new(
                    "Conversation-only restore will leave the isolated code state unchanged.",
                )
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }

        if point.file_changes.is_empty() {
            let note = if point.fs_checkpoint_id.is_some() {
                "No file summary was captured for this rewind point. The filesystem snapshot can still be restored."
            } else {
                "No filesystem checkpoint was captured for this rewind point."
            };
            frame.render_widget(
                Paragraph::new(note)
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        };

        let lines = point
            .file_changes
            .iter()
            .map(|change| {
                Line::from(vec![
                    Span::styled(&change.path, Style::default().fg(Color::White)),
                    Span::raw(" "),
                    Span::styled(
                        format!("+{}", change.insertions),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("-{}", change.deletions),
                        Style::default().fg(Color::Red),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
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
