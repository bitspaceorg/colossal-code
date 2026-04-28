use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, render::edit_file_diff::build_edit_file_diff};

impl App {
    pub(crate) fn render_isolated_changes_panel(
        &self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
    ) {
        let has_conflicts = !self.isolated_changes.conflict_paths.is_empty();
        let title = if has_conflicts {
            " Isolated Changes Review · Apply Blocked "
        } else {
            " Isolated Changes Review "
        };
        let border_color = if has_conflicts {
            Color::Red
        } else {
            Color::Cyan
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .title(title)
            .title_bottom(
                Line::from(" ↑/↓ select · Enter apply · d discard · Esc close ").centered(),
            );

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
        let header_area = sections[0];
        let body_area = sections[1];

        let noun = if self.isolated_changes.pending_count == 1 {
            "change"
        } else {
            "changes"
        };
        let mut header_spans = vec![Span::styled(
            format!(
                " {} pending isolated {}",
                self.isolated_changes.pending_count, noun
            ),
            Style::default().fg(Color::DarkGray),
        )];
        if has_conflicts {
            header_spans.push(Span::styled(" • ", Style::default().fg(Color::DarkGray)));
            header_spans.push(Span::styled(
                format!("{} conflicting", self.isolated_changes.conflict_paths.len()),
                Style::default().fg(Color::Red),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(header_spans)), header_area);

        let panes = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(body_area);
        self.render_isolated_changes_list(frame, panes[0]);
        self.render_isolated_changes_diff(frame, panes[1]);
    }

    fn render_isolated_changes_list(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Files ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.isolated_changes.review_entries.is_empty() {
            frame.render_widget(
                Paragraph::new("No isolated changes pending")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let visible_height = inner.height as usize;
        let selected = self
            .isolated_changes
            .review_selected
            .min(self.isolated_changes.review_entries.len().saturating_sub(1));
        let scroll_offset = if selected >= visible_height {
            selected.saturating_sub(visible_height.saturating_sub(1))
        } else {
            0
        };
        let end = (scroll_offset + visible_height).min(self.isolated_changes.review_entries.len());

        let items: Vec<ListItem> = self.isolated_changes.review_entries[scroll_offset..end]
            .iter()
            .enumerate()
            .map(|(display_idx, entry)| {
                let actual_idx = scroll_offset + display_idx;
                let is_selected = actual_idx == selected;
                let path = entry.path.display().to_string();
                let is_conflict = self
                    .isolated_changes
                    .conflict_paths
                    .iter()
                    .any(|p| p == &path);
                let marker = if is_selected { "> " } else { "  " };
                let color = if is_conflict {
                    Color::Red
                } else {
                    Color::White
                };
                let style = if is_selected {
                    Style::default().fg(color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Yellow)),
                    Span::styled(path, style),
                ]))
            })
            .collect();

        frame.render_widget(List::new(items), inner);
    }

    fn render_isolated_changes_diff(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Diff ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(entry) = self
            .isolated_changes
            .review_entries
            .get(self.isolated_changes.review_selected)
        else {
            frame.render_widget(
                Paragraph::new("Select a file to preview")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };

        let rendered = build_edit_file_diff(
            &entry.old_string,
            &entry.new_string,
            &entry.path.display().to_string(),
            inner.width as usize,
            Span::raw(""),
            true,
        );
        frame.render_widget(
            Paragraph::new(Text::from(rendered.lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}
