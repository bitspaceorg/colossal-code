use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

pub(crate) fn render_tip_line(tip: &str, help_color: Color) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::raw(" "));

    let mut remaining = tip.to_string();
    if remaining.contains(".niterules") {
        let parts: Vec<&str> = remaining.splitn(2, ".niterules").collect();
        if !parts[0].is_empty() {
            spans.push(Span::raw(parts[0].to_string()));
        }
        spans.push(Span::styled(
            ".niterules",
            Style::default().fg(Color::Magenta),
        ));
        remaining = parts.get(1).unwrap_or(&"").to_string();
    }

    if remaining.contains("/help") {
        let parts: Vec<&str> = remaining.splitn(2, "/help").collect();
        if !parts[0].is_empty() {
            spans.push(Span::raw(parts[0].to_string()));
        }
        spans.push(Span::styled("/help", Style::default().fg(help_color)));
        remaining = parts.get(1).unwrap_or(&"").to_string();
    }

    if remaining.contains("Alt+n") {
        let parts: Vec<&str> = remaining.splitn(2, "Alt+n").collect();
        if !parts[0].is_empty() {
            spans.push(Span::raw(parts[0].to_string()));
        }
        spans.push(Span::styled("Alt+n", Style::default().fg(Color::Yellow)));
        remaining = parts.get(1).unwrap_or(&"").to_string();
    }

    if !remaining.is_empty() {
        spans.push(Span::raw(remaining));
    }

    Line::from(spans)
}
