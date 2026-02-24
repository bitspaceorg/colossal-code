use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_autocomplete(
    frame: &mut Frame,
    area: Rect,
    suggestions: &[(String, String)],
    selected_index: usize,
) {
    let scroll_offset =
        calculate_scroll_offset(area.height as usize, suggestions.len(), selected_index);

    let lines: Vec<Line> = suggestions
        .iter()
        .enumerate()
        .map(|(idx, (cmd, desc))| {
            let is_selected = idx == selected_index;
            let cmd_style = if is_selected {
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let desc_style = if is_selected {
                Style::default().fg(Color::Blue)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let padded_cmd = format!("{:width$}", cmd, width = 35);

            Line::from(vec![
                Span::raw("  "),
                Span::styled(padded_cmd, cmd_style),
                Span::styled(desc.clone(), desc_style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines).scroll((scroll_offset as u16, 0));
    frame.render_widget(paragraph, area);
}

fn calculate_scroll_offset(
    visible_height: usize,
    total_items: usize,
    selected_index: usize,
) -> usize {
    if visible_height == 0 || total_items <= visible_height {
        return 0;
    }

    if selected_index < visible_height / 2 {
        0
    } else if selected_index >= total_items.saturating_sub(visible_height / 2) {
        total_items.saturating_sub(visible_height)
    } else {
        selected_index.saturating_sub(visible_height / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::calculate_scroll_offset;

    #[test]
    fn scroll_offset_is_zero_when_list_fits() {
        assert_eq!(calculate_scroll_offset(8, 6, 2), 0);
    }

    #[test]
    fn scroll_offset_is_zero_near_start() {
        assert_eq!(calculate_scroll_offset(6, 20, 2), 0);
    }

    #[test]
    fn scroll_offset_tracks_center_in_middle() {
        assert_eq!(calculate_scroll_offset(6, 20, 10), 7);
    }

    #[test]
    fn scroll_offset_clamps_near_end() {
        assert_eq!(calculate_scroll_offset(6, 20, 19), 14);
    }
}
