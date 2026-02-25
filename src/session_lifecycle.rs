use std::{collections::HashMap, time::Instant};

use agent_core::{orchestrator::StepRole, StepStatus};

use crate::session_manager::{OrchestratorEntry, SessionManager, SessionRole, SessionStatus};

pub(crate) fn update_session_for_step(
    orchestrator_sessions: &mut HashMap<String, OrchestratorEntry>,
    session_manager: &mut SessionManager,
    spec_id: &str,
    spec_title: &str,
    prefix: &str,
    step_index: &str,
    step_title: &str,
    status: StepStatus,
    role: StepRole,
) {
    let session_role = session_role_from_step_role(role);
    let entry = orchestrator_sessions
        .entry(prefix.to_string())
        .or_insert_with(|| OrchestratorEntry {
            spec_id: spec_id.to_string(),
            spec_title: spec_title.to_string(),
            prefix: prefix.to_string(),
            step_index: step_index.to_string(),
            step_title: step_title.to_string(),
            role: session_role.clone(),
            status: SessionStatus::Pending,
            started_at: None,
            completed_at: None,
            worktree_branch: None,
            worktree_path: None,
        });

    entry.role = session_role;
    entry.status = step_status_to_session_status(status);

    match status {
        StepStatus::InProgress => {
            if entry.started_at.is_none() {
                entry.started_at = Some(Instant::now());
            }
            entry.completed_at = None;
        }
        StepStatus::Completed | StepStatus::Failed => {
            if entry.started_at.is_none() {
                entry.started_at = Some(Instant::now());
            }
            entry.completed_at = Some(Instant::now());
        }
        StepStatus::Pending => {
            entry.started_at = None;
            entry.completed_at = None;
        }
    }

    let snapshot: Vec<OrchestratorEntry> = orchestrator_sessions.values().cloned().collect();
    session_manager.update_from_orchestrator(snapshot);
}

fn session_role_from_step_role(role: StepRole) -> SessionRole {
    match role {
        StepRole::Implementor => SessionRole::Implementor,
        StepRole::Summarizer => SessionRole::Summarizer,
        StepRole::Verifier => SessionRole::Verifier,
        StepRole::Merge => SessionRole::Merge,
    }
}

fn step_status_to_session_status(status: StepStatus) -> SessionStatus {
    match status {
        StepStatus::Pending => SessionStatus::Pending,
        StepStatus::InProgress => SessionStatus::InProgress,
        StepStatus::Completed => SessionStatus::Completed,
        StepStatus::Failed => SessionStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::update_session_for_step;
    use crate::session_manager::{SessionManager, SessionRole, SessionStatus};
    use agent_core::{orchestrator::StepRole, StepStatus};
    use std::collections::HashMap;

    #[test]
    fn creates_and_updates_session_entry_for_step_status() {
        let mut orchestrator_sessions = HashMap::new();
        let mut session_manager = SessionManager::new();

        update_session_for_step(
            &mut orchestrator_sessions,
            &mut session_manager,
            "spec-1",
            "Demo spec",
            "1",
            "1",
            "Implement feature",
            StepStatus::InProgress,
            StepRole::Implementor,
        );

        let entry = orchestrator_sessions.get("1").expect("entry exists");
        assert_eq!(entry.status, SessionStatus::InProgress);
        assert_eq!(entry.role, SessionRole::Implementor);
        assert!(entry.started_at.is_some());
        assert!(entry.completed_at.is_none());

        update_session_for_step(
            &mut orchestrator_sessions,
            &mut session_manager,
            "spec-1",
            "Demo spec",
            "1",
            "1",
            "Implement feature",
            StepStatus::Completed,
            StepRole::Implementor,
        );

        let completed_entry = orchestrator_sessions.get("1").expect("entry exists");
        assert_eq!(completed_entry.status, SessionStatus::Completed);
        assert!(completed_entry.started_at.is_some());
        assert!(completed_entry.completed_at.is_some());
    }

    #[test]
    fn pending_status_clears_session_timestamps() {
        let mut orchestrator_sessions = HashMap::new();
        let mut session_manager = SessionManager::new();

        update_session_for_step(
            &mut orchestrator_sessions,
            &mut session_manager,
            "spec-1",
            "Demo spec",
            "2",
            "2",
            "Verify feature",
            StepStatus::Failed,
            StepRole::Verifier,
        );

        update_session_for_step(
            &mut orchestrator_sessions,
            &mut session_manager,
            "spec-1",
            "Demo spec",
            "2",
            "2",
            "Verify feature",
            StepStatus::Pending,
            StepRole::Verifier,
        );

        let entry = orchestrator_sessions.get("2").expect("entry exists");
        assert_eq!(entry.status, SessionStatus::Pending);
        assert!(entry.started_at.is_none());
        assert!(entry.completed_at.is_none());
    }

    #[test]
    fn refreshes_session_manager_snapshot() {
        let mut orchestrator_sessions = HashMap::new();
        let mut session_manager = SessionManager::new();

        update_session_for_step(
            &mut orchestrator_sessions,
            &mut session_manager,
            "spec-1",
            "Demo spec",
            "3",
            "3",
            "Summarize",
            StepStatus::InProgress,
            StepRole::Summarizer,
        );

        let saved = session_manager
            .orchestrator_entries
            .iter()
            .find(|entry| entry.prefix == "3")
            .expect("session manager updated");

        assert_eq!(saved.status, SessionStatus::InProgress);
        assert_eq!(saved.role, SessionRole::Summarizer);
    }
}
