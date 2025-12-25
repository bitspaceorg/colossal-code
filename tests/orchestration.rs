//! Integration tests for orchestrator wiring and retries.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use agent_core::orchestrator::{
    Orchestrator, OrchestratorAgent, OrchestratorEvent, Verifier, VerifierChain,
};
use agent_core::{
    SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus,
};
use agent_protocol::types::spec::TestResult;
use agent_protocol::types::{
    spec::FeedbackEntry,
    task::{Task, TaskMetadata, TaskState, TaskStatus},
};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

fn build_spec(id: &str, titles: &[&str]) -> SpecSheet {
    let steps: Vec<SpecStep> = titles
        .iter()
        .enumerate()
        .map(|(idx, title)| SpecStep {
            index: (idx + 1).to_string(),
            title: title.to_string(),
            instructions: format!("Work on {}", title),
            acceptance_criteria: vec![format!("Validate {}", title)],
            required_tools: vec![],
            constraints: vec![],
            status: StepStatus::Pending,
            dependencies: if idx == 0 {
                vec![]
            } else {
                vec![(idx).to_string()]
            },
            sub_spec: None,
            completed_at: None,
        })
        .collect();

    SpecSheet {
        id: id.to_string(),
        title: format!("{} spec", id),
        description: "integration".to_string(),
        steps,
        created_by: "tests".to_string(),
        created_at: Utc::now(),
        metadata: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn task_summary(step_index: &str, text: &str) -> TaskSummary {
    TaskSummary {
        task_id: format!("task-{}", step_index),
        step_index: step_index.to_string(),
        summary_text: text.to_string(),
        artifacts_touched: vec![],
        tests_run: vec![TestRun {
            name: format!("test-{}", step_index),
            result: TestResult::Pass,
            logs_path: None,
            duration_ms: Some(10),
        }],
        verification: TaskVerification {
            status: VerificationStatus::Passed,
            feedback: vec![],
        },
    }
}

fn scripted_task(
    spec: &SpecSheet,
    step: &SpecStep,
    summary_text: &str,
    submitted: bool,
    child_spec: Option<SpecSheet>,
) -> Task {
    let summary = task_summary(&step.index, summary_text);
    let metadata = TaskMetadata {
        spec_sheet: child_spec.map(|s| serde_json::to_value(s).unwrap()),
        summary: Some(serde_json::to_value(summary.clone()).unwrap()),
        extra: BTreeMap::new(),
    };

    Task {
        id: format!(
            "{}-{}-{}",
            spec.id,
            step.index,
            if submitted { "split" } else { "done" }
        ),
        context_id: None,
        status: TaskStatus {
            state: if submitted {
                TaskState::Submitted
            } else {
                TaskState::Completed
            },
            timestamp: Some(Utc::now()),
            message: Some(summary_text.to_string()),
            error: None,
        },
        messages: vec![],
        artifacts: vec![],
        history: None,
        metadata: Some(metadata),
    }
}

#[derive(Default)]
struct RecordingMainAgent {
    statuses: Mutex<Vec<(String, StepStatus)>>,
    summaries: Mutex<Vec<TaskSummary>>,
    messages: Mutex<Vec<(String, String)>>,
    closed: Mutex<Vec<String>>,
}

impl RecordingMainAgent {
    fn summaries(&self) -> Vec<TaskSummary> {
        self.summaries.lock().unwrap().clone()
    }

    fn messages(&self) -> Vec<(String, String)> {
        self.messages.lock().unwrap().clone()
    }
}

#[async_trait]
impl OrchestratorAgent for RecordingMainAgent {
    async fn update_spec_status(
        &self,
        _spec: &SpecSheet,
        step: &SpecStep,
        _prefix: &str,
    ) -> anyhow::Result<()> {
        self.statuses
            .lock()
            .unwrap()
            .push((step.index.clone(), step.status));
        Ok(())
    }

    async fn execute_step(&self, _step: SpecStep, _spec: &SpecSheet) -> anyhow::Result<Task> {
        Err(anyhow::anyhow!("main agent does not execute steps"))
    }

    async fn update_task_summary(&self, summary: &TaskSummary) -> anyhow::Result<()> {
        self.summaries.lock().unwrap().push(summary.clone());
        Ok(())
    }

    async fn send_task_message(&self, task_id: &str, message: &str) -> anyhow::Result<()> {
        self.messages
            .lock()
            .unwrap()
            .push((task_id.to_string(), message.to_string()));
        Ok(())
    }

    async fn notify_step_success(&self, _summary: &TaskSummary) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close_task_channel(&self, task_id: &str) -> anyhow::Result<()> {
        self.closed.lock().unwrap().push(task_id.to_string());
        Ok(())
    }
}

struct ScriptedSubAgent {
    tasks: Mutex<VecDeque<(String, Task)>>,
    executions: Mutex<Vec<String>>,
}

impl ScriptedSubAgent {
    fn new(tasks: VecDeque<(String, Task)>) -> Self {
        Self {
            tasks: Mutex::new(tasks),
            executions: Mutex::new(Vec::new()),
        }
    }

    fn executions(&self) -> Vec<String> {
        self.executions.lock().unwrap().clone()
    }
}

#[async_trait]
impl OrchestratorAgent for ScriptedSubAgent {
    async fn update_spec_status(
        &self,
        _spec: &SpecSheet,
        _step: &SpecStep,
        _prefix: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> anyhow::Result<Task> {
        let label = format!("{}:{}", spec.id, step.index);
        self.executions.lock().unwrap().push(label.clone());
        let mut queue = self.tasks.lock().unwrap();
        let (expected, task) = queue.pop_front().expect("no scripted task available");
        assert_eq!(expected, label, "unexpected execution order");
        Ok(task)
    }

    async fn update_task_summary(&self, _summary: &TaskSummary) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send_task_message(&self, _task_id: &str, _message: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn notify_step_success(&self, _summary: &TaskSummary) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close_task_channel(&self, _task_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

struct PassingVerifier;

#[async_trait]
impl Verifier for PassingVerifier {
    async fn verify(&self, _summary: &TaskSummary) -> Result<(), FeedbackEntry> {
        Ok(())
    }
}

struct FailOnceVerifier {
    tripped: Mutex<HashSet<String>>,
}

impl FailOnceVerifier {
    fn new() -> Self {
        Self {
            tripped: Mutex::new(HashSet::new()),
        }
    }
}

#[async_trait]
impl Verifier for FailOnceVerifier {
    async fn verify(&self, summary: &TaskSummary) -> Result<(), FeedbackEntry> {
        let mut guard = self.tripped.lock().unwrap();
        if guard.insert(summary.step_index.clone()) {
            Err(FeedbackEntry {
                author: "fail-once".to_string(),
                message: format!("retry step {}", summary.step_index),
                timestamp: Utc::now(),
            })
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn sequential_spec_records_history() {
    let spec = build_spec("seq", &["plan", "build", "ship"]);
    let step_indices: Vec<String> = spec.steps.iter().map(|s| s.index.clone()).collect();
    let mut queue = VecDeque::new();
    for step in &spec.steps {
        queue.push_back((
            format!("{}:{}", spec.id, step.index),
            scripted_task(&spec, step, "happy path", false, None),
        ));
    }
    let sub_agent = Arc::new(ScriptedSubAgent::new(queue));
    let sub_factory = {
        let agent = sub_agent.clone();
        Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> { agent.clone() })
    };
    let main_agent = Arc::new(RecordingMainAgent::default());

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, _control) = Orchestrator::new_with_control(
        main_agent.clone(),
        sub_factory,
        VerifierChain::new(vec![Box::new(PassingVerifier)]),
        spec,
        event_tx,
    );

    orchestrator.run().await.unwrap();

    assert_eq!(
        sub_agent.executions(),
        vec![
            "seq:1".to_string(),
            "seq:2".to_string(),
            "seq:3".to_string()
        ]
    );
    assert_eq!(main_agent.summaries().len(), step_indices.len() * 2);

    let mut history_updates = Vec::new();
    let mut saw_completed = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            OrchestratorEvent::SummaryUpdated { summary } => {
                history_updates.push(summary.step_index);
            }
            OrchestratorEvent::Completed => {
                saw_completed = true;
            }
            _ => {}
        }
    }
    let expected_updates: Vec<String> = step_indices
        .iter()
        .flat_map(|idx| [idx.clone(), idx.clone()])
        .collect();
    assert_eq!(history_updates, expected_updates);
    assert!(saw_completed);
}

#[tokio::test]
async fn split_spec_runs_depth_first() {
    let parent = build_spec("parent", &["prep", "split", "finish"]);
    let child = build_spec("child", &["sub-one", "sub-two"]);

    let mut queue = VecDeque::new();
    let step_one = parent.steps[0].clone();
    queue.push_back((
        format!("{}:{}", parent.id, step_one.index),
        scripted_task(&parent, &step_one, "done", false, None),
    ));

    let step_two = parent.steps[1].clone();
    queue.push_back((
        format!("{}:{}", parent.id, step_two.index),
        scripted_task(&parent, &step_two, "spawn child", true, Some(child.clone())),
    ));

    for child_step in &child.steps {
        queue.push_back((
            format!("{}:{}", child.id, child_step.index),
            scripted_task(&child, child_step, "child step", false, None),
        ));
    }

    queue.push_back((
        format!("{}:{}", parent.id, step_two.index),
        scripted_task(&parent, &step_two, "resume", false, None),
    ));

    let step_three = parent.steps[2].clone();
    queue.push_back((
        format!("{}:{}", parent.id, step_three.index),
        scripted_task(&parent, &step_three, "wrap", false, None),
    ));

    let sub_agent = Arc::new(ScriptedSubAgent::new(queue));
    let sub_factory = {
        let agent = sub_agent.clone();
        Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> { agent.clone() })
    };
    let main_agent = Arc::new(RecordingMainAgent::default());
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, _control) = Orchestrator::new_with_control(
        main_agent,
        sub_factory,
        VerifierChain::new(vec![Box::new(PassingVerifier)]),
        parent,
        event_tx,
    );

    orchestrator.run().await.unwrap();

    assert_eq!(
        sub_agent.executions(),
        vec![
            "parent:1".to_string(),
            "parent:2".to_string(),
            "child:1".to_string(),
            "child:2".to_string(),
            "parent:2".to_string(),
            "parent:3".to_string()
        ]
    );
    let mut saw_child_event = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, OrchestratorEvent::ChildSpecPushed { .. }) {
            saw_child_event = true;
        }
    }
    assert!(saw_child_event);
}

#[tokio::test]
async fn verifier_failure_emits_event_and_retry() {
    let spec = build_spec("retry", &["verify"]);
    let mut queue = VecDeque::new();
    let retry_step = spec.steps[0].clone();
    queue.push_back((
        format!("{}:{}", spec.id, retry_step.index),
        scripted_task(&spec, &retry_step, "first", false, None),
    ));
    queue.push_back((
        format!("{}:{}", spec.id, retry_step.index),
        scripted_task(&spec, &retry_step, "second", false, None),
    ));

    let sub_agent = Arc::new(ScriptedSubAgent::new(queue));
    let sub_factory = {
        let agent = sub_agent.clone();
        Arc::new(move |_step: &SpecStep| -> Arc<dyn OrchestratorAgent> { agent.clone() })
    };
    let main_agent = Arc::new(RecordingMainAgent::default());
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, _control) = Orchestrator::new_with_control(
        main_agent.clone(),
        sub_factory,
        VerifierChain::new(vec![Box::new(FailOnceVerifier::new())]),
        spec,
        event_tx,
    );

    orchestrator.run().await.unwrap();

    assert_eq!(sub_agent.executions(), vec!["retry:1", "retry:1"]);

    let mut saw_failure_event = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, OrchestratorEvent::VerifierFailed { .. }) {
            saw_failure_event = true;
        }
    }
    assert!(saw_failure_event);

    let messages = main_agent.messages();
    assert!(
        messages
            .iter()
            .any(|(_, msg)| msg.contains("Verification failed"))
    );
}
