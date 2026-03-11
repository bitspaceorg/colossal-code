use super::{
    build_spec_plan_lines, build_tool_only_plan_lines, compose_tool_plan_view_lines,
    OrchestrationStatusLine, SpecPlanRenderParams,
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

#[test]
fn build_spec_plan_lines_applies_active_highlight_style() {
    let spec = sample_spec();
    let latest_summaries = HashMap::new();
    let tool_calls = HashMap::new();
    let lines = build_spec_plan_lines(
        &spec,
        SpecPlanRenderParams {
            orchestrator_paused: false,
            selected_index: 0,
            show_history: false,
            step_drawer_open: false,
            orchestrator_history: &[],
            latest_summaries: &latest_summaries,
            step_tool_calls: &tool_calls,
            active_prefix: Some("1"),
            include_metadata: false,
            max_width: 80,
        },
    );

    let style = lines[0].spans[1].style;
    assert_eq!(style.fg, Some(Color::Cyan));
    assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn build_spec_plan_lines_applies_selected_branch_italic_style() {
    let spec = sample_spec();
    let latest_summaries = HashMap::new();
    let tool_calls = HashMap::new();
    let lines = build_spec_plan_lines(
        &spec,
        SpecPlanRenderParams {
            orchestrator_paused: false,
            selected_index: 0,
            show_history: false,
            step_drawer_open: false,
            orchestrator_history: &[],
            latest_summaries: &latest_summaries,
            step_tool_calls: &tool_calls,
            active_prefix: None,
            include_metadata: false,
            max_width: 80,
        },
    );

    let style = lines[0].spans[1].style;
    assert!(style
        .add_modifier
        .contains(ratatui::style::Modifier::ITALIC));
}

#[test]
fn build_spec_plan_lines_renders_history_when_enabled() {
    let spec = sample_spec();
    let latest_summaries = HashMap::new();
    let tool_calls = HashMap::new();

    let lines = build_spec_plan_lines(
        &spec,
        SpecPlanRenderParams {
            orchestrator_paused: false,
            selected_index: 0,
            show_history: true,
            step_drawer_open: false,
            orchestrator_history: &[],
            latest_summaries: &latest_summaries,
            step_tool_calls: &tool_calls,
            active_prefix: None,
            include_metadata: false,
            max_width: 80,
        },
    );

    assert_eq!(line_text(&lines[0]), "History: no entries yet.");
}
