use std::collections::HashMap;

use agent_core::SpecSheet;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::{labels, plan_history, plan_lines};

pub use super::plan_lines::SpecPlanRenderParams;

const NO_STEPS_FALLBACK: &str = "No steps in this spec.";
const HISTORY_EMPTY_FALLBACK: &str = "History: no entries yet.";

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

pub fn build_tool_only_plan_lines(
    spec: &SpecSheet,
    step_tool_calls: &HashMap<String, Vec<crate::StepToolCallEntry>>,
    active_prefix: Option<&str>,
    max_width: usize,
) -> Vec<Line<'static>> {
    plan_lines::build_tool_only_plan_lines(spec, step_tool_calls, active_prefix, max_width)
}

pub fn build_spec_plan_lines(
    spec: &SpecSheet,
    params: SpecPlanRenderParams<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if params.include_metadata {
        labels::append_spec_metadata_lines(&mut lines, spec, params.orchestrator_paused);
    }

    if params.show_history {
        if params.orchestrator_history.is_empty() {
            lines.push(Line::from(HISTORY_EMPTY_FALLBACK.to_string()));
            return lines;
        }
        lines.extend(plan_history::build_history_lines(
            params.orchestrator_history,
        ));
        return lines;
    }

    if spec.steps.is_empty() {
        lines.push(Line::from(NO_STEPS_FALLBACK));
        return lines;
    }

    lines.extend(plan_lines::build_spec_step_lines(spec, &params));
    lines
}

#[cfg(test)]
#[path = "plan_view_tests.rs"]
mod tests;
