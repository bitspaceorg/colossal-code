use std::{collections::HashMap, sync::Arc};

use agent_protocol::{
    error::A2AResult,
    jsonrpc::StreamEvent,
    server::{A2AHandler, A2AServer},
    types::{
        agent_card::AgentCard,
        message::Message,
        spec::{SpecSheet, SpecStep, SpecStepRef, TaskSummary},
        task::{Task, TaskError, TaskMetadata, TaskState, TaskStatusUpdateEvent},
    },
    A2AError,
};
use async_trait::async_trait;
use anyhow::anyhow;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::{
    runtime::Builder,
    sync::{mpsc, RwLock},
};
use uuid::Uuid;
use tracing::{error, info, warn};

use crate::{Agent, AgentMessage};

const SPLIT_DIRECTIVE: &str = "__nite_split__";

struct TaskContext {
    _spec: SpecSheet,
    _step: SpecStep,
    cleanup_actions: Vec<Box<dyn FnOnce() + Send + Sync>>,
}

impl TaskContext {
    fn new(spec: SpecSheet, step: SpecStep) -> Self {
        Self {
            _spec: spec,
            _step: step,
            cleanup_actions: Vec::new(),
        }
    }

    fn register_cleanup<F>(&mut self, cleanup: F)
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        self.cleanup_actions.push(Box::new(cleanup));
    }

    fn cleanup(&mut self) {
        for action in self.cleanup_actions.drain(..) {
            action();
        }
    }
}

fn parse_spec_sheet(metadata: Option<&Value>) -> A2AResult<SpecSheet> {
    let spec_value = metadata
        .and_then(|value| {
            value
                .get("specSheet")
                .or_else(|| value.get("spec_sheet"))
                .cloned()
        })
        .ok_or_else(|| {
            A2AError::InvalidParams("message metadata missing specSheet".to_string())
        })?;
    serde_json::from_value(spec_value).map_err(|err| {
        A2AError::InvalidParams(format!("invalid specSheet metadata: {err}"))
    })
}

fn parse_spec_step(
    metadata: Option<&Value>,
    fallback: Option<SpecStepRef>,
) -> A2AResult<SpecStepRef> {
    if let Some(step_value) = metadata
        .and_then(|value| value.get("specStep").or_else(|| value.get("spec_step")))
        .cloned()
    {
        return serde_json::from_value(step_value).map_err(|err| {
            A2AError::InvalidParams(format!("invalid specStep metadata: {err}"))
        });
    }

    if let Some(step) = fallback {
        warn!("specStep metadata missing; falling back to RPC params");
        return Ok(step);
    }

    Err(A2AError::InvalidParams(
        "specStep metadata is required".to_string(),
    ))
}

fn resolve_step(spec: &SpecSheet, step_ref: &SpecStepRef) -> A2AResult<SpecStep> {
    spec
        .steps
        .iter()
        .find(|step| step.index == step_ref.index)
        .cloned()
        .ok_or_else(|| {
            A2AError::InvalidParams(format!(
                "spec {} missing step {}",
                spec.id, step_ref.index
            ))
        })
}

fn split_requested(metadata: Option<&Value>) -> bool {
    metadata
        .and_then(|value| value.get("split").or_else(|| value.get("splitRequested")))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn resolve_task_id(task_id: Option<String>, message_task_id: Option<String>) -> String {
    task_id
        .or(message_task_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn prepare_spec_and_step(
    message: &Message,
    spec_step: Option<SpecStepRef>,
) -> A2AResult<(SpecSheet, SpecStep)> {
    let spec = parse_spec_sheet(message.metadata.as_ref())?;
    let step_ref = parse_spec_step(message.metadata.as_ref(), spec_step)?;
    if spec.id != step_ref.spec_id {
        return Err(A2AError::InvalidParams(format!(
            "specStep spec_id {} does not match specSheet {}",
            step_ref.spec_id, spec.id
        )));
    }

    let base_step = resolve_step(&spec, &step_ref)?;
    let mut prepared_step = append_user_context(&base_step, message);
    if split_requested(message.metadata.as_ref()) {
        prepared_step
            .constraints
            .push(SPLIT_DIRECTIVE.to_string());
    }

    Ok((spec, prepared_step))
}

fn append_user_context(step: &SpecStep, message: &Message) -> SpecStep {
    let mut updated = step.clone();
    let trimmed = message.text_content();
    if trimmed.trim().is_empty() {
        return updated;
    }
    updated.instructions = format!(
        "{}\n\nUser context:\n{}",
        updated.instructions, trimmed.trim()
    );
    updated
}

#[derive(Clone, Default)]
struct TaskStreamManager {
    streams: Arc<RwLock<HashMap<String, HashMap<String, mpsc::Sender<StreamEvent>>>>>,
}

impl TaskStreamManager {
    fn new() -> Self {
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn register(
        &self,
        task_id: &str,
    ) -> (String, mpsc::Sender<StreamEvent>, mpsc::Receiver<StreamEvent>) {
        let (tx, rx) = mpsc::channel(32);
        let subscriber_id = Uuid::new_v4().to_string();
        {
            let mut guard = self.streams.write().await;
            guard
                .entry(task_id.to_string())
                .or_default()
                .insert(subscriber_id.clone(), tx.clone());
        }
        self.spawn_cleanup(task_id.to_string(), subscriber_id.clone(), tx.clone());
        (subscriber_id, tx, rx)
    }

    fn spawn_cleanup(
        &self,
        task_id: String,
        subscriber_id: String,
        sender: mpsc::Sender<StreamEvent>,
    ) {
        let streams = Arc::clone(&self.streams);
        tokio::spawn(async move {
            sender.closed().await;
            let mut guard = streams.write().await;
            if let Some(entry) = guard.get_mut(&task_id) {
                entry.remove(&subscriber_id);
                if entry.is_empty() {
                    guard.remove(&task_id);
                }
            }
        });
    }

    async fn remove(&self, task_id: &str, subscriber_id: &str) {
        let mut guard = self.streams.write().await;
        if let Some(entry) = guard.get_mut(task_id) {
            entry.remove(subscriber_id);
            if entry.is_empty() {
                guard.remove(task_id);
            }
        }
    }

    async fn send(&self, task_id: &str, event: StreamEvent) {
        let targets = {
            let guard = self.streams.read().await;
            guard.get(task_id).map(|entry| {
                entry
                    .iter()
                    .map(|(id, sender)| (id.clone(), sender.clone()))
                    .collect::<Vec<_>>()
            })
        };

        if let Some(subscribers) = targets {
            let mut failed = Vec::new();
            for (id, sender) in subscribers {
                if sender.send(event.clone()).await.is_err() {
                    failed.push(id);
                }
            }

            if !failed.is_empty() {
                let mut guard = self.streams.write().await;
                if let Some(entry) = guard.get_mut(task_id) {
                    for id in failed {
                        entry.remove(&id);
                    }
                    if entry.is_empty() {
                        guard.remove(task_id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
impl TaskStreamManager {
    async fn subscriber_count(&self, task_id: &str) -> usize {
        let guard = self.streams.read().await;
        guard.get(task_id).map(|entry| entry.len()).unwrap_or(0)
    }
}

/// Handler that bridges the interactive [`Agent`] with the A2A protocol server.
#[derive(Clone)]
pub struct AgentA2AHandler {
    agent: Agent,
    tasks: Arc<RwLock<HashMap<String, Task>>>,
    task_contexts: Arc<RwLock<HashMap<String, TaskContext>>>,
    stream_manager: TaskStreamManager,
}

impl AgentA2AHandler {
    fn base_metadata_for_step(step: &SpecStep) -> TaskMetadata {
        let mut metadata = TaskMetadata::default();
        metadata
            .extra
            .insert("stepIndex".to_string(), json!(step.index));
        metadata
            .extra
            .insert("stepTitle".to_string(), json!(step.title));
        metadata
            .extra
            .insert("stepInstructions".to_string(), json!(step.instructions));
        metadata
    }

    fn store_summary_metadata(
        metadata: &mut TaskMetadata,
        summary: &TaskSummary,
    ) -> serde_json::Result<()> {
        metadata.summary = Some(serde_json::to_value(summary)?);
        Ok(())
    }

    fn apply_split_metadata(
        metadata: &mut TaskMetadata,
        split_spec: &SpecSheet,
        summary: &TaskSummary,
    ) -> serde_json::Result<()> {
        metadata.spec_sheet = Some(serde_json::to_value(split_spec.clone())?);
        Self::store_summary_metadata(metadata, summary)
    }

    fn mark_split_task(task: &mut Task) {
        task.status.state = TaskState::Submitted;
        task.status.timestamp = Some(Utc::now());
        task.status.message = Some("split".to_string());
        task.status.error = None;
    }

    fn extract_split_reason(result: &str) -> Option<String> {
        if let Ok(payload) = serde_yaml::from_str::<serde_json::Value>(result) {
            if let Some(reason) = payload.get("reason").and_then(|value| value.as_str()) {
                let trimmed = reason.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    /// Create a new handler for the given agent
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            task_contexts: Arc::new(RwLock::new(HashMap::new())),
            stream_manager: TaskStreamManager::new(),
        }
    }

    /// Store the current task snapshot for later queries
    async fn persist_task(&self, task: Task) {
        self
            .stream_manager
            .send(&task.id, StreamEvent::task(task.clone()))
            .await;
        self.tasks.write().await.insert(task.id.clone(), task);
    }

    async fn emit_status_update(&self, task: &Task, final_update: bool) {
        let event = TaskStatusUpdateEvent {
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            status: task.status.clone(),
            final_update,
        };
        self
            .stream_manager
            .send(&task.id, StreamEvent::status_update(event))
            .await;
    }

    async fn emit_message_event(&self, task_id: &str, message: &Message) {
        let mut enriched = message.clone();
        if enriched.task_id.is_none() {
            enriched.task_id = Some(task_id.to_string());
        }
        self
            .stream_manager
            .send(task_id, StreamEvent::message(enriched))
            .await;
    }

    /// Run the core agent for the given message and return a completed task record
    async fn execute_task(
        &self,
        requested_task_id: Option<String>,
        step: &SpecStep,
        spec: &SpecSheet,
    ) -> A2AResult<Task> {
        let mut task = if let Some(id) = requested_task_id {
            Task::with_id(id)
        } else {
            Task::new()
        };
        task.context_id = Some(spec.id.clone());
        task.status.state = TaskState::Working;
        task.status.timestamp = Some(Utc::now());
        task.status.message = Some(format!("Processing step {}", step.index));
        task.status.error = None;
        self.emit_status_update(&task, false).await;

        let mut metadata = Self::base_metadata_for_step(step);
        let mut stored_input = Message::user(step.instructions.clone());
        stored_input.task_id = Some(task.id.clone());
        stored_input.context_id = task.context_id.clone();
        self.emit_message_event(&task.id, &stored_input).await;
        task.messages.push(stored_input);

        let split_mode = step
            .constraints
            .iter()
            .any(|constraint| constraint == SPLIT_DIRECTIVE);
        if split_mode {
            return self
                .handle_split_task(task, metadata, step, spec, None)
                .await;
        }

        let prompt = step.instructions.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = self.agent.clone();
        let blocking = tokio::task::spawn_blocking(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| anyhow!("failed to create agent runtime: {e}"))?;
            runtime.block_on(async move { agent.process_message(prompt, tx).await })
        });

        let mut latest_response = String::new();
        let mut stats_payload = None;
        let mut error_message = None;
        let mut tool_events: Vec<(String, String, Option<String>)> = Vec::new();
        let mut split_requested_via_tool = false;
        let mut split_tool_reason: Option<String> = None;

        while let Some(agent_msg) = rx.recv().await {
            match agent_msg {
                AgentMessage::AgentResponse(content, _) => {
                    let chunk = content.clone();
                    latest_response = content;
                    let mut stream_message = Message::agent(chunk);
                    stream_message.task_id = Some(task.id.clone());
                    stream_message.context_id = task.context_id.clone();
                    self.emit_message_event(&task.id, &stream_message).await;
                }
                AgentMessage::Error(err) => {
                    error_message = Some(err);
                    break;
                }
                AgentMessage::GenerationStats(stats) => {
                    stats_payload = Some(stats);
                }
                AgentMessage::ToolCallStarted(name, args) => {
                    tool_events.push((name, args, None));
                }
                AgentMessage::ToolCallCompleted(name, result) => {
                    if let Some(entry) = tool_events.iter_mut().rev().find(|(existing, _, completed)| {
                        existing == &name && completed.is_none()
                    }) {
                        entry.2 = Some(result.clone());
                    }
                    if name == "request_split" {
                        split_requested_via_tool = true;
                        split_tool_reason = Self::extract_split_reason(&result);
                        self.agent.request_cancel();
                        break;
                    }
                }
                AgentMessage::Done => break,
                _ => {}
            }
        }

        match blocking.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => error_message = Some(err.to_string()),
            Err(err) => error_message = Some(err.to_string()),
        }

        if split_requested_via_tool {
            return self
                .handle_split_task(task, metadata, step, spec, split_tool_reason)
                .await;
        }

        if let Some(err) = error_message {
            task.status.state = TaskState::Failed;
            task.status.timestamp = Some(Utc::now());
            task.status.message = Some("Generation failed".to_string());
            task.status.error = Some(TaskError {
                code: -1,
                message: err.clone(),
                data: None,
            });
            self.emit_status_update(&task, true).await;
            self.persist_task(task.clone()).await;
            return Err(A2AError::InternalError(err));
        }

        if !latest_response.trim().is_empty() {
            let mut response = Message::agent(latest_response.clone());
            response.task_id = Some(task.id.clone());
            response.context_id = task.context_id.clone();
            self.emit_message_event(&task.id, &response).await;
            task.messages.push(response);
        }

        metadata.extra.insert(
            "toolLog".to_string(),
            json!(tool_events
                .into_iter()
                .map(|(name, args, result)| json!({
                    "name": name,
                    "arguments": args,
                    "result": result,
                }))
                .collect::<Vec<_>>()),
        );

        task.metadata = Some(metadata.clone());
        let summary = self.agent.collect_summary(&task);
        Self::store_summary_metadata(&mut metadata, &summary).map_err(|err| {
            A2AError::InternalError(format!("failed to serialize summary: {err}"))
        })?;
        task.metadata = Some(metadata);

        task.status.state = TaskState::Completed;
        task.status.timestamp = Some(Utc::now());
        task.status.message = Some("Response ready".to_string());
        self.emit_status_update(&task, true).await;
        if let Some(stats) = stats_payload {
            let stats_value = json!({
                "avgCompletionTokPerSec": stats.avg_completion_tok_per_sec,
                "completionTokens": stats.completion_tokens,
                "promptTokens": stats.prompt_tokens,
                "timeToFirstTokenSec": stats.time_to_first_token_sec,
                "stopReason": stats.stop_reason,
            });
            if let Some(metadata) = task.metadata.as_mut() {
                metadata
                    .extra
                    .insert("generationStats".to_string(), stats_value);
            }
        }

        self.persist_task(task.clone()).await;
        Ok(task)
    }

    async fn handle_split_task(
        &self,
        mut task: Task,
        mut metadata: TaskMetadata,
        step: &SpecStep,
        spec: &SpecSheet,
        split_reason: Option<String>,
    ) -> A2AResult<Task> {
        let split_spec = self
            .agent
            .request_split(step)
            .await
            .map_err(|err| A2AError::InternalError(format!("failed to request split: {err}")))?;

        let summary = Agent::synthesize_split_summary(&task, step, &split_spec);
        Self::apply_split_metadata(&mut metadata, &split_spec, &summary).map_err(|err| {
            A2AError::InternalError(format!("failed to serialize split metadata: {err}"))
        })?;
        task.metadata = Some(metadata);

        Self::mark_split_task(&mut task);

        let mut response = Message::agent(format!(
            "Step {} split into {} child steps for spec {}{}",
            step.index,
            split_spec.steps.len(),
            spec.id,
            split_reason
                .as_ref()
                .map(|reason| format!(" (reason: {})", reason))
                .unwrap_or_default()
        ));
        response.task_id = Some(task.id.clone());
        response.context_id = task.context_id.clone();
        self.emit_message_event(&task.id, &response).await;
        task.messages.push(response);
        self.emit_status_update(&task, true).await;
        Ok(task)
    }

    async fn cleanup_task_context(&self, task_id: &str) {
        let mut contexts = self.task_contexts.write().await;
        if let Some(context) = contexts.get_mut(task_id) {
            context.cleanup();
        }
        contexts.remove(task_id);
    }
 
    pub async fn register_cleanup_action<F>(&self, task_id: &str, cleanup: F) -> bool
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        let mut contexts = self.task_contexts.write().await;
        if let Some(context) = contexts.get_mut(task_id) {
            context.register_cleanup(cleanup);
            true
        } else {
            false
        }
    }
}

#[async_trait]
impl A2AHandler for AgentA2AHandler {
    async fn handle_message(
        &self,
        message: Message,
        task_id: Option<String>,
        spec_step: Option<SpecStepRef>,
    ) -> A2AResult<Task> {
        let (spec, prepared_step) = prepare_spec_and_step(&message, spec_step)?;
        let resolved_task_id = resolve_task_id(task_id, message.task_id.clone());

        info!(
            task_id = %resolved_task_id,
            spec = %spec.id,
            step = %prepared_step.index,
            "Executing spec step"
        );

        {
            let mut contexts = self.task_contexts.write().await;
            contexts.insert(
                resolved_task_id.clone(),
                TaskContext::new(spec.clone(), prepared_step.clone()),
            );
        }

        let task = match self
            .execute_task(Some(resolved_task_id.clone()), &prepared_step, &spec)
            .await
        {
            Ok(task) => task,
            Err(err) => {
                self.cleanup_task_context(&resolved_task_id).await;
                return Err(err);
            }
        };
        self.cleanup_task_context(&resolved_task_id).await;
        Ok(task)
    }

    async fn handle_streaming_message(
        &self,
        message: Message,
        task_id: Option<String>,
        spec_step: Option<SpecStepRef>,
    ) -> A2AResult<mpsc::Receiver<StreamEvent>> {
        let (spec, prepared_step) = prepare_spec_and_step(&message, spec_step)?;
        let resolved_task_id = resolve_task_id(task_id, message.task_id.clone());

        info!(
            task_id = %resolved_task_id,
            spec = %spec.id,
            step = %prepared_step.index,
            "Streaming spec step"
        );

        {
            let mut contexts = self.task_contexts.write().await;
            contexts.insert(
                resolved_task_id.clone(),
                TaskContext::new(spec.clone(), prepared_step.clone()),
            );
        }

        let (subscriber_id, _stream_sender, rx) = self.stream_manager.register(&resolved_task_id).await;
        let manager = self.stream_manager.clone();
        let task_for_cleanup = resolved_task_id.clone();
        let subscriber_for_cleanup = subscriber_id.clone();
        let _ = self
            .register_cleanup_action(&resolved_task_id, move || {
                let manager = manager.clone();
                let task = task_for_cleanup.clone();
                let subscriber = subscriber_for_cleanup.clone();
                tokio::spawn(async move {
                    manager.remove(&task, &subscriber).await;
                });
            })
            .await;

        let handler = self.clone();
        let stream_task_id = resolved_task_id.clone();
        tokio::spawn(async move {
            let result = handler
                .execute_task(Some(stream_task_id.clone()), &prepared_step, &spec)
                .await;
            if let Err(err) = result {
                error!(task_id = %stream_task_id, error = %err, "streaming execution failed");
            }
            handler.cleanup_task_context(&stream_task_id).await;
        });

        Ok(rx)
    }

    async fn get_task(&self, task_id: &str) -> A2AResult<Task> {
        self
            .tasks
            .read()
            .await
            .get(task_id)
            .cloned()
            .ok_or_else(|| A2AError::TaskNotFound(task_id.to_string()))
    }

    async fn list_tasks(
        &self,
        context_id: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> A2AResult<(Vec<Task>, usize)> {
        let mut tasks: Vec<Task> = self.tasks.read().await.values().cloned().collect();
        if let Some(ctx) = context_id {
            tasks.retain(|task| task.context_id.as_deref() == Some(ctx));
        }
        let total = tasks.len();
        let start = offset.unwrap_or(0).min(total);
        let end = limit
            .map(|limit| start.saturating_add(limit).min(total))
            .unwrap_or(total);
        Ok((tasks[start..end].to_vec(), total))
    }

    async fn cancel_task(&self, task_id: &str) -> A2AResult<Task> {
        self.agent.request_cancel();
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            if !task.status.state.is_terminal() {
                task.status.state = TaskState::Cancelled;
                task.status.timestamp = Some(Utc::now());
                task.status.message = Some("Cancelled by requester".to_string());
            }
            return Ok(task.clone());
        }
        Err(A2AError::TaskNotFound(task_id.to_string()))
    }

    async fn subscribe_to_task(
        &self,
        task_id: &str,
    ) -> A2AResult<mpsc::Receiver<StreamEvent>> {
        let current_task = {
            let tasks = self.tasks.read().await;
            tasks.get(task_id).cloned()
        };
        if current_task.is_none() {
            return Err(A2AError::TaskNotFound(task_id.to_string()));
        }

        let (_subscriber_id, sender, rx) = self.stream_manager.register(task_id).await;
        if let Some(task) = current_task {
            sender
                .send(StreamEvent::task(task))
                .await
                .map_err(|_| A2AError::InternalError("failed to initialize task stream".to_string()))?;
        }
        Ok(rx)
    }
}

/// Build an [`A2AServer`] that exposes the agent over HTTP using the provided agent card.
pub fn create_a2a_server(agent: Agent, agent_card: AgentCard) -> A2AServer<AgentA2AHandler> {
    let handler = AgentA2AHandler::new(agent);
    A2AServer::new(agent_card, handler)
}

/// Convenience helper to build a minimal agent card pointing at the given base URL.
pub fn default_agent_card(base_url: impl Into<String>) -> AgentCard {
    AgentCard::builder()
        .name("nite-agent")
        .description("Terminal-native coding agent")
        .base_url(base_url)
        .streaming(false)
        .skill("code", "Code Assistant", Some("General-purpose coding help".to_string()))
        .build()
        .expect("default agent card is always valid")
}

/// Simple smoke test that ensures the default card serializes correctly.
#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::types::spec::{
        SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, VerificationStatus,
    };
    use agent_protocol::types::task::{Task, TaskMetadata, TaskState};
    use chrono::Utc;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    fn sample_spec() -> SpecSheet {
        SpecSheet {
            id: "spec-1".to_string(),
            title: "Spec".to_string(),
            description: "Desc".to_string(),
            steps: vec![SpecStep {
                index: "1".to_string(),
                title: "Step".to_string(),
                instructions: "Do the thing".to_string(),
                acceptance_criteria: vec![],
                required_tools: vec![],
                constraints: vec![],
                dependencies: vec![],
                status: StepStatus::Pending,
                sub_spec: None,
                completed_at: None,
            }],
            created_by: "tester".to_string(),
            created_at: Utc::now(),
            metadata: json!({}),
        }
    }

    #[test]
    fn default_card_has_required_fields() {
        let card = default_agent_card("http://localhost:9000/rpc");
        assert_eq!(card.name, "nite-agent");
        assert!(card.supports_streaming() == false);
        assert_eq!(card.skills.len(), 1);
    }

    #[test]
    fn parse_spec_sheet_requires_metadata() {
        let err = parse_spec_sheet(None).unwrap_err();
        assert!(format!("{err}").contains("specSheet"));
    }

    #[test]
    fn parse_spec_step_requires_metadata_or_fallback() {
        let err = parse_spec_step(None, None).unwrap_err();
        assert!(format!("{err}").contains("specStep"));
    }

    #[test]
    fn parse_spec_step_reads_metadata() {
        let step = SpecStepRef {
            index: "2".to_string(),
            instructions: "Do more".to_string(),
            spec_id: "spec-1".to_string(),
        };
        let metadata = json!({ "specStep": step });
        let parsed = parse_spec_step(Some(&metadata), None).unwrap();
        assert_eq!(parsed.index, "2");
    }

    #[test]
    fn resolve_step_validates_index() {
        let spec = sample_spec();
        let reference = SpecStepRef {
            index: "1".to_string(),
            instructions: "Use context".to_string(),
            spec_id: spec.id.clone(),
        };
        let step = resolve_step(&spec, &reference).unwrap();
        assert_eq!(step.index, "1");
    }

    #[test]
    fn task_context_cleanup_drops_handles() {
        let spec = sample_spec();
        let step = spec.steps[0].clone();
        let mut context = TaskContext::new(spec, step);
        let flag = Arc::new(AtomicBool::new(false));
        let handle = flag.clone();
        context.register_cleanup(move || {
            handle.store(true, Ordering::SeqCst);
        });
        context.cleanup();
        assert!(flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stream_manager_cleans_up_subscribers() {
        let manager = TaskStreamManager::new();
        let (_id, _sender, rx) = manager.register("task-clean").await;
        assert_eq!(manager.subscriber_count("task-clean").await, 1);
        drop(rx);
        sleep(Duration::from_millis(25)).await;
        assert_eq!(manager.subscriber_count("task-clean").await, 0);
    }

    #[test]
    fn step_metadata_excludes_spec_sheet() {
        let step = sample_spec().steps[0].clone();
        let metadata = AgentA2AHandler::base_metadata_for_step(&step);
        assert!(metadata.spec_sheet.is_none());
        assert_eq!(metadata.extra.get("stepIndex").unwrap(), &json!("1"));
    }

    #[test]
    fn split_metadata_sets_spec_sheet_and_summary() {
        let child = sample_spec();
        let mut metadata = TaskMetadata::default();
        let summary = TaskSummary {
            task_id: "task-1".to_string(),
            step_index: "1".to_string(),
            summary_text: "Split summary".to_string(),
            artifacts_touched: vec![],
            tests_run: vec![],
            verification: TaskVerification {
                status: VerificationStatus::Pending,
                feedback: vec![],
            },
        };
        AgentA2AHandler::apply_split_metadata(&mut metadata, &child, &summary).unwrap();
        assert!(metadata.spec_sheet.is_some());
        assert!(metadata.summary.is_some());
    }

    #[test]
    fn split_state_marks_task_submitted() {
        let mut task = Task::new();
        AgentA2AHandler::mark_split_task(&mut task);
        assert_eq!(task.status.state, TaskState::Submitted);
        assert_eq!(task.status.message.as_deref(), Some("split"));
    }
}
