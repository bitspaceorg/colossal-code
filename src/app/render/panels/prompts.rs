use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

pub fn render_sandbox_prompt<'a>(sandbox_blocked_path: &'a str) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Red)),
        Span::raw("Add "),
        Span::styled(sandbox_blocked_path, Style::default().fg(Color::Yellow)),
        Span::raw(" to writable roots?"),
    ]));

    let option_spans = vec![
        Span::raw("  "),
        Span::styled("0: ", Style::default().fg(Color::Yellow)),
        Span::raw("Accept   "),
        Span::styled("1: ", Style::default().fg(Color::Yellow)),
        Span::raw("Deny   "),
        Span::styled("2: ", Style::default().fg(Color::Yellow)),
        Span::raw("Interrupt and tell Nite what to do"),
    ];
    lines.push(Line::from(option_spans));

    lines
}

pub fn render_approval_prompt<'a>(approval_prompt_content: &'a str) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Yellow)),
        Span::raw(approval_prompt_content),
    ]));

    let option_spans = vec![
        Span::raw("  "),
        Span::styled("0: ", Style::default().fg(Color::Yellow)),
        Span::raw("Approve   "),
        Span::styled("1: ", Style::default().fg(Color::Yellow)),
        Span::raw("Deny   "),
        Span::styled("2: ", Style::default().fg(Color::Yellow)),
        Span::raw("Interrupt and tell Nite what to do"),
    ];
    lines.push(Line::from(option_spans));

    lines
}

pub fn render_isolated_changes_prompt(pending_count: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let noun = if pending_count == 1 {
        "change"
    } else {
        "changes"
    };

    lines.push(Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "Apply {} isolated {} to the workspace?",
            pending_count, noun
        )),
    ]));

    let option_spans = vec![
        Span::raw("  "),
        Span::styled("0: ", Style::default().fg(Color::Yellow)),
        Span::raw("Apply   "),
        Span::styled("1: ", Style::default().fg(Color::Yellow)),
        Span::raw("Keep isolated   "),
        Span::styled("2: ", Style::default().fg(Color::Yellow)),
        Span::raw("Discard"),
    ];
    lines.push(Line::from(option_spans));

    lines
}

#[cfg(test)]
mod tests {
    use super::{render_approval_prompt, render_isolated_changes_prompt, render_sandbox_prompt};
    use ratatui::style::Color;

    #[test]
    fn sandbox_prompt_renders_expected_lines_and_styles() {
        let lines = render_sandbox_prompt("/tmp/workspace");

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content.as_ref(), "● ");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[0].spans[2].content.as_ref(), "/tmp/workspace");
        assert_eq!(lines[0].spans[2].style.fg, Some(Color::Yellow));
        assert_eq!(lines[1].spans[2].content.as_ref(), "Accept   ");
        assert_eq!(
            lines[1].spans[6].content.as_ref(),
            "Interrupt and tell Nite what to do"
        );
    }

    #[test]
    fn approval_prompt_renders_expected_lines_and_styles() {
        let lines = render_approval_prompt("Allow `bash` command?");

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content.as_ref(), "● ");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Yellow));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Allow `bash` command?");
        assert_eq!(lines[1].spans[2].content.as_ref(), "Approve   ");
        assert_eq!(lines[1].spans[4].content.as_ref(), "Deny   ");
    }

    #[test]
    fn isolated_changes_prompt_renders_expected_lines() {
        let lines = render_isolated_changes_prompt(3);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(
            lines[0].spans[1].content.as_ref(),
            "Apply 3 isolated changes to the workspace?"
        );
        assert_eq!(lines[1].spans[2].content.as_ref(), "Apply   ");
        assert_eq!(lines[1].spans[4].content.as_ref(), "Keep isolated   ");
        assert_eq!(lines[1].spans[6].content.as_ref(), "Discard");
    }
}
