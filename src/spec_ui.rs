use std::collections::HashMap;

use agent_core::{SpecSheet, StepStatus, TaskSummary, VerificationStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Wrap},
};

pub fn render_spec_pane_view<'a>(
    frame: &mut Frame<'a>,
    area: Rect,
    spec: &SpecSheet,
    orchestrator_paused: bool,
    selected_index: usize,
    show_history: bool,
    step_drawer_open: bool,
    orchestrator_history: &[TaskSummary],
    latest_summaries: &HashMap<String, TaskSummary>,
) {
    if area.height < 6 {
        return;
    }

    let sections = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(6),
        Constraint::Length(8),
    ])
    .split(area);

    // Metadata section
    let created_at = spec.created_at.format("%Y-%m-%d %H:%M").to_string();
    let paused_badge = if orchestrator_paused { " [PAUSED]" } else { "" };
    let metadata_text = vec![
        Line::from(vec![
            Span::styled("📋 ", Style::default().fg(Color::Yellow)),
            Span::styled(
                &spec.title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(paused_badge, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Created by ", Style::default().fg(Color::DarkGray)),
            Span::styled(&spec.created_by, Style::default().fg(Color::Gray)),
            Span::styled(" • ", Style::default().fg(Color::DarkGray)),
            Span::styled(created_at, Style::default().fg(Color::Gray)),
        ]),
        Line::from(vec![
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
        ]),
    ];
    let metadata = Paragraph::new(metadata_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Spec metadata "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(metadata, sections[0]);

    // Table section
    let mut rows: Vec<Row> = Vec::new();
    for (idx, step) in spec.steps.iter().enumerate() {
        let status_icon = match step.status {
            StepStatus::Pending => "○",
            StepStatus::InProgress => "◐",
            StepStatus::Completed => "●",
            StepStatus::Failed => "✗",
        };
        let deps = if step.dependencies.is_empty() {
            "—".to_string()
        } else {
            format!("↳ {}", step.dependencies.join(", "))
        };
        let criteria_snippet = if let Some(first) = step.acceptance_criteria.first() {
            let mut snippet = first.clone();
            if snippet.len() > 24 {
                snippet.truncate(24);
                snippet.push_str("…");
            }
            snippet
        } else {
            "No acceptance criteria".to_string()
        };
        let mut row = Row::new(vec![
            Cell::from(step.index.clone()),
            Cell::from(status_icon.to_string()),
            Cell::from(step.title.clone()),
            Cell::from(format!("{} | {}", criteria_snippet, deps)),
        ]);
        if idx == selected_index {
            row = row.style(Style::default().fg(Color::Cyan));
        }
        rows.push(row);
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Percentage(35),
            Constraint::Percentage(55),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("#"),
            Cell::from("State"),
            Cell::from("Title"),
            Cell::from("Details"),
        ])
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Steps "),
    )
    .column_spacing(1);
    frame.render_widget(table, sections[1]);

    if show_history {
        let mut lines: Vec<Line> = Vec::new();
        if orchestrator_history.is_empty() {
            lines.push(Line::from("No task history available."));
        } else {
            for summary in orchestrator_history {
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
            }
        }
        let history_block = Block::default()
            .title(" Spec history (H to toggle) ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        frame.render_widget(
            Paragraph::new(lines)
                .block(history_block)
                .wrap(Wrap { trim: true }),
            sections[2],
        );
    } else {
        let drawer_block = Block::default()
            .title(" Step drawer (Enter to toggle) ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        let mut drawer_lines: Vec<Line> = Vec::new();
        if let Some(step) = spec.steps.get(selected_index) {
            if step_drawer_open {
                if let Some(summary) = latest_summaries.get(&step.index) {
                    drawer_lines.push(Line::from(format!("Summary: {}", summary.summary_text)));
                    if !summary.tests_run.is_empty() {
                        let tests = summary
                            .tests_run
                            .iter()
                            .map(|test| format!("{}({:?})", test.name, test.result))
                            .collect::<Vec<_>>()
                            .join(", ");
                        drawer_lines.push(Line::from(format!("Tests: {}", tests)));
                    }
                    if !summary.artifacts_touched.is_empty() {
                        drawer_lines.push(Line::from(format!(
                            "Artifacts: {}",
                            summary.artifacts_touched.join(", ")
                        )));
                    }
                    if !summary.verification.feedback.is_empty() {
                        for feedback in &summary.verification.feedback {
                            drawer_lines.push(Line::from(format!(
                                "Feedback {}: {}",
                                feedback.author, feedback.message
                            )));
                        }
                    }
                } else {
                    drawer_lines.push(Line::from("No summary available yet."));
                }
            } else {
                drawer_lines.push(Line::from(vec![
                    Span::styled("Step ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&step.index, Style::default().fg(Color::Blue)),
                    Span::raw(": "),
                    Span::styled(
                        &step.title,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                if !step.instructions.is_empty() {
                    drawer_lines.push(Line::from(format!("Instructions: {}", step.instructions)));
                }
                if !step.acceptance_criteria.is_empty() {
                    drawer_lines.push(Line::from("Acceptance criteria:"));
                    for criterion in &step.acceptance_criteria {
                        drawer_lines.push(Line::from(format!("  • {}", criterion)));
                    }
                }
                if !step.dependencies.is_empty() {
                    drawer_lines.push(Line::from(format!(
                        "Depends on: {}",
                        step.dependencies.join(", ")
                    )));
                }
            }
        }
        if drawer_lines.is_empty() {
            drawer_lines.push(Line::from("No step selected."));
        }
        frame.render_widget(
            Paragraph::new(drawer_lines)
                .block(drawer_block)
                .wrap(Wrap { trim: true }),
            sections[2],
        );
    }
}
