use std::time::SystemTime;

use agent_core::{
    Agent, SpecSheet, SpecStep, StepStatus, TaskSummary, VerificationStatus,
    orchestrator::OrchestratorControl,
};
use anyhow::Result;
use async_trait::async_trait;

use crate::{MessageState, MessageType, UIMessageMetadata};

#[async_trait]
pub trait SpecAgentBridge: Send + Sync {
    async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet>;
    fn validate_step_index(&self, spec: &SpecSheet, index: &str) -> Result<()>;
    fn get_spec_status(&self, spec: &SpecSheet) -> Result<String>;
}

#[async_trait]
impl SpecAgentBridge for Agent {
    async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet> {
        Agent::request_split(self, step).await
    }

    fn validate_step_index(&self, spec: &SpecSheet, index: &str) -> Result<()> {
        Agent::validate_step_index(self, spec, index)
    }

    fn get_spec_status(&self, spec: &SpecSheet) -> Result<String> {
        Agent::get_spec_status(self, spec)
    }
}

pub enum SpecCommandResult {
    Handled,
    Unhandled,
}

pub struct MessageLog<'a> {
    pub messages: &'a mut Vec<String>,
    pub types: &'a mut Vec<MessageType>,
    pub states: &'a mut Vec<MessageState>,
    pub metadata: &'a mut Vec<Option<UIMessageMetadata>>,
    pub timestamps: &'a mut Vec<SystemTime>,
}

impl<'a> MessageLog<'a> {
    fn push(&mut self, message: String) {
        self.messages.push(message);
        self.types.push(MessageType::Agent);
        self.states.push(MessageState::Sent);
        self.metadata.push(None);
        self.timestamps.push(SystemTime::now());
    }
}

pub struct SpecCliContext<'a> {
    pub current_spec: &'a mut Option<SpecSheet>,
    pub orchestrator_control: Option<&'a OrchestratorControl>,
    pub orchestrator_history: &'a [TaskSummary],
    pub orchestrator_paused: &'a mut bool,
    pub status_message: &'a mut Option<String>,
    pub message_log: MessageLog<'a>,
}

pub struct SpecCliHandler<'a> {
    ctx: SpecCliContext<'a>,
}

impl<'a> SpecCliHandler<'a> {
    pub fn new(ctx: SpecCliContext<'a>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &mut self,
        agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
        command: &str,
    ) -> SpecCommandResult {
        let cmd_lower = command.to_lowercase();

        if cmd_lower == "/spec" {
            self.show_spec();
            return SpecCommandResult::Handled;
        }

        if cmd_lower.starts_with("/spec split ") {
            self.split_step(agent, command).await;
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec status" {
            self.show_spec_status(agent);
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec abort" {
            self.abort_run();
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec pause" {
            self.pause_run();
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec resume" {
            self.resume_run();
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec rerun" {
            self.rerun_verifiers();
            return SpecCommandResult::Handled;
        }

        if cmd_lower == "/spec history" {
            self.show_history();
            return SpecCommandResult::Handled;
        }

        SpecCommandResult::Unhandled
    }

    fn show_spec(&mut self) {
        if let Some(spec) = self.ctx.current_spec.as_ref() {
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
            self.ctx
                .message_log
                .push(format!("[SPEC]\n{}", status_lines.join("\n")));
        } else {
            self.ctx.message_log.push(
                "[SPEC] No spec loaded. Use /spec <path|goal> to load or create one.".to_string(),
            );
        }
    }

    async fn split_step(
        &mut self,
        agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
        command: &str,
    ) {
        let parts: Vec<&str> = command.split_whitespace().collect();
        let index = parts.get(2).map(|s| s.to_string());
        if index.is_none() {
            self.ctx
                .message_log
                .push("[SPEC ERROR] Usage: /spec split <index>".to_string());
            return;
        }

        let Some(spec) = self.ctx.current_spec.as_ref() else {
            self.ctx.message_log.push(
                "[SPEC ERROR] No spec loaded. Load a spec first with /spec <path|goal>".to_string(),
            );
            return;
        };

        let Some(agent) = agent else {
            self.ctx
                .message_log
                .push("[SPEC ERROR] Agent not initialized".to_string());
            return;
        };

        let index = index.unwrap();
        if let Err(e) = agent.validate_step_index(spec, &index) {
            self.ctx.message_log.push(format!("[SPEC ERROR] {}", e));
            return;
        }

        let Some(step) = spec.steps.iter().find(|s| s.index == index) else {
            self.ctx
                .message_log
                .push(format!("[SPEC ERROR] Step {} not found", index));
            return;
        };

        match agent.request_split(step).await {
            Ok(child_spec) => {
                if let Some(control) = self.ctx.orchestrator_control {
                    match control.inject_split(index.clone(), child_spec.clone()) {
                        Ok(()) => {
                            let summary = child_spec
                                .steps
                                .iter()
                                .map(|s| format!("  {} - {}", s.index, s.title))
                                .collect::<Vec<_>>()
                                .join("\n");
                            self.ctx.message_log.push(format!(
                                "[SPEC] Injected split {} ({} steps)\n{}",
                                index,
                                child_spec.steps.len(),
                                summary
                            ));
                        }
                        Err(e) => {
                            self.ctx
                                .message_log
                                .push(format!("[SPEC ERROR] Failed to inject split: {}", e));
                        }
                    }
                } else {
                    self.ctx.message_log.push(
                        "[SPEC ERROR] No orchestrator control available to inject split"
                            .to_string(),
                    );
                }
            }
            Err(e) => {
                self.ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to split step: {}", e));
            }
        }
    }

    fn show_spec_status(&mut self, agent: Option<&(dyn SpecAgentBridge + Send + Sync)>) {
        if let Some(spec) = self.ctx.current_spec.as_ref() {
            if let Some(agent) = agent {
                match agent.get_spec_status(spec) {
                    Ok(json) => self
                        .ctx
                        .message_log
                        .push(format!("[SPEC STATUS]\n```json\n{}\n```", json)),
                    Err(e) => self
                        .ctx
                        .message_log
                        .push(format!("[SPEC ERROR] Failed to serialize spec: {}", e)),
                }
            } else {
                self.ctx
                    .message_log
                    .push("[SPEC ERROR] Agent not initialized".to_string());
            }
        } else {
            self.ctx
                .message_log
                .push("[SPEC] No spec loaded.".to_string());
        }
    }

    fn abort_run(&mut self) {
        if let Some(control) = self.ctx.orchestrator_control {
            if let Err(e) = control.abort() {
                self.ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to abort: {}", e));
            } else {
                self.ctx
                    .message_log
                    .push("[SPEC] Abort signal sent to orchestrator.".to_string());
                *self.ctx.status_message = Some("Abort requested".to_string());
            }
        } else {
            self.ctx
                .message_log
                .push("[SPEC] No spec running to abort.".to_string());
        }
    }

    fn pause_run(&mut self) {
        if let Some(control) = self.ctx.orchestrator_control {
            if control.is_paused() {
                self.ctx
                    .message_log
                    .push("[SPEC] Orchestrator is already paused.".to_string());
            } else if let Err(e) = control.pause() {
                self.ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to pause: {}", e));
            } else {
                *self.ctx.orchestrator_paused = true;
                self.ctx
                    .message_log
                    .push("[SPEC] Orchestrator paused. Use /spec resume to continue.".to_string());
                *self.ctx.status_message = Some("Paused orchestrator".to_string());
            }
        } else {
            self.ctx
                .message_log
                .push("[SPEC] No orchestrator running to pause.".to_string());
        }
    }

    fn resume_run(&mut self) {
        if let Some(control) = self.ctx.orchestrator_control {
            if !control.is_paused() {
                self.ctx
                    .message_log
                    .push("[SPEC] Orchestrator is not paused.".to_string());
            } else if let Err(e) = control.resume() {
                self.ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to resume: {}", e));
            } else {
                *self.ctx.orchestrator_paused = false;
                self.ctx
                    .message_log
                    .push("[SPEC] Orchestrator resumed.".to_string());
                *self.ctx.status_message = Some("Resumed orchestrator".to_string());
            }
        } else {
            self.ctx
                .message_log
                .push("[SPEC] No orchestrator running to resume.".to_string());
        }
    }

    fn rerun_verifiers(&mut self) {
        if let Some(control) = self.ctx.orchestrator_control {
            if let Err(e) = control.rerun_verifiers() {
                self.ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to rerun verifiers: {}", e));
            } else {
                self.ctx
                    .message_log
                    .push("[SPEC] Rerunning verifiers on last step.".to_string());
                *self.ctx.status_message = Some("Re-running verifiers".to_string());
            }
        } else {
            self.ctx
                .message_log
                .push("[SPEC] No orchestrator running.".to_string());
        }
    }

    fn show_history(&mut self) {
        if self.ctx.orchestrator_history.is_empty() {
            self.ctx
                .message_log
                .push("[SPEC] No task history available.".to_string());
            return;
        }

        let mut history_lines = vec!["[SPEC HISTORY]".to_string()];
        for summary in self.ctx.orchestrator_history {
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
        self.ctx.message_log.push(history_lines.join("\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use agent_core::{StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus};
    use agent_protocol::types::spec::TestResult;
    use chrono::{TimeZone, Utc};
    use tokio::{runtime::Runtime, sync::mpsc};

    struct MockAgent {
        splits: HashMap<String, SpecSheet>,
        status_json: String,
        validations: HashMap<String, bool>,
    }

    #[async_trait]
    impl SpecAgentBridge for MockAgent {
        async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet> {
            self.splits
                .get(&step.index)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing split"))
        }

        fn validate_step_index(&self, _spec: &SpecSheet, index: &str) -> Result<()> {
            if self.validations.get(index).copied().unwrap_or(false) {
                Ok(())
            } else {
                Err(anyhow::anyhow!("invalid index"))
            }
        }

        fn get_spec_status(&self, _spec: &SpecSheet) -> Result<String> {
            Ok(self.status_json.clone())
        }
    }

    struct TestHarness {
        spec: Option<SpecSheet>,
        history: Vec<TaskSummary>,
        control: Option<OrchestratorControl>,
        paused: bool,
        status_message: Option<String>,
        messages: Vec<String>,
        types: Vec<MessageType>,
        states: Vec<MessageState>,
        metadata: Vec<Option<UIMessageMetadata>>,
        timestamps: Vec<SystemTime>,
    }

    impl TestHarness {
        fn new(
            spec: Option<SpecSheet>,
            history: Vec<TaskSummary>,
            control: Option<OrchestratorControl>,
        ) -> Self {
            Self {
                spec,
                history,
                control,
                paused: false,
                status_message: None,
                messages: Vec::new(),
                types: Vec::new(),
                states: Vec::new(),
                metadata: Vec::new(),
                timestamps: Vec::new(),
            }
        }

        fn run(&mut self, agent: Option<&(dyn SpecAgentBridge + Send + Sync)>, command: &str) {
            let mut handler = SpecCliHandler::new(SpecCliContext {
                current_spec: &mut self.spec,
                orchestrator_control: self.control.as_ref(),
                orchestrator_history: &self.history,
                orchestrator_paused: &mut self.paused,
                status_message: &mut self.status_message,
                message_log: MessageLog {
                    messages: &mut self.messages,
                    types: &mut self.types,
                    states: &mut self.states,
                    metadata: &mut self.metadata,
                    timestamps: &mut self.timestamps,
                },
            });
            Runtime::new()
                .unwrap()
                .block_on(handler.execute(agent, command));
        }
    }

    fn sample_spec() -> SpecSheet {
        SpecSheet {
            id: "spec-1".to_string(),
            title: "Demo Spec".to_string(),
            description: "desc".to_string(),
            steps: vec![SpecStep {
                index: "1".to_string(),
                title: "Do work".to_string(),
                instructions: String::new(),
                acceptance_criteria: vec![],
                required_tools: vec![],
                constraints: vec![],
                status: StepStatus::Pending,
                dependencies: vec![],
                sub_spec: None,
                completed_at: None,
            }],
            created_by: "tester".to_string(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            metadata: serde_json::Value::Null,
        }
    }

    fn history_entry() -> TaskSummary {
        TaskSummary {
            task_id: "task-1".to_string(),
            step_index: "1".to_string(),
            summary_text: "Completed work".to_string(),
            artifacts_touched: vec!["src/lib.rs".to_string()],
            tests_run: vec![TestRun {
                name: "cargo test".to_string(),
                result: TestResult::Pass,
                logs_path: None,
                duration_ms: Some(42),
            }],
            verification: TaskVerification {
                status: VerificationStatus::Passed,
                feedback: vec![],
            },
        }
    }

    #[test]
    fn show_spec_lists_steps() {
        let mut harness = TestHarness::new(Some(sample_spec()), vec![], None);
        harness.run(None, "/spec");
        assert!(harness.messages[0].contains("Demo Spec"));
        assert_eq!(harness.types.len(), 1);
    }

    #[test]
    fn split_injects_child_spec_through_control() {
        let mut child = sample_spec();
        child.id = "child".into();
        child.title = "Child".into();
        child.steps[0].title = "Detail".into();

        let mut validations = HashMap::new();
        validations.insert("1".to_string(), true);
        let mut splits = HashMap::new();
        splits.insert("1".to_string(), child.clone());
        let agent = MockAgent {
            splits,
            status_json: "{}".into(),
            validations,
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let control = OrchestratorControl::new(tx);
        let mut harness = TestHarness::new(Some(sample_spec()), vec![], Some(control));
        harness.run(Some(&agent), "/spec split 1");
        assert!(harness.messages[0].contains("Injected split 1"));
    }

    #[test]
    fn status_requires_agent() {
        let mut harness = TestHarness::new(Some(sample_spec()), vec![], None);
        harness.run(None, "/spec status");
        assert_eq!(harness.messages[0], "[SPEC ERROR] Agent not initialized");
    }

    #[test]
    fn status_uses_agent_snapshot() {
        let agent = MockAgent {
            splits: HashMap::new(),
            status_json: "{\"ok\":true}".into(),
            validations: HashMap::new(),
        };
        let mut harness = TestHarness::new(Some(sample_spec()), vec![], None);
        harness.run(Some(&agent), "/spec status");
        assert!(harness.messages[0].contains("\"ok\":true"));
    }

    #[test]
    fn pause_resume_and_rerun_update_state() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let control = OrchestratorControl::new(tx);
        let mut harness = TestHarness::new(None, vec![], Some(control));
        harness.run(None, "/spec pause");
        assert!(harness.messages[0].contains("paused"));
        assert!(harness.paused);

        harness.run(None, "/spec resume");
        assert!(harness.messages.last().unwrap().contains("resumed"));
        assert!(!harness.paused);

        harness.run(None, "/spec rerun");
        assert!(harness.messages.last().unwrap().contains("Rerunning"));
    }

    #[test]
    fn abort_without_control_reports_error() {
        let mut harness = TestHarness::new(None, vec![], None);
        harness.run(None, "/spec abort");
        assert_eq!(harness.messages[0], "[SPEC] No spec running to abort.");
    }

    #[test]
    fn history_renders_summaries() {
        let mut harness = TestHarness::new(Some(sample_spec()), vec![history_entry()], None);
        harness.run(None, "/spec history");
        assert!(harness.messages[0].contains("[SPEC HISTORY]"));
        assert!(harness.messages[0].contains("Artifacts"));
    }
}
