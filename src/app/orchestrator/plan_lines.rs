use std::collections::HashMap;

use agent_core::{SpecSheet, SpecStep, TaskSummary};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::StepToolCallEntry;

use super::plan_styles::{
    compose_prefix, format_tool_label, step_status_icon, style_for_step, style_for_tool,
    tool_status_icon, trim_to_width,
};

pub struct SpecPlanRenderParams<'a> {
    pub orchestrator_paused: bool,
    pub selected_index: usize,
    pub show_history: bool,
    pub step_drawer_open: bool,
    pub orchestrator_history: &'a [TaskSummary],
    pub latest_summaries: &'a HashMap<String, TaskSummary>,
    pub step_tool_calls: &'a HashMap<String, Vec<StepToolCallEntry>>,
    pub active_prefix: Option<&'a str>,
    pub include_metadata: bool,
    pub max_width: usize,
}

struct StepRenderContext<'a> {
    selected_prefix: Option<&'a str>,
    drawer_open: bool,
    active_prefix: Option<&'a str>,
    step_tool_calls: &'a HashMap<String, Vec<StepToolCallEntry>>,
    latest_summaries: &'a HashMap<String, TaskSummary>,
    max_width: usize,
}

pub(crate) fn build_tool_only_plan_lines(
    spec: &SpecSheet,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
    active_prefix: Option<&str>,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

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

pub(crate) fn build_spec_step_lines(
    spec: &SpecSheet,
    params: &SpecPlanRenderParams<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if spec.steps.is_empty() {
        lines.push(Line::from("No steps in this spec."));
        return lines;
    }

    let bounded_index = params.selected_index.min(spec.steps.len().saturating_sub(1));
    let selected_prefix = spec.steps.get(bounded_index).map(|step| step.index.clone());
    let context = StepRenderContext {
        selected_prefix: selected_prefix.as_deref(),
        drawer_open: params.step_drawer_open && params.include_metadata,
        active_prefix: params.active_prefix,
        step_tool_calls: params.step_tool_calls,
        latest_summaries: params.latest_summaries,
        max_width: params.max_width,
    };

    for step in &spec.steps {
        append_step_lines(&mut lines, step, "", 0, &context);
    }

    lines
}

fn step_has_tool_activity(
    step: &SpecStep,
    parent_prefix: &str,
    step_tool_calls: &HashMap<String, Vec<StepToolCallEntry>>,
) -> bool {
    let prefix = compose_prefix(parent_prefix, &step.index);

    if step_tool_calls.contains_key(&prefix) {
        return true;
    }

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

    if !step_has_tool_activity(step, parent_prefix, step_tool_calls) {
        return;
    }

    let indent = "  ".repeat(depth);
    let mut style = style_for_step(step.status);

    if let Some(active) = active_prefix
        && active == prefix
    {
        style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
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

fn append_step_lines(
    lines: &mut Vec<Line<'static>>,
    step: &SpecStep,
    parent_prefix: &str,
    depth: usize,
    context: &StepRenderContext<'_>,
) {
    let prefix = compose_prefix(parent_prefix, &step.index);
    let indent = "  ".repeat(depth);
    let mut style = style_for_step(step.status);
    if let Some(active) = context.active_prefix
        && active == prefix
    {
        style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
    }
    let mut branch_selected = false;
    if let Some(selected) = context.selected_prefix
        && prefix.starts_with(selected)
    {
        style = style.add_modifier(Modifier::ITALIC);
        branch_selected = prefix == selected;
    }

    let base_text = if step.title.trim().is_empty() {
        step.instructions.clone()
    } else {
        step.title.clone()
    };
    let reserved = indent.len() + 4;
    let available = context.max_width.saturating_sub(reserved);
    let content = trim_to_width(&base_text, available);

    lines.push(Line::from(vec![
        Span::raw(format!("{}{} ", indent, step_status_icon(step.status))),
        Span::styled(content, style),
    ]));

    if context.drawer_open && branch_selected {
        append_drawer_lines(
            lines,
            depth + 1,
            step,
            context.latest_summaries,
            context.max_width,
        );
    }

    if let Some(entries) = context.step_tool_calls.get(&prefix) {
        for entry in entries {
            let entry_indent = "  ".repeat(depth + 1);
            let available = context.max_width.saturating_sub(entry_indent.len() + 6);
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
            append_step_lines(lines, child, &prefix, depth + 1, context);
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
                Span::styled(format!("{}• ", indent), Style::default().fg(Color::DarkGray)),
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
            Span::styled(format!("{}Latest: ", indent), Style::default().fg(Color::Gray)),
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
                Span::styled(format!("{}Tests: ", indent), Style::default().fg(Color::Gray)),
                Span::styled(trim_to_width(&tests, available), Style::default().fg(Color::White)),
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
                Span::styled(format!("{}• ", indent), Style::default().fg(Color::DarkGray)),
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
