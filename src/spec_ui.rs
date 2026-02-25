use std::collections::HashMap;

use agent_core::{SpecSheet, SpecStep, StepStatus, TaskSummary, VerificationStatus};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::{SessionRole, StepToolCallEntry, ToolCallStatus};

pub struct OrchestrationStatusLine {
    pub current_frame: String,
    pub color_spans: Vec<(String, Color)>,
    pub elapsed_secs: u64,
}

pub fn compose_tool_plan_view_lines(
    lines: &mut Vec<Line<'_>>,
    plan_lines: Vec<Line<'static>>,
    orchestration_status: Option<OrchestrationStatusLine>,
) {
    let mut plan_lines = plan_lines.into_iter().peekable();
    if plan_lines.peek().is_none() {
        return;
    }

    lines.push(Line::from(" "));
    lines.extend(plan_lines);

    if let Some(status) = orchestration_status {
        let mins = status.elapsed_secs / 60;
        let secs = status.elapsed_secs % 60;
        let time_str = if mins > 0 {
            format!("{}m {:02}s", mins, secs)
        } else {
            format!("{}s", secs)
        };

        let mut spans = vec![
            Span::styled(
                status.current_frame,
                Style::default().fg(Color::Rgb(255, 165, 0)),
            ),
            Span::raw(" "),
        ];
        for (text, color) in status.color_spans {
            spans.push(Span::styled(text, Style::default().fg(color)));
        }
        spans.push(Span::styled(
            format!(" [Esc to interrupt | {}]", time_str),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::from(spans));
    }
}

/// Build tool-only plan lines for the main message view.
/// Only shows steps that have actual tool activity - no empty steps.
pub fn build_tool_only_plan_lines(
    spec: &SpecSheet,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
    active_prefix: Option<&str>,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Only show if there's actual tool activity
    if step_tool_calls.is_empty() {
        return lines;
    }

    for step in &spec.steps {
        append_tool_only_step_lines(
            &mut lines,
            step,
            "",
            0,
            active_prefix,
            step_tool_calls,
            max_width,
        );
    }

    lines
}

/// Check if a step or any of its children have tool calls
fn step_has_tool_activity(
    step: &SpecStep,
    parent_prefix: &str,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
) -> bool {
    let prefix = compose_prefix(parent_prefix, &step.index);

    // Check if this step has tool calls
    if step_tool_calls.contains_key(&prefix) {
        return true;
    }

    // Check children
    if let Some(sub_spec) = &step.sub_spec {
        for child in &sub_spec.steps {
            if step_has_tool_activity(child, &prefix, step_tool_calls) {
                return true;
            }
        }
    }

    false
}

fn append_tool_only_step_lines(
    lines: &mut Vec<Line<'static>>,
    step: &SpecStep,
    parent_prefix: &str,
    depth: usize,
    active_prefix: Option<&str>,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
    max_width: usize,
) {
    let prefix = compose_prefix(parent_prefix, &step.index);

    // Skip steps without any tool activity (including children)
    if !step_has_tool_activity(step, parent_prefix, step_tool_calls) {
        return;
    }

    let indent = "  ".repeat(depth);
    let mut style = style_for_step(step.status);

    // Highlight active step
    if let Some(active) = active_prefix {
        if active == prefix {
            style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
        }
    }

    let bullet = "●";

    let base_text = if step.title.trim().is_empty() {
        step.instructions.clone()
    } else {
        step.title.clone()
    };
    let reserved = indent.len() + 4;
    let available = max_width.saturating_sub(reserved);
    let content = trim_to_width(&base_text, available);

    lines.push(Line::from(vec![
        Span::raw(format!("{}{} ", indent, bullet)),
        Span::styled(content, style),
    ]));

    // Show tool calls under this step
    if let Some(entries) = step_tool_calls.get(&prefix) {
        for (i, entry) in entries.iter().enumerate() {
            let entry_indent = "  ".repeat(depth);
            let is_last = i == entries.len() - 1;
            let connector = if is_last { "└ " } else { "│ " };
            let available = max_width.saturating_sub(entry_indent.len() + 6);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}│ {} ", entry_indent, connector),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format_tool_label(entry, available),
                    style_for_tool(entry.status),
                ),
            ]));
        }
    }

    // Process sub-spec steps recursively
    if let Some(sub_spec) = &step.sub_spec {
        for child in &sub_spec.steps {
            append_tool_only_step_lines(
                lines,
                child,
                &prefix,
                depth + 1,
                active_prefix,
                step_tool_calls,
                max_width,
            );
        }
    }
}

pub fn build_spec_plan_lines(
    spec: &SpecSheet,
    orchestrator_paused: bool,
    selected_index: usize,
    show_history: bool,
    step_drawer_open: bool,
    orchestrator_history: &[TaskSummary],
    latest_summaries: &HashMap<String, TaskSummary>,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
    active_prefix: Option<&str>,
    include_metadata: bool,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if include_metadata {
        let paused_badge = if orchestrator_paused { " [PAUSED]" } else { "" };
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

    if show_history {
        lines.extend(build_history_lines(orchestrator_history));
        return lines;
    }

    if spec.steps.is_empty() {
        lines.push(Line::from("No steps in this spec."));
        return lines;
    }

    let bounded_index = selected_index.min(spec.steps.len().saturating_sub(1));
    let selected_prefix = spec.steps.get(bounded_index).map(|step| step.index.clone());

    for step in &spec.steps {
        append_step_lines(
            &mut lines,
            step,
            "",
            0,
            selected_prefix.as_deref(),
            step_drawer_open && include_metadata,
            active_prefix,
            step_tool_calls,
            latest_summaries,
            max_width,
        );
    }

    lines
}

fn append_step_lines(
    lines: &mut Vec<Line<'static>>,
    step: &SpecStep,
    parent_prefix: &str,
    depth: usize,
    selected_prefix: Option<&str>,
    drawer_open: bool,
    active_prefix: Option<&str>,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
    latest_summaries: &HashMap<String, TaskSummary>,
    max_width: usize,
) {
    let prefix = compose_prefix(parent_prefix, &step.index);
    let indent = "  ".repeat(depth);
    let mut style = style_for_step(step.status);
    if let Some(active) = active_prefix {
        if active == prefix {
            style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
        }
    }
    let mut branch_selected = false;
    if let Some(selected) = selected_prefix {
        if prefix.starts_with(selected) {
            style = style.add_modifier(Modifier::ITALIC);
            branch_selected = prefix == selected;
        }
    }

    let base_text = if step.title.trim().is_empty() {
        step.instructions.clone()
    } else {
        step.title.clone()
    };
    let reserved = indent.len() + 4;
    let available = max_width.saturating_sub(reserved);
    let content = trim_to_width(&base_text, available);

    lines.push(Line::from(vec![
        Span::raw(format!("{}{} ", indent, step_status_icon(step.status))),
        Span::styled(content, style),
    ]));

    if drawer_open && branch_selected {
        append_drawer_lines(lines, depth + 1, step, latest_summaries, max_width);
    }

    if let Some(entries) = step_tool_calls.get(&prefix) {
        for entry in entries {
            let entry_indent = "  ".repeat(depth + 1);
            let available = max_width.saturating_sub(entry_indent.len() + 6);
            lines.push(Line::from(vec![
                Span::raw(format!(
                    "{}└ {} ",
                    entry_indent,
                    tool_status_icon(entry.status)
                )),
                Span::styled(
                    format_tool_label(entry, available),
                    style_for_tool(entry.status),
                ),
            ]));
        }
    }

    if let Some(sub_spec) = &step.sub_spec {
        for child in &sub_spec.steps {
            append_step_lines(
                lines,
                child,
                &prefix,
                depth + 1,
                selected_prefix,
                drawer_open,
                active_prefix,
                step_tool_calls,
                latest_summaries,
                max_width,
            );
        }
    }
}

fn append_drawer_lines(
    lines: &mut Vec<Line<'static>>,
    depth: usize,
    step: &SpecStep,
    latest_summaries: &HashMap<String, TaskSummary>,
    max_width: usize,
) {
    let indent = "  ".repeat(depth);
    let available = max_width.saturating_sub(indent.len() + 2);
    let mut pushed = false;

    if !step.instructions.trim().is_empty() {
        lines.push(Line::from(vec![Span::styled(
            format!("{}Instructions:", indent),
            Style::default().fg(Color::Gray),
        )]));
        for chunk in step
            .instructions
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}↳ ", indent),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    trim_to_width(chunk.trim(), available),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        pushed = true;
    }

    if !step.acceptance_criteria.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            format!("{}Acceptance criteria:", indent),
            Style::default().fg(Color::Gray),
        )]));
        for criterion in &step.acceptance_criteria {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}• ", indent),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    trim_to_width(criterion, available),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        pushed = true;
    }

    if !step.dependencies.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}Depends on: ", indent),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                trim_to_width(&step.dependencies.join(", "), available),
                Style::default().fg(Color::White),
            ),
        ]));
        pushed = true;
    }

    if let Some(summary) = latest_summaries.get(&step.index) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}Latest: ", indent),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                trim_to_width(&summary.summary_text, available),
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
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}Tests: ", indent),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    trim_to_width(&tests, available),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        if !summary.artifacts_touched.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}Artifacts: ", indent),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    trim_to_width(&summary.artifacts_touched.join(", "), available),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}Verification: ", indent),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{:?}", summary.verification.status),
                Style::default().fg(Color::White),
            ),
        ]));
        for feedback in &summary.verification.feedback {
            let message = format!("{}: {}", feedback.author, feedback.message);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}• ", indent),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    trim_to_width(&message, available),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        pushed = true;
    }

    if !pushed {
        lines.push(Line::from(vec![Span::styled(
            format!("{}No additional details yet.", indent),
            Style::default().fg(Color::DarkGray),
        )]));
    }
}

fn build_history_lines(history: &[TaskSummary]) -> Vec<Line<'static>> {
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

fn format_tool_label(entry: &StepToolCallEntry, available: usize) -> String {
    let role_prefix = match entry.role {
        SessionRole::Implementor => "",
        SessionRole::Summarizer => "[Summarize] ",
        SessionRole::Verifier => "[Verifier] ",
        SessionRole::Merge => "[Merge] ",
    };
    trim_to_width(&format!("{}{}", role_prefix, entry.label), available)
}

fn compose_prefix(parent: &str, index: &str) -> String {
    if parent.is_empty() {
        index.to_string()
    } else {
        format!("{}.{}", parent, index)
    }
}

fn step_status_icon(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "○",
        StepStatus::InProgress => "◐",
        StepStatus::Completed => "●",
        StepStatus::Failed => "✗",
    }
}

fn tool_status_icon(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Started => "◐",
        ToolCallStatus::Completed => "●",
        ToolCallStatus::Error => "✗",
    }
}

fn style_for_step(status: StepStatus) -> Style {
    match status {
        StepStatus::Pending => Style::default().fg(Color::DarkGray),
        StepStatus::InProgress => Style::default().fg(Color::Yellow),
        StepStatus::Completed => Style::default().fg(Color::Green),
        StepStatus::Failed => Style::default().fg(Color::Red),
    }
}

fn style_for_tool(status: ToolCallStatus) -> Style {
    match status {
        ToolCallStatus::Started => Style::default().fg(Color::Yellow),
        ToolCallStatus::Completed => Style::default().fg(Color::Green),
        ToolCallStatus::Error => Style::default().fg(Color::Red),
    }
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + ch_width > max_width {
            result.push('…');
            break;
        }
        result.push(ch);
        width += ch_width;
    }
    if result.is_empty() {
        text.chars().take(max_width).collect()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_tool_only_plan_lines, compose_tool_plan_view_lines, OrchestrationStatusLine,
    };
    use crate::{SessionRole, StepToolCallEntry, ToolCallStatus};
    use agent_core::{SpecSheet, SpecStep, StepStatus};
    use chrono::{TimeZone, Utc};
    use ratatui::{
        style::Color,
        text::{Line, Span},
    };
    use std::collections::HashMap;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn sample_spec() -> SpecSheet {
        SpecSheet {
            id: "spec-1".to_string(),
            title: "Demo Spec".to_string(),
            description: "desc".to_string(),
            steps: vec![
                SpecStep {
                    index: "1".to_string(),
                    title: "No tools".to_string(),
                    instructions: String::new(),
                    acceptance_criteria: vec![],
                    required_tools: vec![],
                    constraints: vec![],
                    is_parallel: false,
                    requires_verification: false,
                    max_parallelism: None,
                    status: StepStatus::Pending,
                    dependencies: vec![],
                    sub_spec: None,
                    completed_at: None,
                },
                SpecStep {
                    index: "2".to_string(),
                    title: "With tools".to_string(),
                    instructions: String::new(),
                    acceptance_criteria: vec![],
                    required_tools: vec![],
                    constraints: vec![],
                    is_parallel: false,
                    requires_verification: false,
                    max_parallelism: None,
                    status: StepStatus::InProgress,
                    dependencies: vec![],
                    sub_spec: None,
                    completed_at: None,
                },
            ],
            created_by: "tester".to_string(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn build_tool_only_plan_lines_omits_steps_without_activity() {
        let mut tool_calls = HashMap::new();
        tool_calls.insert(
            "2".to_string(),
            vec![StepToolCallEntry {
                id: 1,
                label: "run tests".to_string(),
                status: ToolCallStatus::Started,
                role: SessionRole::Implementor,
                worktree_branch: None,
                worktree_path: None,
            }],
        );

        let lines = build_tool_only_plan_lines(&sample_spec(), &tool_calls, Some("2"), 80);
        let combined = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(combined.contains("With tools"));
        assert!(!combined.contains("No tools"));
    }

    #[test]
    fn compose_tool_plan_view_lines_adds_gap_plan_and_status_line() {
        let mut lines = vec![Line::from("existing")];
        let plan_lines = vec![Line::from(vec![Span::raw("plan")])];

        compose_tool_plan_view_lines(
            &mut lines,
            plan_lines,
            Some(OrchestrationStatusLine {
                current_frame: "*".to_string(),
                color_spans: vec![("thinking...".to_string(), Color::Yellow)],
                elapsed_secs: 65,
            }),
        );

        assert_eq!(line_text(&lines[1]), " ");
        assert_eq!(line_text(&lines[2]), "plan");
        assert!(line_text(lines.last().unwrap()).contains("[Esc to interrupt | 1m 05s]"));
    }
}
