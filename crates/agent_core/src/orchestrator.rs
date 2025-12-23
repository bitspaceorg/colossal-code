use std::sync::Arc;

use agent_protocol::types::{
    spec::{
        FeedbackEntry, SpecSheet, SpecStep, StepStatus, TaskSummary, VerificationStatus,
    },
    task::{Task, TaskState},
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{map::Entry, Map, Value};

use crate::Agent;

#[async_trait]
pub trait OrchestratorAgent: Send + Sync {
    async fn update_spec_status(
        &self,
        spec: &SpecSheet,
        step: &SpecStep,
        prefix: &str,
    ) -> Result<()>;

    async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task>;

    async fn update_task_summary(&self, summary: &TaskSummary) -> Result<()>;

    async fn send_task_message(&self, task_id: &str, message: &str) -> Result<()>;

    async fn notify_step_success(&self, summary: &TaskSummary) -> Result<()>;

    async fn close_task_channel(&self, task_id: &str) -> Result<()>;
}

#[async_trait]
pub trait Verifier: Send + Sync {
    async fn verify(&self, summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry>;
}

pub struct VerifierChain {
    verifiers: Vec<Box<dyn Verifier>>,
}

impl VerifierChain {
    pub fn new(verifiers: Vec<Box<dyn Verifier>>) -> Self {
        Self { verifiers }
    }

    pub fn default_chain() -> Self {
        Self::new(vec![
            Box::new(CommandVerifier::default()),
            Box::new(LintVerifier::default()),
            Box::new(PolicyVerifier::default()),
        ])
    }

    pub async fn run(
        &self,
        summary: &TaskSummary,
    ) -> std::result::Result<(), FeedbackEntry> {
        for verifier in &self.verifiers {
            if let Err(feedback) = verifier.verify(summary).await {
                return Err(feedback);
            }
        }

        Ok(())
    }
}

impl Default for VerifierChain {
    fn default() -> Self {
        Self::default_chain()
    }
}

impl From<Vec<Box<dyn Verifier>>> for VerifierChain {
    fn from(value: Vec<Box<dyn Verifier>>) -> Self {
        Self::new(value)
    }
}

pub struct Orchestrator {
    main_agent: Arc<dyn OrchestratorAgent>,
    sub_agent_factory:
        Arc<dyn Fn(&SpecStep) -> Arc<dyn OrchestratorAgent> + Send + Sync>,
    verifier_chain: VerifierChain,
    stack: Vec<(SpecSheet, usize, String)>,
}

impl Orchestrator {
    pub fn new(
        main_agent: Arc<dyn OrchestratorAgent>,
        sub_agent_factory:
            Arc<dyn Fn(&SpecStep) -> Arc<dyn OrchestratorAgent> + Send + Sync>,
        verifier_chain: VerifierChain,
        spec: SpecSheet,
    ) -> Self {
        Self {
            main_agent,
            sub_agent_factory,
            verifier_chain,
            stack: vec![(spec, 0, String::new())],
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while let Some((mut spec, step_idx, prefix)) = self.stack.pop() {
            if step_idx >= spec.steps.len() {
                continue;
            }

            let current_prefix = Self::compose_prefix(&prefix, &spec.steps[step_idx].index);

            {
                let entry = spec
                    .steps
                    .get_mut(step_idx)
                    .expect("step index validated above");
                entry.status = StepStatus::InProgress;
                entry.completed_at = None;
            }

            let in_progress_step = spec.steps[step_idx].clone();
            self
                .main_agent
                .update_spec_status(&spec, &in_progress_step, &current_prefix)
                .await?;

            let sub_agent = (self.sub_agent_factory)(&in_progress_step);
            let task = sub_agent.execute_step(in_progress_step.clone(), &spec).await?;
            if task.status.state == TaskState::Submitted {
                if let Some(child_spec) = Self::extract_sub_spec(&task)? {
                    if let Some(entry) = spec.steps.get_mut(step_idx) {
                        entry.status = StepStatus::Pending;
                        entry.completed_at = None;
                        entry.sub_spec = Some(Box::new(child_spec.clone()));
                    }
                    self.stack.push((spec, step_idx, prefix));
                    self.stack.push((child_spec, 0, current_prefix.clone()));
                    continue;
                }
            }

            match self.verify_and_feedback(&task).await? {
                StepDisposition::Retry => {
                    if let Some(entry) = spec.steps.get_mut(step_idx) {
                        entry.status = StepStatus::Pending;
                        entry.completed_at = None;
                    }
                    let pending_step = spec
                        .steps
                        .get(step_idx)
                        .cloned()
                        .expect("step index validated above");
                    self
                        .main_agent
                        .update_spec_status(&spec, &pending_step, &current_prefix)
                        .await?;
                    self.stack.push((spec, step_idx, prefix));
                    continue;
                }
                StepDisposition::Success(summary) => {
                    let child_spec = Self::extract_sub_spec(&task)?;
                    if let Some(ref sub_spec) = child_spec {
                        spec.steps[step_idx].sub_spec = Some(Box::new(sub_spec.clone()));
                    }

                    spec.steps[step_idx].status = StepStatus::Completed;
                    spec.steps[step_idx].completed_at = Some(Utc::now());
                    Self::append_summary_to_history(&mut spec, &summary)?;

                    let completed_step = spec.steps[step_idx].clone();
                    self
                        .main_agent
                        .update_spec_status(&spec, &completed_step, &current_prefix)
                        .await?;

                    let next_idx = step_idx + 1;

                    if next_idx < spec.steps.len() {
                        self.stack.push((spec, next_idx, prefix));
                    }

                    if let Some(child) = child_spec {
                        self.stack.push((child, 0, current_prefix.clone()));
                    }
                }
            }
        }

        Ok(())
    }

    async fn verify_and_feedback(&self, task: &Task) -> Result<StepDisposition> {
        let mut summary = Self::extract_summary(task)?;
        summary.verification.status = VerificationStatus::Pending;
        self.main_agent.update_task_summary(&summary).await?;

        match self.verifier_chain.run(&summary).await {
            Ok(()) => {
                summary.verification.status = VerificationStatus::Passed;
                self.main_agent.update_task_summary(&summary).await?;
                self
                    .main_agent
                    .close_task_channel(&summary.task_id)
                    .await?;
                self.main_agent.notify_step_success(&summary).await?;
                Ok(StepDisposition::Success(summary))
            }
            Err(mut feedback) => {
                feedback.timestamp = Utc::now();
                summary.verification.status = VerificationStatus::Failed;
                summary.verification.feedback.push(feedback.clone());
                self.main_agent.update_task_summary(&summary).await?;
                let failure_message = format!(
                    "Verification failed for task {} step {}: {}",
                    summary.task_id, summary.step_index, feedback.message
                );
                self
                    .main_agent
                    .send_task_message(&summary.task_id, &failure_message)
                    .await?;
                Ok(StepDisposition::Retry)
            }
        }
    }

    fn extract_summary(task: &Task) -> Result<TaskSummary> {
        if let Some(metadata) = &task.metadata {
            return metadata
                .summary()
                .context("failed to deserialize task summary")?
                .ok_or_else(|| anyhow!("task {} missing summary metadata", task.id));
        }

        Err(anyhow!("task {} missing metadata", task.id))
    }

    fn extract_sub_spec(task: &Task) -> Result<Option<SpecSheet>> {
        if let Some(metadata) = &task.metadata {
            return metadata
                .spec_sheet()
                .context("failed to deserialize nested spec");
        }

        Ok(None)
    }

    fn append_summary_to_history(spec: &mut SpecSheet, summary: &TaskSummary) -> Result<()> {
        let summary_value = serde_json::to_value(summary)?;
        if !spec.metadata.is_object() {
            spec.metadata = Value::Object(Map::new());
        }

        let metadata = spec
            .metadata
            .as_object_mut()
            .expect("metadata converted to object above");

        match metadata.entry("history".to_string()) {
            Entry::Vacant(slot) => {
                slot.insert(Value::Array(vec![summary_value]));
            }
            Entry::Occupied(mut slot) => {
                if let Value::Array(items) = slot.get_mut() {
                    items.push(summary_value);
                } else {
                    let previous = slot.insert(Value::Array(Vec::new()));
                    if let Value::Array(items) = slot.get_mut() {
                        items.push(previous);
                        items.push(summary_value);
                    }
                }
            }
        }

        Ok(())
    }

fn compose_prefix(prefix: &str, index: &str) -> String {
        if prefix.is_empty() {
            index.to_string()
        } else {
            format!("{}.{}", prefix, index)
        }
    }
}

enum StepDisposition {
    Retry,
    Success(TaskSummary),
}

#[async_trait]
impl OrchestratorAgent for Agent {
    async fn update_spec_status(
        &self,
        spec: &SpecSheet,
        step: &SpecStep,
        prefix: &str,
    ) -> Result<()> {
        self.update_spec_status(spec, step, prefix).await
    }

    async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
        self.execute_step(step, spec).await
    }

    async fn update_task_summary(&self, summary: &TaskSummary) -> Result<()> {
        self.update_task_summary(summary).await
    }

    async fn send_task_message(&self, task_id: &str, message: &str) -> Result<()> {
        self.send_task_message(task_id, message).await
    }

    async fn notify_step_success(&self, summary: &TaskSummary) -> Result<()> {
        self.notify_step_success(summary).await
    }

    async fn close_task_channel(&self, task_id: &str) -> Result<()> {
        self.close_task_channel(task_id).await
    }
}

#[derive(Default)]
struct CommandVerifier;

#[async_trait]
impl Verifier for CommandVerifier {
    async fn verify(&self, _summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
        Ok(())
    }
}

#[derive(Default)]
struct LintVerifier;

#[async_trait]
impl Verifier for LintVerifier {
    async fn verify(&self, _summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
        Ok(())
    }
}

#[derive(Default)]
struct PolicyVerifier;

#[async_trait]
impl Verifier for PolicyVerifier {
    async fn verify(&self, _summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::types::{
        spec::{SpecSheet, SpecStep, StepStatus, TaskVerification, VerificationStatus},
        task::{Task, TaskMetadata, TaskState, TaskStatus},
    };
    use anyhow::anyhow;
    use chrono::Utc;
    use serde_json::Value;
    use std::{collections::VecDeque, sync::Mutex};

    fn make_step(index: &str, sub_spec: Option<SpecSheet>) -> SpecStep {
        SpecStep {
            index: index.to_string(),
            title: format!("Step {index}"),
            instructions: "Do things".to_string(),
            acceptance_criteria: vec![],
            required_tools: vec![],
            constraints: vec![],
            dependencies: vec![],
            status: StepStatus::Pending,
            sub_spec: sub_spec.map(Box::new),
            completed_at: None,
        }
    }

    fn make_spec(id: &str, steps: Vec<SpecStep>) -> SpecSheet {
        SpecSheet {
            id: id.to_string(),
            title: format!("Spec {id}"),
            description: "Spec".to_string(),
            steps,
            created_by: "tester".to_string(),
            created_at: Utc::now(),
            metadata: Value::Null,
        }
    }

    fn make_task(task_id: &str, step_index: &str, sub_spec: Option<SpecSheet>) -> Task {
        let summary = TaskSummary {
            task_id: task_id.to_string(),
            step_index: step_index.to_string(),
            summary_text: format!("Summary {step_index}"),
            artifacts_touched: vec![],
            tests_run: vec![],
            verification: TaskVerification {
                status: VerificationStatus::Pending,
                feedback: vec![],
            },
        };
        let mut metadata = TaskMetadata::default();
        metadata.summary = Some(serde_json::to_value(summary.clone()).unwrap());
        metadata.spec_sheet = sub_spec.map(|spec| serde_json::to_value(spec).unwrap());

        Task {
            id: task_id.to_string(),
            context_id: None,
            status: TaskStatus {
                state: TaskState::Completed,
                timestamp: Some(Utc::now()),
                message: None,
                error: None,
            },
            messages: vec![],
            artifacts: vec![],
            history: None,
            metadata: Some(metadata),
        }
    }

    fn make_split_task(task_id: &str, step_index: &str, sub_spec: SpecSheet) -> Task {
        let mut metadata = TaskMetadata::default();
        metadata.spec_sheet = Some(serde_json::to_value(sub_spec).unwrap());
        let summary = TaskSummary {
            task_id: task_id.to_string(),
            step_index: step_index.to_string(),
            summary_text: format!("Step {step_index} split"),
            artifacts_touched: vec![],
            tests_run: vec![],
            verification: TaskVerification {
                status: VerificationStatus::Pending,
                feedback: vec![],
            },
        };
        metadata.summary = Some(serde_json::to_value(summary).unwrap());

        Task {
            id: task_id.to_string(),
            context_id: None,
            status: TaskStatus {
                state: TaskState::Submitted,
                timestamp: Some(Utc::now()),
                message: Some("split".to_string()),
                error: None,
            },
            messages: vec![],
            artifacts: vec![],
            history: None,
            metadata: Some(metadata),
        }
    }

    #[derive(Default)]
    struct MockMainAgent {
        spec_updates: Mutex<Vec<(String, StepStatus)>>,
        summaries: Mutex<Vec<TaskSummary>>,
        messages: Mutex<Vec<(String, String)>>,
        closed_channels: Mutex<Vec<String>>,
        successes: Mutex<Vec<TaskSummary>>,
    }

    impl MockMainAgent {
        fn in_progress_prefixes(&self) -> Vec<String> {
            self.spec_updates
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(prefix, status)| {
                    if matches!(status, StepStatus::InProgress) {
                        Some(prefix.clone())
                    } else {
                        None
                    }
                })
                .collect()
        }

        fn messages(&self) -> Vec<(String, String)> {
            self.messages.lock().unwrap().clone()
        }

        fn closed_channels(&self) -> Vec<String> {
            self.closed_channels.lock().unwrap().clone()
        }

        fn successes(&self) -> Vec<TaskSummary> {
            self.successes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl OrchestratorAgent for MockMainAgent {
        async fn update_spec_status(
            &self,
            _spec: &SpecSheet,
            step: &SpecStep,
            prefix: &str,
        ) -> Result<()> {
            self.spec_updates
                .lock()
                .unwrap()
                .push((prefix.to_string(), step.status));
            Ok(())
        }

        async fn execute_step(&self, _step: SpecStep, _spec: &SpecSheet) -> Result<Task> {
            Err(anyhow!("main agent does not execute steps"))
        }

        async fn update_task_summary(&self, summary: &TaskSummary) -> Result<()> {
            self.summaries.lock().unwrap().push(summary.clone());
            Ok(())
        }

        async fn send_task_message(&self, task_id: &str, message: &str) -> Result<()> {
            self.messages
                .lock()
                .unwrap()
                .push((task_id.to_string(), message.to_string()));
            Ok(())
        }

        async fn notify_step_success(&self, summary: &TaskSummary) -> Result<()> {
            self.successes.lock().unwrap().push(summary.clone());
            Ok(())
        }

        async fn close_task_channel(&self, task_id: &str) -> Result<()> {
            self.closed_channels
                .lock()
                .unwrap()
                .push(task_id.to_string());
            Ok(())
        }
    }

    struct MockSubAgent {
        tasks: Mutex<VecDeque<(String, Task)>>,
        executions: Mutex<Vec<String>>,
    }

    impl MockSubAgent {
        fn new(queue: VecDeque<(String, Task)>) -> Self {
            Self {
                tasks: Mutex::new(queue),
                executions: Mutex::new(Vec::new()),
            }
        }

        fn executions(&self) -> Vec<String> {
            self.executions.lock().unwrap().clone()
        }
    }

    struct SplitPlanSubAgent {
        child_spec: SpecSheet,
        executions: Mutex<Vec<String>>,
        split_used: Mutex<bool>,
    }

    impl SplitPlanSubAgent {
        fn new(child_spec: SpecSheet) -> Self {
            Self {
                child_spec,
                executions: Mutex::new(Vec::new()),
                split_used: Mutex::new(false),
            }
        }

        fn executions(&self) -> Vec<String> {
            self.executions.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl OrchestratorAgent for MockSubAgent {
        async fn update_spec_status(
            &self,
            _spec: &SpecSheet,
            _step: &SpecStep,
            _prefix: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
            let label = format!("{}:{}", spec.id, step.index);
            self.executions.lock().unwrap().push(label.clone());
            let mut tasks = self.tasks.lock().unwrap();
            let (expected_label, task) = tasks.pop_front().expect("no task for step");
            assert_eq!(expected_label, label, "unexpected execution order");
            Ok(task)
        }

        async fn update_task_summary(&self, _summary: &TaskSummary) -> Result<()> {
            Ok(())
        }

        async fn send_task_message(&self, _task_id: &str, _message: &str) -> Result<()> {
            Ok(())
        }

        async fn notify_step_success(&self, _summary: &TaskSummary) -> Result<()> {
            Ok(())
        }

        async fn close_task_channel(&self, _task_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OrchestratorAgent for SplitPlanSubAgent {
        async fn update_spec_status(
            &self,
            _spec: &SpecSheet,
            _step: &SpecStep,
            _prefix: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
            let label = format!("{}:{}", spec.id, step.index);
            self.executions.lock().unwrap().push(label.clone());
            let task = match label.as_str() {
                "root:1" => make_task("task-1", "1", None),
                "root:2" => {
                    let mut used = self.split_used.lock().unwrap();
                    if !*used {
                        *used = true;
                        make_split_task("task-split", "2", self.child_spec.clone())
                    } else {
                        make_task("task-2", "2", None)
                    }
                }
                "root.2:1" => make_task("child-1", "1", None),
                "root.2:2" => make_task("child-2", "2", None),
                "root:3" => make_task("task-3", "3", None),
                other => panic!("unexpected execution {other}"),
            };
            Ok(task)
        }

        async fn update_task_summary(&self, _summary: &TaskSummary) -> Result<()> {
            Ok(())
        }

        async fn send_task_message(&self, _task_id: &str, _message: &str) -> Result<()> {
            Ok(())
        }

        async fn notify_step_success(&self, _summary: &TaskSummary) -> Result<()> {
            Ok(())
        }

        async fn close_task_channel(&self, _task_id: &str) -> Result<()> {
            Ok(())
        }
    }

    struct AlwaysPassVerifier;

    #[async_trait]
    impl Verifier for AlwaysPassVerifier {
        async fn verify(&self, _summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
            Ok(())
        }
    }

    struct FailOnceVerifier {
        attempts: Mutex<u32>,
    }

    impl FailOnceVerifier {
        fn new() -> Self {
            Self {
                attempts: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl Verifier for FailOnceVerifier {
        async fn verify(&self, _summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
            let mut guard = self.attempts.lock().unwrap();
            if *guard == 0 {
                *guard += 1;
                Err(FeedbackEntry {
                    author: "verifier".to_string(),
                    message: "fail once".to_string(),
                    timestamp: Utc::now(),
                })
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn orchestrator_processes_stack_depth_first() {
        let child_spec = make_spec(
            "root.1",
            vec![make_step("1", None), make_step("2", None)],
        );
        let root_spec = make_spec(
            "root",
            vec![make_step("1", Some(child_spec.clone())), make_step("2", None)],
        );

        let tasks = VecDeque::from(vec![
            (
                "root:1".to_string(),
                make_task("task-1", "1", Some(child_spec.clone())),
            ),
            ("root.1:1".to_string(), make_task("task-1.1", "1", None)),
            ("root.1:2".to_string(), make_task("task-1.2", "2", None)),
            ("root:2".to_string(), make_task("task-2", "2", None)),
        ]);

        let main_agent = Arc::new(MockMainAgent::default());
        let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
        let sub_agent = Arc::new(MockSubAgent::new(tasks));
        let sub_agent_factory = {
            let sub_agent = sub_agent.clone();
            Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            })
        };

        let mut orchestrator = Orchestrator::new(
            main_agent_trait,
            sub_agent_factory,
            VerifierChain::new(vec![Box::new(AlwaysPassVerifier)]),
            root_spec,
        );

        orchestrator.run().await.unwrap();

        let prefixes = main_agent.in_progress_prefixes();
        assert_eq!(prefixes, vec!["1", "1.1", "1.2", "2"]);
        assert_eq!(sub_agent.executions(), vec!["root:1", "root.1:1", "root.1:2", "root:2"]);
    }

    #[tokio::test]
    async fn verifier_failure_retries_step() {
        let spec = make_spec("root", vec![make_step("1", None)]);
        let tasks = VecDeque::from(vec![
            ("root:1".to_string(), make_task("task-1", "1", None)),
            ("root:1".to_string(), make_task("task-1b", "1", None)),
        ]);
        let main_agent = Arc::new(MockMainAgent::default());
        let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
        let sub_agent = Arc::new(MockSubAgent::new(tasks));
        let sub_agent_factory = {
            let sub_agent = sub_agent.clone();
            Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            })
        };

        let mut orchestrator = Orchestrator::new(
            main_agent_trait,
            sub_agent_factory,
            VerifierChain::new(vec![Box::new(FailOnceVerifier::new())]),
            spec,
        );

        orchestrator.run().await.unwrap();

        let executions = sub_agent.executions();
        assert_eq!(executions, vec!["root:1", "root:1"]);
        let messages = main_agent.messages();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].1.contains("Verification failed"));
        let closed = main_agent.closed_channels();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0], "task-1b");
    }

    #[tokio::test]
    async fn closes_channel_after_success() {
        let spec = make_spec("root", vec![make_step("1", None)]);
        let tasks = VecDeque::from(vec![(
            "root:1".to_string(),
            make_task("task-success", "1", None),
        )]);
        let main_agent = Arc::new(MockMainAgent::default());
        let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
        let sub_agent = Arc::new(MockSubAgent::new(tasks));
        let sub_agent_factory = {
            let sub_agent = sub_agent.clone();
            Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            })
        };

        let mut orchestrator = Orchestrator::new(
            main_agent_trait,
            sub_agent_factory,
            VerifierChain::new(vec![Box::new(AlwaysPassVerifier)]),
            spec,
        );

        orchestrator.run().await.unwrap();

        let closed = main_agent.closed_channels();
        assert_eq!(closed, vec!["task-success".to_string()]);
        assert_eq!(main_agent.successes().len(), 1);
    }

    #[tokio::test]
    async fn split_child_spec_runs_before_parent_resumes() {
        let child_spec = make_spec(
            "root.2",
            vec![make_step("1", None), make_step("2", None)],
        );
        let root_spec = make_spec(
            "root",
            vec![
                make_step("1", None),
                make_step("2", None),
                make_step("3", None),
            ],
        );

        let main_agent = Arc::new(MockMainAgent::default());
        let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
        let sub_agent = Arc::new(SplitPlanSubAgent::new(child_spec.clone()));
        let sub_agent_factory = {
            let sub_agent = sub_agent.clone();
            Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            })
        };

        let mut orchestrator = Orchestrator::new(
            main_agent_trait,
            sub_agent_factory,
            VerifierChain::new(vec![Box::new(AlwaysPassVerifier)]),
            root_spec,
        );

        orchestrator.run().await.unwrap();
        assert_eq!(
            sub_agent.executions(),
            vec![
                "root:1".to_string(),
                "root:2".to_string(),
                "root.2:1".to_string(),
                "root.2:2".to_string(),
                "root:2".to_string(),
                "root:3".to_string()
            ]
        );
    }
}
