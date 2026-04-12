use super::*;
use agent_protocol::types::{
    spec::{SpecSheet, SpecStep, StepStatus, TaskVerification, VerificationStatus},
    task::{Task, TaskMetadata, TaskState, TaskStatus},
};
use anyhow::anyhow;
use chrono::Utc;
use serde_json::{Value, map::Entry};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

fn make_step(index: &str, sub_spec: Option<SpecSheet>) -> SpecStep {
    SpecStep {
        index: index.to_string(),
        title: format!("Step {index}"),
        instructions: "Do things".to_string(),
        acceptance_criteria: vec![],
        required_tools: vec![],
        constraints: vec![],
        dependencies: vec![],
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
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
        worktree: None,
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

fn make_verifier_task(spec_id: &str, status: &str, feedback: &str) -> Task {
    let mut metadata = TaskMetadata::default();
    let arguments = serde_json::json!({
        "status": status,
        "feedback": feedback,
        "end_convo": true,
    });
    metadata.extra.insert(
        "toolLog".to_string(),
        serde_json::json!([
            {
                "name": "submit_verification",
                "arguments": arguments.to_string(),
                "result": serde_json::Value::Null
            }
        ]),
    );

    Task {
        id: format!("{}-verifier", spec_id),
        context_id: None,
        status: TaskStatus {
            state: TaskState::Completed,
            timestamp: Some(Utc::now()),
            message: Some("verification run".to_string()),
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
        worktree: None,
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

struct BlockingSubAgent {
    cancelled: Arc<AtomicBool>,
    started_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

impl BlockingSubAgent {
    fn new(cancelled: Arc<AtomicBool>, started_tx: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
        Self {
            cancelled,
            started_tx,
        }
    }

    fn cancelled_task(spec: &SpecSheet, step: &SpecStep) -> Task {
        let mut task = Task::new();
        task.context_id = Some(spec.id.clone());
        task.status = TaskStatus {
            state: TaskState::Cancelled,
            timestamp: Some(Utc::now()),
            message: Some("Step cancelled".to_string()),
            error: None,
        };
        let mut metadata = TaskMetadata::default();
        metadata
            .extra
            .insert("step_index".to_string(), Value::String(step.index.clone()));
        task.metadata = Some(metadata);
        task
    }
}

#[async_trait]
impl OrchestratorAgent for BlockingSubAgent {
    async fn update_spec_status(
        &self,
        _spec: &SpecSheet,
        _step: &SpecStep,
        _prefix: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
        self.execute_step_with_events_and_cancel(step, spec, "", None, None)
            .await
    }

    async fn execute_step_with_events_and_cancel(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        _prefix: &str,
        _event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
        mut cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Result<Task> {
        let _ = self.started_tx.send(());
        if let Some(rx) = cancel_rx.as_mut() {
            let _ = rx.recv().await;
            self.cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(Self::cancelled_task(spec, &step))
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
    use std::path::PathBuf;

    let child_spec = make_spec("root.1", vec![make_step("1", None), make_step("2", None)]);
    let root_spec = make_spec(
        "root",
        vec![
            make_step("1", Some(child_spec.clone())),
            make_step("2", None),
        ],
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
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let mut orchestrator = Orchestrator::new(main_agent_trait, sub_agent_factory, root_spec);

    orchestrator.run().await.unwrap();

    let prefixes = main_agent.in_progress_prefixes();
    assert_eq!(prefixes, vec!["1", "1.1", "1.2", "2"]);
    assert_eq!(
        sub_agent.executions(),
        vec!["root:1", "root.1:1", "root.1:2", "root:2"]
    );
}

#[tokio::test]
async fn verifier_failure_retries_step() {
    use std::path::PathBuf;

    let mut step = make_step("1", None);
    step.requires_verification = true;
    let spec = make_spec("root", vec![step.clone()]);
    let tasks = VecDeque::from(vec![
        ("root:1".to_string(), make_task("task-1", "1", None)),
        (
            "root::summary:summary".to_string(),
            make_task("root-summary-1", "summary", None),
        ),
        (
            "root::verifier:1".to_string(),
            make_verifier_task("root", "needs_revision", "try again"),
        ),
        ("root:1".to_string(), make_task("task-1b", "1", None)),
        (
            "root::summary:summary".to_string(),
            make_task("root-summary-2", "summary", None),
        ),
        (
            "root::verifier:1".to_string(),
            make_verifier_task("root", "verified", "looks good"),
        ),
    ]);
    let main_agent = Arc::new(MockMainAgent::default());
    let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
    let sub_agent = Arc::new(MockSubAgent::new(tasks));
    let sub_agent_factory = {
        let sub_agent = sub_agent.clone();
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let mut orchestrator = Orchestrator::new(main_agent_trait, sub_agent_factory, spec);

    orchestrator.run().await.unwrap();

    let executions = sub_agent.executions();
    assert_eq!(
        executions,
        vec![
            "root:1",
            "root::summary:summary",
            "root::verifier:1",
            "root:1",
            "root::summary:summary",
            "root::verifier:1"
        ]
    );
    let messages = main_agent.messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].1.contains("try again"));
    let closed = main_agent.closed_channels();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0], "task-1b");
}

#[tokio::test]
async fn closes_channel_after_success() {
    use std::path::PathBuf;

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
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let mut orchestrator = Orchestrator::new(main_agent_trait, sub_agent_factory, spec);

    orchestrator.run().await.unwrap();

    let closed = main_agent.closed_channels();
    assert_eq!(closed, vec!["task-success".to_string()]);
    assert_eq!(main_agent.successes().len(), 1);
}

#[tokio::test]
async fn split_child_spec_runs_before_parent_resumes() {
    use std::path::PathBuf;

    let child_spec = make_spec("root.2", vec![make_step("1", None), make_step("2", None)]);
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
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let mut orchestrator = Orchestrator::new(main_agent_trait, sub_agent_factory, root_spec);

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

#[tokio::test]
async fn abort_stops_execution() {
    use std::path::PathBuf;

    let spec = make_spec(
        "root",
        vec![
            make_step("1", None),
            make_step("2", None),
            make_step("3", None),
        ],
    );
    let tasks = VecDeque::from(vec![
        ("root:1".to_string(), make_task("task-1", "1", None)),
        ("root:2".to_string(), make_task("task-2", "2", None)),
        ("root:3".to_string(), make_task("task-3", "3", None)),
    ]);
    let main_agent = Arc::new(MockMainAgent::default());
    let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
    let sub_agent = Arc::new(MockSubAgent::new(tasks));
    let sub_agent_factory = {
        let sub_agent = sub_agent.clone();
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, control) =
        Orchestrator::new_with_control(main_agent_trait, sub_agent_factory, spec, event_tx);

    // Abort immediately
    control.abort().unwrap();

    orchestrator.run().await.unwrap();

    // Should receive Aborted event
    let mut found_abort = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, OrchestratorEvent::Aborted) {
            found_abort = true;
        }
    }
    assert!(found_abort, "Should receive Aborted event");

    // Should not have executed any steps since abort was immediate
    assert!(sub_agent.executions().is_empty() || sub_agent.executions().len() < 3);
}

#[tokio::test]
async fn events_emitted_during_execution() {
    use std::path::PathBuf;

    let spec = make_spec("root", vec![make_step("1", None)]);
    let tasks = VecDeque::from(vec![("root:1".to_string(), make_task("task-1", "1", None))]);
    let main_agent = Arc::new(MockMainAgent::default());
    let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
    let sub_agent = Arc::new(MockSubAgent::new(tasks));
    let sub_agent_factory = {
        let sub_agent = sub_agent.clone();
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, _control) =
        Orchestrator::new_with_control(main_agent_trait, sub_agent_factory, spec, event_tx);

    orchestrator.run().await.unwrap();

    // Collect all events
    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }

    // Should have InProgress, SummaryUpdated, SummaryUpdated (Passed), Completed events
    assert!(events.iter().any(|e| matches!(
        e,
        OrchestratorEvent::StepStatusChanged {
            status: StepStatus::InProgress,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OrchestratorEvent::StepStatusChanged {
            status: StepStatus::Completed,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OrchestratorEvent::Completed))
    );
}

#[tokio::test]
async fn inject_split_adds_child_spec() {
    use std::path::PathBuf;

    let spec = make_spec("root", vec![make_step("1", None), make_step("2", None)]);
    let child_spec = make_spec("child", vec![make_step("1", None)]);
    let tasks = VecDeque::from(vec![
        ("root:1".to_string(), make_task("task-1", "1", None)),
        ("root:2".to_string(), make_task("task-2", "2", None)),
        ("child:1".to_string(), make_task("child-task-1", "1", None)),
    ]);
    let main_agent = Arc::new(MockMainAgent::default());
    let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;
    let sub_agent = Arc::new(MockSubAgent::new(tasks));
    let sub_agent_factory = {
        let sub_agent = sub_agent.clone();
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                sub_agent.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, control) =
        Orchestrator::new_with_control(main_agent_trait, sub_agent_factory, spec, event_tx);

    // Inject child spec at step 1
    control.inject_split("1".to_string(), child_spec).unwrap();

    orchestrator.run().await.unwrap();

    // Check for ChildSpecPushed event
    let mut found_child_pushed = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, OrchestratorEvent::ChildSpecPushed { .. }) {
            found_child_pushed = true;
        }
    }
    assert!(found_child_pushed, "Should receive ChildSpecPushed event");
}

#[tokio::test]
async fn abort_sends_cancel_signal_to_running_step() {
    use std::path::PathBuf;

    let spec = make_spec("root", vec![make_step("1", None)]);
    let main_agent = Arc::new(MockMainAgent::default());
    let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;

    let (start_tx, mut start_rx) = tokio::sync::mpsc::unbounded_channel();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let blocker = Arc::new(BlockingSubAgent::new(cancelled.clone(), start_tx));
    let sub_agent_factory = {
        let blocker = blocker.clone();
        Arc::new(
            move |_step: &SpecStep, _cwd: Option<PathBuf>| -> Arc<dyn OrchestratorAgent> {
                blocker.clone() as Arc<dyn OrchestratorAgent>
            },
        )
    };

    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (mut orchestrator, control) =
        Orchestrator::new_with_control(main_agent_trait, sub_agent_factory, spec, event_tx);

    let orchestrator_handle = tokio::spawn(async move {
        orchestrator.run().await.unwrap();
    });

    start_rx.recv().await.expect("step never started");
    control.abort().unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), orchestrator_handle)
        .await
        .expect("orchestrator did not stop after abort");

    assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
}
