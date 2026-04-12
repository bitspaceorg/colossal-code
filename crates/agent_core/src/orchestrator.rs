use std::any::Any;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_protocol::types::{
    spec::{FeedbackEntry, SpecSheet, SpecStep, StepStatus, TaskSummary, VerificationStatus},
    task::{Task, TaskState},
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, map::Entry};
use tokio::sync::mpsc;

use crate::{Agent, resolve_workspace_root};

pub use self::orchestrator_support::{
    StepDisposition, SummarizerContext, VerificationContext, VerificationOutcome,
    VerificationToolPayload, Verifier, VerifierChain, build_summarizer_spec, build_verifier_spec,
    format_tool_log,
};

#[path = "orchestrator_support.rs"]
mod orchestrator_support;
#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod orchestrator_tests;

/// Role of a step within orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepRole {
    Implementor,
    Summarizer,
    Verifier,
    Merge,
}

/// Events emitted by the orchestrator for TUI updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrchestratorEvent {
    /// Step status changed (in-progress, completed, failed, retry)
    StepStatusChanged {
        spec_id: String,
        spec_title: String,
        step_index: String,
        step_title: String,
        prefix: String,
        role: StepRole,
        status: StepStatus,
    },
    /// Task summary updated
    SummaryUpdated { summary: TaskSummary },
    /// Verifier failed, step will be retried
    VerifierFailed {
        summary: TaskSummary,
        feedback: String,
    },
    /// Child spec pushed onto stack (split occurred)
    ChildSpecPushed {
        parent_step_index: String,
        child_spec_id: String,
        child_step_count: usize,
    },
    /// Task channel closed (used by SSE subscribers)
    ChannelClosed {
        task_id: String,
        closed_at: DateTime<Utc>,
    },
    /// Orchestrator paused
    Paused,
    /// Orchestrator resumed
    Resumed,
    /// Orchestrator aborted
    Aborted,
    /// Step was cancelled and awaiting resume
    StepCancelled { prefix: String },
    /// Orchestrator completed all steps
    Completed,
    /// Error occurred
    Error(String),
    /// Tool call started during step execution
    ToolCallStarted {
        prefix: String,
        tool_name: String,
        arguments: String,
    },
    /// Tool call completed during step execution
    ToolCallCompleted {
        prefix: String,
        tool_name: String,
        result: String,
        is_error: bool,
    },
    /// Sub-agent message (text, thinking, or tool call detail)
    AgentMessage {
        prefix: String,
        message: SubAgentMessage,
    },
}

/// A message from a sub-agent during step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubAgentMessage {
    /// User prompt sent to the sub-agent
    UserPrompt { content: String },
    /// Text response from the sub-agent
    Text { content: String },
    /// Thinking block from the sub-agent
    Thinking { content: String, duration_secs: u64 },
    /// Tool call with full details
    ToolCall {
        tool_name: String,
        arguments: String,
        result: Option<String>,
        is_error: bool,
    },
    /// Generation statistics
    GenerationStats {
        tokens_per_sec: f32,
        input_tokens: usize,
        output_tokens: usize,
    },
    /// Sub-agent finished its turn
    Done,
    /// Sub-agent errored out while running a step
    Error { message: String },
}

/// Control signals for the orchestrator.
#[derive(Debug, Clone)]
pub enum OrchestratorCommand {
    /// Pause execution after current step
    Pause,
    /// Resume execution
    Resume,
    /// Abort execution
    Abort,
    /// Rerun verifiers on the last task summary
    RerunVerifiers,
    /// Inject a split spec at a given index
    InjectSplit {
        step_index: String,
        child_spec: SpecSheet,
    },
    /// Cancel a running step by prefix
    CancelStep { prefix: String },
    /// Resume a cancelled step with an optional message
    ResumeStep { prefix: String, message: String },
}

/// Control handle for the orchestrator, allowing TUI to send commands.
#[derive(Clone)]
pub struct OrchestratorControl {
    command_tx: mpsc::UnboundedSender<OrchestratorCommand>,
    paused: Arc<AtomicBool>,
    aborted: Arc<AtomicBool>,
    current_prefix: Arc<Mutex<Option<String>>>,
    current_cancel: Arc<Mutex<Option<mpsc::UnboundedSender<()>>>>,
}

impl OrchestratorControl {
    fn signal_current_cancel(&self) {
        if let Some(sender) = self.current_cancel.lock().unwrap().clone() {
            let _ = sender.send(());
        }
    }

    fn signal_cancel_if_matches(&self, prefix: &str) {
        let matches_current = self
            .current_prefix
            .lock()
            .unwrap()
            .as_deref()
            .map(|p| p == prefix)
            .unwrap_or(false);
        if matches_current {
            self.signal_current_cancel();
        }
    }

    /// Create a new control handle with the given command sender.
    pub fn new(command_tx: mpsc::UnboundedSender<OrchestratorCommand>) -> Self {
        Self {
            command_tx,
            paused: Arc::new(AtomicBool::new(false)),
            aborted: Arc::new(AtomicBool::new(false)),
            current_prefix: Arc::new(Mutex::new(None)),
            current_cancel: Arc::new(Mutex::new(None)),
        }
    }

    /// Pause the orchestrator after the current step completes.
    pub fn pause(&self) -> Result<()> {
        self.paused.store(true, Ordering::SeqCst);
        self.command_tx
            .send(OrchestratorCommand::Pause)
            .map_err(|e| anyhow!("Failed to send pause command: {}", e))
    }

    /// Resume the orchestrator.
    pub fn resume(&self) -> Result<()> {
        self.paused.store(false, Ordering::SeqCst);
        self.command_tx
            .send(OrchestratorCommand::Resume)
            .map_err(|e| anyhow!("Failed to send resume command: {}", e))
    }

    /// Abort the orchestrator run.
    pub fn abort(&self) -> Result<()> {
        self.aborted.store(true, Ordering::SeqCst);
        self.signal_current_cancel();
        self.command_tx
            .send(OrchestratorCommand::Abort)
            .map_err(|e| anyhow!("Failed to send abort command: {}", e))
    }

    /// Rerun verifiers on the last completed step.
    pub fn rerun_verifiers(&self) -> Result<()> {
        self.command_tx
            .send(OrchestratorCommand::RerunVerifiers)
            .map_err(|e| anyhow!("Failed to send rerun verifiers command: {}", e))
    }

    /// Cancel a running step by prefix.
    pub fn cancel_step(&self, prefix: String) -> Result<()> {
        self.signal_cancel_if_matches(&prefix);
        self.command_tx
            .send(OrchestratorCommand::CancelStep { prefix })
            .map_err(|e| anyhow!("Failed to send cancel step command: {}", e))
    }

    /// Resume a cancelled step with a user message.
    pub fn resume_step(&self, prefix: String, message: String) -> Result<()> {
        self.command_tx
            .send(OrchestratorCommand::ResumeStep { prefix, message })
            .map_err(|e| anyhow!("Failed to send resume step command: {}", e))
    }

    /// Inject a split spec at a given step index.
    pub fn inject_split(&self, step_index: String, child_spec: SpecSheet) -> Result<()> {
        self.command_tx
            .send(OrchestratorCommand::InjectSplit {
                step_index,
                child_spec,
            })
            .map_err(|e| anyhow!("Failed to send inject split command: {}", e))
    }

    /// Check if the orchestrator is paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Check if the orchestrator has been aborted.
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }
}

#[async_trait]
pub trait OrchestratorAgent: Send + Sync {
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }

    async fn update_spec_status(
        &self,
        spec: &SpecSheet,
        step: &SpecStep,
        prefix: &str,
    ) -> Result<()>;

    async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task>;

    /// Execute a step with optional event sink for tool call notifications.
    /// Default implementation just calls execute_step, ignoring events.
    async fn execute_step_with_events(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    ) -> Result<Task> {
        let _ = prefix;
        let _ = event_tx;
        self.execute_step(step, spec).await
    }

    async fn execute_step_with_events_and_cancel(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
        _cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Result<Task> {
        self.execute_step_with_events(step, spec, prefix, event_tx)
            .await
    }

    async fn update_task_summary(&self, summary: &TaskSummary) -> Result<()>;

    async fn send_task_message(&self, task_id: &str, message: &str) -> Result<()>;

    async fn notify_step_success(&self, summary: &TaskSummary) -> Result<()>;

    async fn close_task_channel(&self, task_id: &str) -> Result<()>;
}

pub struct Orchestrator {
    main_agent: Arc<dyn OrchestratorAgent>,
    sub_agent_factory:
        Arc<dyn Fn(&SpecStep, Option<PathBuf>) -> Arc<dyn OrchestratorAgent> + Send + Sync>,
    _verifier_chain: VerifierChain,
    stack: Vec<(SpecSheet, usize, String)>,
    /// Event sender for TUI updates
    event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    /// Command receiver for control signals
    command_rx: Option<mpsc::UnboundedReceiver<OrchestratorCommand>>,
    /// Pause flag
    paused: Arc<AtomicBool>,
    /// Abort flag
    aborted: Arc<AtomicBool>,
    /// Last task summary for rerun verifiers
    last_summary: Option<TaskSummary>,
    /// Cached context for rerunning the verifier agent
    last_verification: Option<VerificationContext>,
    /// Prefix of the step currently executing
    current_prefix: Option<String>,
    /// Cancel channel for the current step
    current_cancel: Option<mpsc::UnboundedSender<()>>,
    /// Shared view of the current prefix for external control
    shared_current_prefix: Arc<Mutex<Option<String>>>,
    /// Shared view of the cancel sender for external control
    shared_current_cancel: Arc<Mutex<Option<mpsc::UnboundedSender<()>>>>,
    /// Pending resume payload for a cancelled step
    pending_resume: Option<(SpecSheet, usize, String)>,
    /// Pending cancellation prefix
    cancel_prefix: Option<String>,
}

impl Orchestrator {
    fn set_current_prefix_state(&mut self, prefix: Option<String>) {
        self.current_prefix = prefix.clone();
        let mut shared = self.shared_current_prefix.lock().unwrap();
        *shared = prefix;
    }

    fn set_current_cancel_state(&mut self, sender: Option<mpsc::UnboundedSender<()>>) {
        self.current_cancel = sender.clone();
        let mut shared = self.shared_current_cancel.lock().unwrap();
        *shared = sender;
    }

    fn take_current_cancel_sender(&mut self) -> Option<mpsc::UnboundedSender<()>> {
        let sender = self.current_cancel.take();
        if sender.is_some() {
            let mut shared = self.shared_current_cancel.lock().unwrap();
            *shared = None;
        }
        sender
    }

    /// Create a new orchestrator without event/control channels (legacy interface).
    pub fn new(
        main_agent: Arc<dyn OrchestratorAgent>,
        sub_agent_factory: Arc<
            dyn Fn(&SpecStep, Option<PathBuf>) -> Arc<dyn OrchestratorAgent> + Send + Sync,
        >,
        spec: SpecSheet,
    ) -> Self {
        let shared_current_prefix = Arc::new(Mutex::new(None));
        let shared_current_cancel = Arc::new(Mutex::new(None));
        let mut orchestrator = Self {
            main_agent,
            sub_agent_factory,
            _verifier_chain: VerifierChain::default(),
            stack: vec![(spec, 0, String::new())],
            event_tx: None,
            command_rx: None,
            paused: Arc::new(AtomicBool::new(false)),
            aborted: Arc::new(AtomicBool::new(false)),
            last_summary: None,
            last_verification: None,
            current_prefix: None,
            current_cancel: None,
            shared_current_prefix,
            shared_current_cancel,
            pending_resume: None,
            cancel_prefix: None,
        };
        orchestrator.set_current_prefix_state(None);
        orchestrator.set_current_cancel_state(None);
        orchestrator
    }

    /// Create a new orchestrator with event sender and command receiver.
    /// Returns the Orchestrator and an OrchestratorControl handle for the TUI.
    pub fn new_with_control(
        main_agent: Arc<dyn OrchestratorAgent>,
        sub_agent_factory: Arc<
            dyn Fn(&SpecStep, Option<PathBuf>) -> Arc<dyn OrchestratorAgent> + Send + Sync,
        >,
        spec: SpecSheet,
        event_tx: mpsc::UnboundedSender<OrchestratorEvent>,
    ) -> (Self, OrchestratorControl) {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let paused = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        let shared_current_prefix = Arc::new(Mutex::new(None));
        let shared_current_cancel = Arc::new(Mutex::new(None));

        let control = OrchestratorControl {
            command_tx,
            paused: paused.clone(),
            aborted: aborted.clone(),
            current_prefix: shared_current_prefix.clone(),
            current_cancel: shared_current_cancel.clone(),
        };

        let mut orchestrator = Self {
            main_agent,
            sub_agent_factory,
            _verifier_chain: VerifierChain::default(),
            stack: vec![(spec, 0, String::new())],
            event_tx: Some(event_tx),
            command_rx: Some(command_rx),
            paused,
            aborted,
            last_summary: None,
            last_verification: None,
            current_prefix: None,
            current_cancel: None,
            shared_current_prefix,
            shared_current_cancel,
            pending_resume: None,
            cancel_prefix: None,
        };
        orchestrator.set_current_prefix_state(None);
        orchestrator.set_current_cancel_state(None);

        (orchestrator, control)
    }

    /// Emit an event if the event channel is configured.
    fn emit_event(&self, event: OrchestratorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Check for pending commands and process them.
    /// Returns true if abort was requested.
    async fn process_commands(&mut self) -> bool {
        let commands: Vec<OrchestratorCommand> = if let Some(ref mut rx) = self.command_rx {
            let mut cmds = Vec::new();
            while let Ok(cmd) = rx.try_recv() {
                cmds.push(cmd);
            }
            cmds
        } else {
            Vec::new()
        };

        for cmd in commands {
            match cmd {
                OrchestratorCommand::Pause => {
                    self.paused.store(true, Ordering::SeqCst);
                    self.emit_event(OrchestratorEvent::Paused);
                }
                OrchestratorCommand::Resume => {
                    self.paused.store(false, Ordering::SeqCst);
                    self.emit_event(OrchestratorEvent::Resumed);
                }
                OrchestratorCommand::Abort => {
                    self.aborted.store(true, Ordering::SeqCst);
                    self.emit_event(OrchestratorEvent::Aborted);
                    return true;
                }
                OrchestratorCommand::RerunVerifiers => {
                    if let Err(err) = self.rerun_last_verifier().await {
                        self.emit_event(OrchestratorEvent::Error(format!(
                            "Failed to rerun verifier: {}",
                            err
                        )));
                    }
                }
                OrchestratorCommand::InjectSplit {
                    step_index,
                    child_spec,
                } => {
                    let prefix = self
                        .stack
                        .last()
                        .map(|(_, _, p)| Self::compose_prefix(p, &step_index))
                        .unwrap_or_else(|| step_index.clone());
                    self.stack.push((child_spec.clone(), 0, prefix));
                    self.emit_event(OrchestratorEvent::ChildSpecPushed {
                        parent_step_index: step_index,
                        child_spec_id: child_spec.id.clone(),
                        child_step_count: child_spec.steps.len(),
                    });
                }
                OrchestratorCommand::CancelStep { prefix } => {
                    self.cancel_prefix = Some(prefix.clone());
                    if let Some(current) = self.current_prefix.as_ref() {
                        if current == &prefix {
                            if let Some(tx) = self.take_current_cancel_sender() {
                                let _ = tx.send(());
                            }
                        }
                    }
                }
                OrchestratorCommand::ResumeStep { prefix, message } => {
                    if let Some((mut spec, step_idx, resume_prefix)) = self.pending_resume.take() {
                        if resume_prefix == prefix {
                            spec.steps[step_idx].status = StepStatus::Pending;
                            spec.steps[step_idx].completed_at = None;
                            if !message.trim().is_empty() {
                                let instructions = spec.steps[step_idx].instructions.clone();
                                spec.steps[step_idx].instructions = format!(
                                    "{}\n\nUser note: {}",
                                    instructions.trim_end(),
                                    message
                                );
                            }
                            self.stack.push((spec, step_idx, resume_prefix));
                            self.paused.store(false, Ordering::SeqCst);
                            self.emit_event(OrchestratorEvent::Resumed);
                        } else {
                            self.pending_resume = Some((spec, step_idx, resume_prefix));
                        }
                    } else {
                        self.paused.store(false, Ordering::SeqCst);
                        self.emit_event(OrchestratorEvent::Resumed);
                    }
                }
            }
        }
        false
    }

    /// Wait while paused, processing resume/abort commands.
    async fn wait_while_paused(&mut self) -> bool {
        while self.paused.load(Ordering::SeqCst) {
            if self.process_commands().await {
                return true;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        false
    }

    pub async fn run(&mut self) -> Result<()> {
        let workspace_root = resolve_workspace_root();
        while let Some((mut spec, step_idx, prefix)) = self.stack.pop() {
            if self.process_commands().await {
                return Ok(());
            }

            if self.wait_while_paused().await {
                return Ok(());
            }

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

            self.emit_event(OrchestratorEvent::StepStatusChanged {
                spec_id: spec.id.clone(),
                spec_title: spec.title.clone(),
                step_index: spec.steps[step_idx].index.clone(),
                step_title: spec.steps[step_idx].title.clone(),
                prefix: current_prefix.clone(),
                role: StepRole::Implementor,
                status: StepStatus::InProgress,
            });

            let in_progress_step = spec.steps[step_idx].clone();
            self.main_agent
                .update_spec_status(&spec, &in_progress_step, &current_prefix)
                .await?;

            let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
            self.set_current_prefix_state(Some(current_prefix.clone()));
            self.set_current_cancel_state(Some(cancel_tx));

            if self.cancel_prefix.as_deref() == Some(&current_prefix) {
                if let Some(entry) = spec.steps.get_mut(step_idx) {
                    entry.status = StepStatus::Pending;
                    entry.completed_at = None;
                }
                self.pending_resume = Some((spec, step_idx, prefix));
                self.cancel_prefix = None;
                self.set_current_cancel_state(None);
                self.set_current_prefix_state(None);
                self.emit_event(OrchestratorEvent::StepCancelled {
                    prefix: current_prefix.clone(),
                });
                self.paused.store(true, Ordering::SeqCst);
                continue;
            }

            let sub_agent =
                (self.sub_agent_factory)(&in_progress_step, Some(workspace_root.clone()));
            let task = sub_agent
                .execute_step_with_events_and_cancel(
                    in_progress_step.clone(),
                    &spec,
                    &current_prefix,
                    self.event_tx.clone(),
                    Some(cancel_rx),
                )
                .await?;

            self.set_current_cancel_state(None);
            self.set_current_prefix_state(None);

            if self.process_commands().await {
                return Ok(());
            }

            if self.cancel_prefix.as_deref() == Some(&current_prefix) {
                if let Some(entry) = spec.steps.get_mut(step_idx) {
                    entry.status = StepStatus::Pending;
                    entry.completed_at = None;
                }
                self.pending_resume = Some((spec, step_idx, prefix));
                self.cancel_prefix = None;
                self.emit_event(OrchestratorEvent::StepCancelled {
                    prefix: current_prefix.clone(),
                });
                self.paused.store(true, Ordering::SeqCst);
                continue;
            }
            if task.status.state == TaskState::Cancelled {
                if let Some(entry) = spec.steps.get_mut(step_idx) {
                    entry.status = StepStatus::Pending;
                    entry.completed_at = None;
                }
                self.pending_resume = Some((spec, step_idx, prefix));
                self.emit_event(OrchestratorEvent::StepCancelled {
                    prefix: current_prefix.clone(),
                });
                self.paused.store(true, Ordering::SeqCst);
                continue;
            }
            if task.status.state == TaskState::Submitted {
                if let Some(child_spec) = Self::extract_sub_spec(&task)? {
                    if let Some(entry) = spec.steps.get_mut(step_idx) {
                        entry.status = StepStatus::Pending;
                        entry.completed_at = None;
                        entry.sub_spec = Some(Box::new(child_spec.clone()));
                    }

                    self.emit_event(OrchestratorEvent::ChildSpecPushed {
                        parent_step_index: spec.steps[step_idx].index.clone(),
                        child_spec_id: child_spec.id.clone(),
                        child_step_count: child_spec.steps.len(),
                    });

                    self.stack.push((spec, step_idx, prefix));
                    self.stack.push((child_spec, 0, current_prefix.clone()));
                    continue;
                }
            }

            if task.status.state == TaskState::Failed {
                if let Some(entry) = spec.steps.get_mut(step_idx) {
                    entry.status = StepStatus::Failed;
                    entry.completed_at = Some(chrono::Utc::now());
                }
                self.emit_event(OrchestratorEvent::StepStatusChanged {
                    spec_id: spec.id.clone(),
                    spec_title: spec.title.clone(),
                    step_index: spec.steps[step_idx].index.clone(),
                    step_title: spec.steps[step_idx].title.clone(),
                    prefix: current_prefix.clone(),
                    role: StepRole::Implementor,
                    status: StepStatus::Failed,
                });
                self.main_agent
                    .update_spec_status(&spec, &spec.steps[step_idx], &current_prefix)
                    .await?;
                self.stack.push((spec, step_idx + 1, prefix));
                continue;
            }

            let tool_log = Self::format_tool_log(&task);
            let mut summary = Self::extract_summary(&task)?;
            let mut summarizer_error: Option<String> = None;

            if in_progress_step.requires_verification {
                let workspace_root = resolve_workspace_root().display().to_string();
                let context = SummarizerContext {
                    spec_id: spec.id.clone(),
                    spec_title: spec.title.clone(),
                    step: in_progress_step.clone(),
                    prefix: current_prefix.clone(),
                    summary: summary.clone(),
                    tool_log: tool_log.clone(),
                    workspace_root,
                };

                match self.run_summarizer_agent(&context).await {
                    Ok(mut summarized) => {
                        summarized.step_index = in_progress_step.index.clone();
                        summarized.task_id = summary.task_id.clone();
                        summarized.artifacts_touched = summary.artifacts_touched.clone();
                        summarized.tests_run = summary.tests_run.clone();
                        summarized.worktree = summary.worktree.clone();
                        summary = summarized;
                    }
                    Err(err) => {
                        summarizer_error = Some(err.to_string());
                    }
                }
            }

            let disposition = if let Some(message) = summarizer_error {
                summary.verification.status = VerificationStatus::Failed;
                summary.verification.feedback.push(FeedbackEntry {
                    author: "summarizer".to_string(),
                    message: message.clone(),
                    timestamp: Utc::now(),
                });
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.emit_event(OrchestratorEvent::VerifierFailed {
                    summary: summary.clone(),
                    feedback: message.clone(),
                });
                self.main_agent
                    .send_task_message(&summary.task_id, &message)
                    .await?;
                StepDisposition::Retry
            } else {
                self.verify_and_feedback(
                    summary,
                    &spec,
                    &in_progress_step,
                    &current_prefix,
                    tool_log,
                )
                .await?
            };

            match disposition {
                StepDisposition::Retry => {
                    if let Some(entry) = spec.steps.get_mut(step_idx) {
                        entry.status = StepStatus::Pending;
                        entry.completed_at = None;
                    }

                    self.emit_event(OrchestratorEvent::StepStatusChanged {
                        spec_id: spec.id.clone(),
                        spec_title: spec.title.clone(),
                        step_index: spec.steps[step_idx].index.clone(),
                        step_title: spec.steps[step_idx].title.clone(),
                        prefix: current_prefix.clone(),
                        role: StepRole::Implementor,
                        status: StepStatus::Pending,
                    });

                    let pending_step = spec
                        .steps
                        .get(step_idx)
                        .cloned()
                        .expect("step index validated above");
                    self.main_agent
                        .update_spec_status(&spec, &pending_step, &current_prefix)
                        .await?;
                    self.stack.push((spec, step_idx, prefix));
                    continue;
                }
                StepDisposition::Success(summary) => {
                    self.last_summary = Some(summary.clone());

                    let child_spec = Self::extract_sub_spec(&task)?;
                    if let Some(ref sub_spec) = child_spec {
                        spec.steps[step_idx].sub_spec = Some(Box::new(sub_spec.clone()));
                    }

                    spec.steps[step_idx].status = StepStatus::Completed;
                    spec.steps[step_idx].completed_at = Some(Utc::now());
                    Self::append_summary_to_history(&mut spec, &summary)?;

                    self.emit_event(OrchestratorEvent::StepStatusChanged {
                        spec_id: spec.id.clone(),
                        spec_title: spec.title.clone(),
                        step_index: spec.steps[step_idx].index.clone(),
                        step_title: spec.steps[step_idx].title.clone(),
                        prefix: current_prefix.clone(),
                        role: StepRole::Implementor,
                        status: StepStatus::Completed,
                    });

                    let completed_step = spec.steps[step_idx].clone();
                    self.main_agent
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
                StepDisposition::Fail(summary) => {
                    self.last_summary = Some(summary.clone());
                    spec.steps[step_idx].status = StepStatus::Failed;
                    spec.steps[step_idx].completed_at = Some(Utc::now());
                    Self::append_summary_to_history(&mut spec, &summary)?;

                    self.emit_event(OrchestratorEvent::StepStatusChanged {
                        spec_id: spec.id.clone(),
                        spec_title: spec.title.clone(),
                        step_index: spec.steps[step_idx].index.clone(),
                        step_title: spec.steps[step_idx].title.clone(),
                        prefix: current_prefix.clone(),
                        role: StepRole::Implementor,
                        status: StepStatus::Failed,
                    });

                    let failed_step = spec.steps[step_idx].clone();
                    self.main_agent
                        .update_spec_status(&spec, &failed_step, &current_prefix)
                        .await?;
                }
            }
        }

        self.emit_event(OrchestratorEvent::Completed);
        Ok(())
    }

    pub async fn run_parallel(&mut self) -> Result<()> {
        self.run().await
    }

    async fn verify_and_feedback(
        &mut self,
        summary: TaskSummary,
        spec: &SpecSheet,
        step: &SpecStep,
        prefix: &str,
        tool_log: String,
    ) -> Result<StepDisposition> {
        let mut summary = summary;

        if !step.requires_verification {
            summary.verification.status = VerificationStatus::Passed;
            self.main_agent.update_task_summary(&summary).await?;
            self.emit_event(OrchestratorEvent::SummaryUpdated {
                summary: summary.clone(),
            });
            self.main_agent.close_task_channel(&summary.task_id).await?;
            self.emit_event(OrchestratorEvent::ChannelClosed {
                task_id: summary.task_id.clone(),
                closed_at: Utc::now(),
            });
            self.main_agent.notify_step_success(&summary).await?;
            return Ok(StepDisposition::Success(summary));
        }

        summary.verification.status = VerificationStatus::Pending;
        self.main_agent.update_task_summary(&summary).await?;

        self.emit_event(OrchestratorEvent::SummaryUpdated {
            summary: summary.clone(),
        });

        let workspace_root = resolve_workspace_root().display().to_string();
        let context = VerificationContext {
            spec_id: spec.id.clone(),
            spec_title: spec.title.clone(),
            step: step.clone(),
            prefix: prefix.to_string(),
            summary: summary.clone(),
            tool_log,
            workspace_root,
        };
        self.last_verification = Some(context.clone());

        let payload = self.run_verifier_agent(&context).await;
        let (outcome, feedback) = match payload {
            Ok(payload) => (
                VerifierChain::map_verification_outcome(&payload.status),
                payload.feedback,
            ),
            Err(err) => (VerificationOutcome::Failed, Some(err.to_string())),
        };

        match outcome {
            VerificationOutcome::Verified => {
                summary.verification.status = VerificationStatus::Passed;
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.main_agent.close_task_channel(&summary.task_id).await?;
                self.emit_event(OrchestratorEvent::ChannelClosed {
                    task_id: summary.task_id.clone(),
                    closed_at: Utc::now(),
                });
                self.main_agent.notify_step_success(&summary).await?;
                if let Some(last) = self.last_verification.as_mut() {
                    last.summary = summary.clone();
                }
                Ok(StepDisposition::Success(summary))
            }
            VerificationOutcome::NeedsRevision => {
                let message = feedback
                    .unwrap_or_else(|| "Verifier requested changes before completion".to_string());
                summary.verification.status = VerificationStatus::Failed;
                summary.verification.feedback.push(FeedbackEntry {
                    author: "verifier".to_string(),
                    message: message.clone(),
                    timestamp: Utc::now(),
                });
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.emit_event(OrchestratorEvent::VerifierFailed {
                    summary: summary.clone(),
                    feedback: message.clone(),
                });
                self.main_agent
                    .send_task_message(&summary.task_id, &message)
                    .await?;
                if let Some(last) = self.last_verification.as_mut() {
                    last.summary = summary.clone();
                }
                Ok(StepDisposition::Retry)
            }
            VerificationOutcome::Failed => {
                let message =
                    feedback.unwrap_or_else(|| "Verifier rejected the changes".to_string());
                summary.verification.status = VerificationStatus::Failed;
                summary.verification.feedback.push(FeedbackEntry {
                    author: "verifier".to_string(),
                    message: message.clone(),
                    timestamp: Utc::now(),
                });
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.emit_event(OrchestratorEvent::VerifierFailed {
                    summary: summary.clone(),
                    feedback: message.clone(),
                });
                self.main_agent
                    .send_task_message(&summary.task_id, &message)
                    .await?;
                if let Some(last) = self.last_verification.as_mut() {
                    last.summary = summary.clone();
                }
                Ok(StepDisposition::Fail(summary))
            }
        }
    }

    async fn run_summarizer_agent(&mut self, context: &SummarizerContext) -> Result<TaskSummary> {
        let summarizer_spec = build_summarizer_spec(context);
        let summarizer_step = summarizer_spec
            .steps
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("Summarizer spec had no steps"))?;
        let summarizer_prefix = format!("{}.summary", context.prefix);

        self.emit_event(OrchestratorEvent::StepStatusChanged {
            spec_id: context.spec_id.clone(),
            spec_title: context.spec_title.clone(),
            step_index: summarizer_step.index.clone(),
            step_title: summarizer_step.title.clone(),
            prefix: summarizer_prefix.clone(),
            role: StepRole::Summarizer,
            status: StepStatus::InProgress,
        });

        let workspace_path = PathBuf::from(&context.workspace_root);
        let mut summarizer_agent = (self.sub_agent_factory)(&summarizer_step, Some(workspace_path));
        if let Some(agent) = summarizer_agent
            .as_any()
            .and_then(|any| any.downcast_ref::<Agent>())
        {
            let tools = crate::tools::get_readonly_tools();
            summarizer_agent =
                Arc::new(agent.with_tools(tools).await?) as Arc<dyn OrchestratorAgent>;
        }

        let summarizer_task = summarizer_agent
            .execute_step_with_events(
                summarizer_step.clone(),
                &summarizer_spec,
                &summarizer_prefix,
                self.event_tx.clone(),
            )
            .await?;

        if summarizer_task.status.state == TaskState::Failed {
            self.emit_event(OrchestratorEvent::StepStatusChanged {
                spec_id: context.spec_id.clone(),
                spec_title: context.spec_title.clone(),
                step_index: summarizer_step.index.clone(),
                step_title: summarizer_step.title.clone(),
                prefix: summarizer_prefix.clone(),
                role: StepRole::Summarizer,
                status: StepStatus::Failed,
            });
            return Err(anyhow!("Summarizer task failed"));
        }

        let mut summary = Self::extract_summary(&summarizer_task)?;
        summary.task_id = context.summary.task_id.clone();
        summary.artifacts_touched = context.summary.artifacts_touched.clone();
        summary.tests_run = context.summary.tests_run.clone();
        summary.worktree = context.summary.worktree.clone();

        self.emit_event(OrchestratorEvent::StepStatusChanged {
            spec_id: context.spec_id.clone(),
            spec_title: context.spec_title.clone(),
            step_index: summarizer_step.index.clone(),
            step_title: summarizer_step.title.clone(),
            prefix: summarizer_prefix,
            role: StepRole::Summarizer,
            status: StepStatus::Completed,
        });

        Ok(summary)
    }

    async fn run_verifier_agent(
        &mut self,
        context: &VerificationContext,
    ) -> Result<VerificationToolPayload> {
        let verifier_spec = build_verifier_spec(context);
        let verifier_step = verifier_spec
            .steps
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("Verifier spec had no steps"))?;
        let verifier_prefix = format!("{}.verify", context.prefix);

        self.emit_event(OrchestratorEvent::StepStatusChanged {
            spec_id: context.spec_id.clone(),
            spec_title: context.spec_title.clone(),
            step_index: verifier_step.index.clone(),
            step_title: verifier_step.title.clone(),
            prefix: verifier_prefix.clone(),
            role: StepRole::Verifier,
            status: StepStatus::InProgress,
        });

        let workspace_path = PathBuf::from(&context.workspace_root);
        let mut verifier_agent = (self.sub_agent_factory)(&verifier_step, Some(workspace_path));
        if let Some(agent) = verifier_agent
            .as_any()
            .and_then(|any| any.downcast_ref::<Agent>())
        {
            let tools = crate::tools::get_verifier_tools();
            verifier_agent = Arc::new(agent.with_tools(tools).await?) as Arc<dyn OrchestratorAgent>;
        }
        let verifier_task = verifier_agent
            .execute_step_with_events(
                verifier_step.clone(),
                &verifier_spec,
                &verifier_prefix,
                self.event_tx.clone(),
            )
            .await?;

        if verifier_task.status.state == TaskState::Failed {
            self.emit_event(OrchestratorEvent::StepStatusChanged {
                spec_id: context.spec_id.clone(),
                spec_title: context.spec_title.clone(),
                step_index: verifier_step.index.clone(),
                step_title: verifier_step.title.clone(),
                prefix: verifier_prefix.clone(),
                role: StepRole::Verifier,
                status: StepStatus::Failed,
            });
            return Err(anyhow!("Verifier task failed"));
        }

        let payload = VerifierChain::extract_verification_payload(&verifier_task)?;
        if !payload.end_convo {
            return Err(anyhow!("submit_verification must set end_convo=true"));
        }

        self.emit_event(OrchestratorEvent::StepStatusChanged {
            spec_id: context.spec_id.clone(),
            spec_title: context.spec_title.clone(),
            step_index: verifier_step.index.clone(),
            step_title: verifier_step.title.clone(),
            prefix: verifier_prefix,
            role: StepRole::Verifier,
            status: StepStatus::Completed,
        });
        Ok(payload)
    }

    async fn rerun_last_verifier(&mut self) -> Result<()> {
        let Some(context) = self.last_verification.clone() else {
            return Ok(());
        };

        let payload = self.run_verifier_agent(&context).await;
        let (outcome, feedback) = match payload {
            Ok(payload) => (
                VerifierChain::map_verification_outcome(&payload.status),
                payload.feedback,
            ),
            Err(err) => (VerificationOutcome::Failed, Some(err.to_string())),
        };

        let mut summary = context.summary.clone();
        match outcome {
            VerificationOutcome::Verified => {
                summary.verification.status = VerificationStatus::Passed;
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.main_agent.notify_step_success(&summary).await?;
            }
            VerificationOutcome::NeedsRevision | VerificationOutcome::Failed => {
                let message =
                    feedback.unwrap_or_else(|| "Verifier rejected the changes".to_string());
                summary.verification.status = VerificationStatus::Failed;
                summary.verification.feedback.push(FeedbackEntry {
                    author: "verifier".to_string(),
                    message: message.clone(),
                    timestamp: Utc::now(),
                });
                self.main_agent.update_task_summary(&summary).await?;
                self.emit_event(OrchestratorEvent::SummaryUpdated {
                    summary: summary.clone(),
                });
                self.emit_event(OrchestratorEvent::VerifierFailed {
                    summary: summary.clone(),
                    feedback: message.clone(),
                });
                self.main_agent
                    .send_task_message(&summary.task_id, &message)
                    .await?;
            }
        }

        self.last_summary = Some(summary.clone());
        if let Some(last) = self.last_verification.as_mut() {
            last.summary = summary;
        }

        Ok(())
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

    fn format_tool_log(task: &Task) -> String {
        orchestrator_support::format_tool_log(task)
    }
}

#[async_trait]
impl OrchestratorAgent for Agent {
    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

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

    async fn execute_step_with_events(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    ) -> Result<Task> {
        self.execute_step_with_events(step, spec, prefix, event_tx)
            .await
    }

    async fn execute_step_with_events_and_cancel(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
        cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Result<Task> {
        self.execute_step_with_events_and_cancel(step, spec, prefix, event_tx, cancel_rx)
            .await
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
