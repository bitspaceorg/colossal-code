//! TUI snapshot tests for the spec pane and session window layouts.

use std::collections::HashMap;

use agent_core::{
    SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus,
};
use agent_protocol::types::spec::TestResult;
use chrono::{TimeZone, Utc};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};

#[path = "../src/spec_ui.rs"]
mod spec_ui;

#[path = "../src/session_manager.rs"]
mod session_manager;

use session_manager::{OrchestratorEntry, SessionManager, SessionStatus};
use spec_ui::render_spec_pane_view;

fn buffer_to_string(buffer: &Buffer) -> String {
    let mut rows = Vec::new();
    for y in 0..buffer.area.height {
        let mut row = String::new();
        for x in 0..buffer.area.width {
            let symbol = buffer.get(x, y).symbol();
            row.push(symbol.chars().next().unwrap_or(' '));
        }
        rows.push(row);
    }
    rows.join("\n")
}

fn create_spec(step_count: usize) -> SpecSheet {
    let steps: Vec<SpecStep> = (1..=step_count)
        .map(|i| SpecStep {
            index: i.to_string(),
            title: format!("Step {}: Feature", i),
            instructions: format!("Implement part {}", i),
            acceptance_criteria: vec![format!("Criteria {}", i)],
            required_tools: vec![],
            constraints: vec![],
            status: match i {
                1 => StepStatus::Completed,
                2 => StepStatus::InProgress,
                _ => StepStatus::Pending,
            },
            dependencies: if i > 1 {
                vec![(i - 1).to_string()]
            } else {
                vec![]
            },
            sub_spec: None,
            completed_at: None,
        })
        .collect();

    SpecSheet {
        id: "spec-001".to_string(),
        title: "Snapshot Spec".to_string(),
        description: "Used in snapshot tests".to_string(),
        steps,
        created_by: "tests".to_string(),
        created_at: Utc.timestamp_opt(0, 0).unwrap(),
        metadata: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn sample_summary(step_index: &str) -> TaskSummary {
    TaskSummary {
        task_id: format!("task-{}", step_index),
        step_index: step_index.to_string(),
        summary_text: format!("Completed step {}", step_index),
        artifacts_touched: vec![format!("file{}.rs", step_index)],
        tests_run: vec![TestRun {
            name: format!("test-step-{}", step_index),
            result: TestResult::Pass,
            logs_path: None,
            duration_ms: Some(50),
        }],
        verification: TaskVerification {
            status: VerificationStatus::Passed,
            feedback: vec![],
        },
    }
}

fn draw_spec_pane(
    spec: &SpecSheet,
    history: &[TaskSummary],
    latest: &HashMap<String, TaskSummary>,
    show_history: bool,
    drawer_open: bool,
) -> Buffer {
    let backend = TestBackend::new(90, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut history_vec = history.to_vec();
    terminal
        .draw(|frame| {
            let area = frame.size();
            render_spec_pane_view(
                frame,
                area,
                spec,
                false,
                1,
                show_history,
                drawer_open,
                &history_vec,
                latest,
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn spec_pane_metadata_and_steps_snapshot() {
    let spec = create_spec(3);
    let mut latest = HashMap::new();
    let summary = sample_summary("2");
    latest.insert("2".to_string(), summary);
    let buffer = draw_spec_pane(&spec, &[], &latest, false, true);
    let output = buffer_to_string(&buffer);
    insta::assert_snapshot!("spec_pane_full", output);
}

#[test]
fn spec_pane_history_snapshot() {
    let spec = create_spec(3);
    let history = vec![sample_summary("1"), sample_summary("2")];
    let latest = HashMap::new();
    let buffer = draw_spec_pane(&spec, &history, &latest, true, false);
    let output = buffer_to_string(&buffer);
    insta::assert_snapshot!("spec_pane_history", output);
}

fn draw_session_window(entries: Vec<OrchestratorEntry>) -> Buffer {
    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut manager = SessionManager::new();
    manager.update_from_orchestrator(entries);
    terminal
        .draw(|frame| {
            manager.render(frame, frame.size());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn session_window_snapshot() {
    let entries = vec![
        OrchestratorEntry {
            spec_id: "spec-001".into(),
            spec_title: "Snapshot Spec".into(),
            prefix: "1".into(),
            step_index: "1".into(),
            step_title: "Root planning".into(),
            status: SessionStatus::InProgress,
            started_at: None,
            completed_at: None,
        },
        OrchestratorEntry {
            spec_id: "spec-001".into(),
            spec_title: "Snapshot Spec".into(),
            prefix: "1.1".into(),
            step_index: "1.1".into(),
            step_title: "Child task".into(),
            status: SessionStatus::Pending,
            started_at: None,
            completed_at: None,
        },
    ];
    let buffer = draw_session_window(entries);
    let output = buffer_to_string(&buffer);
    insta::assert_snapshot!("session_window", output);
}
