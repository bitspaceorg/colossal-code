use agent_core::{TaskSummary, VerificationStatus};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

pub(crate) fn build_history_lines(history: &[TaskSummary]) -> Vec<Line<'static>> {
    if history.is_empty() {
        return vec![Line::from("History: no entries yet.".to_string())];
    }
    let mut lines = vec![Line::from(Span::styled(
        "History (H to toggle):",
        Style::default().fg(Color::Gray),
    ))];
    for summary in history {
        let status_icon = match summary.verification.status {
            VerificationStatus::Passed => "✓",
            VerificationStatus::Failed => "✗",
            VerificationStatus::Pending => "○",
        };
        lines.push(Line::from(vec![
            Span::styled(status_icon, Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(
                format!("Step {} · {}", summary.step_index, summary.summary_text),
                Style::default().fg(Color::White),
            ),
        ]));
        if !summary.tests_run.is_empty() {
            let tests = summary
                .tests_run
                .iter()
                .map(|test| format!("{}({:?})", test.name, test.result))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(format!("  Tests: {}", tests)));
        }
        if !summary.artifacts_touched.is_empty() {
            lines.push(Line::from(format!(
                "  Artifacts: {}",
                summary.artifacts_touched.join(", ")
            )));
        }
        if !summary.verification.feedback.is_empty() {
            for feedback in &summary.verification.feedback {
                lines.push(Line::from(format!(
                    "  Feedback {}: {}",
                    feedback.author, feedback.message
                )));
            }
        }
        if let Some(worktree) = &summary.worktree {
            let mut spans = vec![
                Span::styled("  Branch: ", Style::default().fg(Color::DarkGray)),
                Span::styled(worktree.branch.clone(), Style::default().fg(Color::White)),
            ];
            spans.push(Span::styled(
                format!(" ({})", worktree.path),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::from(spans));
        }
    }
    lines
}
