use agent_core::SpecSheet;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(super) fn append_spec_metadata_lines(
    lines: &mut Vec<Line<'static>>,
    spec: &SpecSheet,
    paused: bool,
) {
    let paused_badge = if paused { " [PAUSED]" } else { "" };
    lines.push(Line::from(vec![
        Span::styled("📋 ", Style::default().fg(Color::Yellow)),
        Span::styled(
            spec.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(paused_badge, Style::default().fg(Color::Yellow)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Created by ", Style::default().fg(Color::DarkGray)),
        Span::styled(spec.created_by.clone(), Style::default().fg(Color::Gray)),
        Span::styled(" • ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            spec.created_at.format("%Y-%m-%d %H:%M").to_string(),
            Style::default().fg(Color::Gray),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Controls: ", Style::default().fg(Color::DarkGray)),
        Span::styled("P Pause", Style::default().fg(Color::Cyan)),
        Span::raw(" · "),
        Span::styled("R Rerun", Style::default().fg(Color::Cyan)),
        Span::raw(" · "),
        Span::styled("A Abort", Style::default().fg(Color::Cyan)),
        Span::raw(" · "),
        Span::styled("H History", Style::default().fg(Color::Cyan)),
        Span::raw(" · "),
        Span::styled("Enter Drawer", Style::default().fg(Color::Cyan)),
    ]));
    lines.push(Line::from(""));
}
