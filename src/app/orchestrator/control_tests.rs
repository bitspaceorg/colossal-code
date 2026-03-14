#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::SystemTime};

    use agent_core::{
        SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun,
        VerificationStatus, orchestrator::OrchestratorControl,
    };
    use agent_protocol::types::spec::TestResult;
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use tokio::{runtime::Runtime, sync::mpsc};

    use crate::app::orchestrator::control::{
        MessageLog, SpecAgentBridge, SpecCliContext, SpecCliHandler,
    };
    use crate::app::{MessageState, MessageType, UIMessageMetadata};

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
                is_parallel: false,
                requires_verification: false,
                max_parallelism: None,
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
            worktree: None,
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
