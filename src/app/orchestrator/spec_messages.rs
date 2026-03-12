use agent_core::{SpecSheet, StepStatus, TaskSummary, VerificationStatus};

pub(crate) fn format_spec_overview(spec: &SpecSheet) -> String {
    let mut status_lines = vec![format!("Spec: {} ({})", spec.title, spec.id)];
    status_lines.push(format!("Description: {}", spec.description));
    status_lines.push(format!("Steps: {}", spec.steps.len()));
    for step in &spec.steps {
        let status_icon = match step.status {
            StepStatus::Pending => "○",
            StepStatus::InProgress => "◐",
            StepStatus::Completed => "●",
            StepStatus::Failed => "✗",
        };
        status_lines.push(format!("  {} {} - {}", status_icon, step.index, step.title));
    }

    format!("[SPEC]\n{}", status_lines.join("\n"))
}

pub(crate) fn format_split_injected(index: &str, child_spec: &SpecSheet) -> String {
    let summary = child_spec
        .steps
        .iter()
        .map(|s| format!("  {} - {}", s.index, s.title))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "[SPEC] Injected split {} ({} steps)\n{}",
        index,
        child_spec.steps.len(),
        summary
    )
}

pub(crate) fn format_history(entries: &[TaskSummary]) -> String {
    let mut history_lines = vec!["[SPEC HISTORY]".to_string()];
    for summary in entries {
        let status_icon = match summary.verification.status {
            VerificationStatus::Passed => "✓",
            VerificationStatus::Failed => "✗",
            VerificationStatus::Pending => "○",
        };
        history_lines.push(format!(
            "  {} Step {} · {}",
            status_icon, summary.step_index, summary.summary_text
        ));
        if !summary.tests_run.is_empty() {
            let tests = summary
                .tests_run
                .iter()
                .map(|test| format!("{}({:?})", test.name, test.result))
                .collect::<Vec<_>>()
                .join(", ");
            history_lines.push(format!("    Tests: {}", tests));
        }
        if !summary.artifacts_touched.is_empty() {
            history_lines.push(format!(
                "    Artifacts: {}",
                summary.artifacts_touched.join(", ")
            ));
        }
        if !summary.verification.feedback.is_empty() {
            for feedback in &summary.verification.feedback {
                history_lines.push(format!(
                    "    Feedback {}: {}",
                    feedback.author, feedback.message
                ));
            }
        }
    }

    history_lines.join("\n")
}
