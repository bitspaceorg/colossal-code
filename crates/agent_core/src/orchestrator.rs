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
use serde_yaml;
use tokio::sync::mpsc;

use crate::{Agent, resolve_workspace_root};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationOutcome {
    Verified,
    NeedsRevision,
    Failed,
}

#[derive(Debug, Deserialize)]
struct VerificationToolPayload {
    status: String,
    #[serde(default)]
    feedback: Option<String>,
    #[serde(default)]
    end_convo: bool,
}

#[derive(Clone)]
struct VerificationContext {
    spec_id: String,
    spec_title: String,
    step: SpecStep,
    prefix: String,
    summary: TaskSummary,
    tool_log: String,
    workspace_root: String,
}

#[derive(Clone)]
struct SummarizerContext {
    spec_id: String,
    spec_title: String,
    step: SpecStep,
    prefix: String,
    summary: TaskSummary,
    tool_log: String,
    workspace_root: String,
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

    pub async fn run(&self, summary: &TaskSummary) -> std::result::Result<(), FeedbackEntry> {
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
        Arc<dyn Fn(&SpecStep, Option<PathBuf>) -> Arc<dyn OrchestratorAgent> + Send + Sync>,
    verifier_chain: VerifierChain,
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
            verifier_chain: VerifierChain::default(),
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
            verifier_chain: VerifierChain::default(),
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
        // Collect commands first to avoid borrow issues
        let commands: Vec<OrchestratorCommand> = if let Some(ref mut rx) = self.command_rx {
            let mut cmds = Vec::new();
            while let Ok(cmd) = rx.try_recv() {
                cmds.push(cmd);
            }
            cmds
        } else {
            Vec::new()
        };

        // Process collected commands
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
                    // Push the child spec onto the stack with the proper prefix
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
                return true; // Aborted
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        false
    }

    pub async fn run(&mut self) -> Result<()> {
        let workspace_root = resolve_workspace_root();
        while let Some((mut spec, step_idx, prefix)) = self.stack.pop() {
            // Check for abort before processing each step
            if self.process_commands().await {
                return Ok(()); // Aborted
            }

            // Wait if paused
            if self.wait_while_paused().await {
                return Ok(()); // Aborted during pause
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

            // Emit step status change event
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
                return Ok(()); // Aborted
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

                    // Emit child spec pushed event
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

            // Skip verification for failed tasks - just mark step as failed
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
                // Continue to next step
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

                    // Emit retry event
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
                    // Store summary for potential rerun
                    self.last_summary = Some(summary.clone());

                    let child_spec = Self::extract_sub_spec(&task)?;
                    if let Some(ref sub_spec) = child_spec {
                        spec.steps[step_idx].sub_spec = Some(Box::new(sub_spec.clone()));
                    }

                    spec.steps[step_idx].status = StepStatus::Completed;
                    spec.steps[step_idx].completed_at = Some(Utc::now());
                    Self::append_summary_to_history(&mut spec, &summary)?;

                    // Emit completion event
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

        // Emit completed event
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
                Self::map_verification_outcome(&payload.status),
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

        let payload = Self::extract_verification_payload(&verifier_task)?;
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
                Self::map_verification_outcome(&payload.status),
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

    fn map_verification_outcome(status: &str) -> VerificationOutcome {
        match status.trim().to_ascii_lowercase().as_str() {
            "verified" => VerificationOutcome::Verified,
            "needs_revision" | "needs-revision" | "revision" | "retry" => {
                VerificationOutcome::NeedsRevision
            }
            _ => VerificationOutcome::Failed,
        }
    }

    fn extract_verification_payload(task: &Task) -> Result<VerificationToolPayload> {
        let tool_log = task
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.extra.get("toolLog"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow!("Verifier did not record tool calls"))?;

        let entry = tool_log
            .iter()
            .rev()
            .find(|entry| {
                entry.get("name").and_then(|value| value.as_str()) == Some("submit_verification")
            })
            .ok_or_else(|| anyhow!("Verifier must call submit_verification"))?;

        let arguments = entry
            .get("arguments")
            .and_then(|value| value.as_str())
            .unwrap_or("{}");
        serde_json::from_str::<VerificationToolPayload>(arguments)
            .or_else(|_| serde_yaml::from_str::<VerificationToolPayload>(arguments))
            .context("Failed to parse submit_verification payload")
    }

    fn format_tool_log(task: &Task) -> String {
        task.metadata
            .as_ref()
            .and_then(|metadata| metadata.extra.get("toolLog"))
            .and_then(|value| value.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let name = entry.get("name").and_then(|v| v.as_str())?;
                        let args = entry
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        Some(format!("- {} {}", name, args))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "(no tool calls recorded)".to_string())
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

fn build_summarizer_spec(context: &SummarizerContext) -> SpecSheet {
    SpecSheet {
        id: format!("{}::summary", context.spec_id),
        title: format!("Summary for {}", context.spec_title),
        description: "Summarize implementation for verification".to_string(),
        steps: vec![build_summarizer_step(context)],
        created_by: "summarizer".to_string(),
        created_at: Utc::now(),
        metadata: Value::Null,
    }
}

fn build_summarizer_step(context: &SummarizerContext) -> SpecStep {
    SpecStep {
        index: "summary".to_string(),
        title: format!("Summarize – {}", context.step.title),
        instructions: build_summarizer_instructions(context),
        acceptance_criteria: vec![
            "Provide a concise, verifier-ready summary".to_string(),
            "Mention any tests run and files touched if known".to_string(),
        ],
        required_tools: vec![],
        constraints: vec![
            "Do not modify files during summarization".to_string(),
            "Use tools only for inspection if needed".to_string(),
        ],
        dependencies: vec![],
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
        status: StepStatus::Pending,
        sub_spec: None,
        completed_at: None,
    }
}

fn build_summarizer_instructions(context: &SummarizerContext) -> String {
    let acceptance = if context.step.acceptance_criteria.is_empty() {
        "(no acceptance criteria provided)".to_string()
    } else {
        context
            .step
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let constraints = if context.step.constraints.is_empty() {
        "(no additional constraints)".to_string()
    } else {
        context
            .step
            .constraints
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## Role\nYou are summarizing spec \"{}\" step {} – {} for verification.\n\n### Workspace Root\n{}\n\n### Step Instructions\n{}\n\n### Acceptance Criteria\n{}\n\n### Constraints\n{}\n\n### Implementor Summary\n{}\n\n### Tool Calls Used\n{}\n\nProduce a concise summary that a verifier can use to validate the changes. Focus on the key modifications, commands run, and artifacts touched.",
        context.spec_title,
        context.step.index,
        context.step.title,
        context.workspace_root,
        context.step.instructions,
        acceptance,
        constraints,
        context.summary.summary_text,
        context.tool_log,
    )
}

fn build_verifier_spec(context: &VerificationContext) -> SpecSheet {
    SpecSheet {
        id: format!("{}::verifier", context.spec_id),
        title: format!("Verification for {}", context.spec_title),
        description: "Verification step".to_string(),
        steps: vec![build_verifier_step(context)],
        created_by: "verifier".to_string(),
        created_at: Utc::now(),
        metadata: Value::Null,
    }
}

fn build_verifier_step(context: &VerificationContext) -> SpecStep {
    SpecStep {
        index: "1".to_string(),
        title: format!("Verify – {}", context.step.title),
        instructions: build_verifier_instructions(context),
        acceptance_criteria: vec![
            "Inspect the implementation using tools and run relevant checks".to_string(),
            "Call submit_verification with the final outcome".to_string(),
        ],
        required_tools: vec!["submit_verification".to_string()],
        constraints: vec![
            "Do not modify files during verification".to_string(),
            "Use tools to read files and run commands as needed".to_string(),
        ],
        dependencies: vec![],
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
        status: StepStatus::Pending,
        sub_spec: None,
        completed_at: None,
    }
}

fn build_verifier_instructions(context: &VerificationContext) -> String {
    let acceptance = if context.step.acceptance_criteria.is_empty() {
        "(no acceptance criteria provided)".to_string()
    } else {
        context
            .step
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let constraints = if context.step.constraints.is_empty() {
        "(no additional constraints)".to_string()
    } else {
        context
            .step
            .constraints
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## Role\nYou are verifying spec \"{}\" step {} – {}.\n\n### Workspace Root\n{}\n\n### Step Instructions\n{}\n\n### Acceptance Criteria\n{}\n\n### Constraints\n{}\n\n### Implementor Summary\n{}\n\n### Tool Calls Used\n{}\n\nUse tools to inspect the codebase, run any relevant checks (tests, builds, linters), and confirm the implementation matches the instructions. Do not modify files.\n\nWhen verification is complete, call submit_verification with status verified, needs_revision, or failed, include feedback, and set end_convo to true.",
        context.spec_title,
        context.step.index,
        context.step.title,
        context.workspace_root,
        context.step.instructions,
        acceptance,
        constraints,
        context.summary.summary_text,
        context.tool_log,
    )
}

enum StepDisposition {
    Retry,
    Success(TaskSummary),
    Fail(TaskSummary),
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
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

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

    struct BlockingSubAgent {
        cancelled: Arc<AtomicBool>,
        started_tx: tokio::sync::mpsc::UnboundedSender<()>,
    }

    impl BlockingSubAgent {
        fn new(
            cancelled: Arc<AtomicBool>,
            started_tx: tokio::sync::mpsc::UnboundedSender<()>,
        ) -> Self {
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
                self.cancelled.store(true, Ordering::SeqCst);
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
        let mut step = make_step("1", None);
        step.requires_verification = true;
        let spec = make_spec("root", vec![step.clone()]);
        let tasks = VecDeque::from(vec![
            ("root:1".to_string(), make_task("task-1", "1", None)),
            (
                "root::verifier:1".to_string(),
                make_verifier_task("root", "needs_revision", "try again"),
            ),
            ("root:1".to_string(), make_task("task-1b", "1", None)),
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
        assert_eq!(executions, vec!["root:1", "root:1"]);
        let messages = main_agent.messages();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].1.contains("try again"));
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
        let spec = make_spec("root", vec![make_step("1", None), make_step("2", None)]);
        let child_spec = make_spec("child", vec![make_step("1", None)]);
        let tasks = VecDeque::from(vec![
            ("root:1".to_string(), make_task("task-1", "1", None)),
            ("child:1".to_string(), make_task("child-task-1", "1", None)),
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
        let spec = make_spec("root", vec![make_step("1", None)]);
        let main_agent = Arc::new(MockMainAgent::default());
        let main_agent_trait = main_agent.clone() as Arc<dyn OrchestratorAgent>;

        let (start_tx, mut start_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));
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

        assert!(cancelled.load(Ordering::SeqCst));
    }
}
