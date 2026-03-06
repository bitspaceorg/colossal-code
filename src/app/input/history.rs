use agent_core::{TaskSummary, VerificationStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

pub fn render_history_panel(
    frame: &mut Frame,
    area: Rect,
    orchestrator_history: &[TaskSummary],
    selected_index: usize,
) {
    if area.height < 4 {
        return;
    }

    let outer = Block::default()
        .title(" Spec history (Esc to close) ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    if orchestrator_history.is_empty() {
        frame.render_widget(
            Paragraph::new("No task history yet.")
                .block(Block::default())
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let chunks =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(inner);
    let items: Vec<ListItem> = orchestrator_history
        .iter()
        .map(|summary| {
            let status_icon = match summary.verification.status {
                VerificationStatus::Passed => "✓",
                VerificationStatus::Failed => "✗",
                VerificationStatus::Pending => "○",
            };
            ListItem::new(format!(
                "{} Step {} · {}",
                status_icon, summary.step_index, summary.summary_text
            ))
        })
        .collect();

    let max_index = orchestrator_history.len().saturating_sub(1);
    let selected_index = selected_index.min(max_index);

    let mut state = ListState::default();
    state.select(Some(selected_index));

    let list = List::new(items)
        .block(Block::default())
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let mut detail_lines: Vec<Line> = Vec::new();
    if let Some(summary) = orchestrator_history.get(selected_index) {
        detail_lines.push(Line::from(format!("Step {}", summary.step_index)));
        detail_lines.push(Line::from(summary.summary_text.clone()));

        if !summary.tests_run.is_empty() {
            let tests = summary
                .tests_run
                .iter()
                .map(|test| format!("{}({:?})", test.name, test.result))
                .collect::<Vec<_>>()
                .join(", ");
            detail_lines.push(Line::from(format!("Tests: {}", tests)));
        }

        if !summary.artifacts_touched.is_empty() {
            detail_lines.push(Line::from(format!(
                "Artifacts: {}",
                summary.artifacts_touched.join(", ")
            )));
        }

        detail_lines.push(Line::from(format!(
            "Verification: {:?}",
            summary.verification.status
        )));

        for feedback in &summary.verification.feedback {
            detail_lines.push(Line::from(format!(
                "{}: {}",
                feedback.author, feedback.message
            )));
        }
    }

    if detail_lines.is_empty() {
        detail_lines.push(Line::from("Select an entry to inspect details."));
    }

    frame.render_widget(
        Paragraph::new(detail_lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(" Details ")
                    .border_type(BorderType::Plain),
            )
            .wrap(Wrap { trim: true }),
        chunks[1],
    );
}
