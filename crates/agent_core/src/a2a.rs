use std::{collections::HashMap, sync::Arc};

use agent_protocol::{
    error::A2AResult,
    jsonrpc::StreamEvent,
    server::{A2AHandler, A2AServer},
    types::{
        agent_card::AgentCard,
        message::Message,
        task::{Task, TaskError, TaskState},
    },
    A2AError,
};
use async_trait::async_trait;
use anyhow::anyhow;
use chrono::Utc;
use serde_json::json;
use tokio::{
    runtime::Builder,
    sync::{mpsc, RwLock},
};

use crate::{Agent, AgentMessage};

/// Handler that bridges the interactive [`Agent`] with the A2A protocol server.
pub struct AgentA2AHandler {
    agent: Agent,
    tasks: Arc<RwLock<HashMap<String, Task>>>,
}

impl AgentA2AHandler {
    /// Create a new handler for the given agent
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store the current task snapshot for later queries
    async fn persist_task(&self, task: Task) {
        self.tasks.write().await.insert(task.id.clone(), task);
    }

    /// Run the core agent for the given message and return a completed task record
    async fn execute_task(
        &self,
        mut task: Task,
        input_message: Message,
    ) -> A2AResult<Task> {
        let prompt = input_message.text_content();
        if prompt.trim().is_empty() {
            return Err(A2AError::InvalidParams(
                "message must include at least one text part".to_string(),
            ));
        }

        let mut stored_input = input_message.clone();
        stored_input.task_id = Some(task.id.clone());
        task.messages.push(stored_input);
        task.status.state = TaskState::Working;
        task.status.timestamp = Some(Utc::now());
        task.status.message = Some("Processing".to_string());
        task.status.error = None;
        self.persist_task(task.clone()).await;

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

        while let Some(agent_msg) = rx.recv().await {
            match agent_msg {
                AgentMessage::AgentResponse(content, _) => {
                    latest_response = content;
                }
                AgentMessage::Error(err) => {
                    error_message = Some(err);
                    break;
                }
                AgentMessage::GenerationStats(stats) => {
                    stats_payload = Some(stats);
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

        if let Some(err) = error_message {
            task.status.state = TaskState::Failed;
            task.status.timestamp = Some(Utc::now());
            task.status.message = Some("Generation failed".to_string());
            task.status.error = Some(TaskError {
                code: -1,
                message: err.clone(),
                data: None,
            });
            self.persist_task(task.clone()).await;
            return Err(A2AError::InternalError(err));
        }

        if !latest_response.trim().is_empty() {
            let mut response = Message::agent(latest_response.clone());
            response.task_id = Some(task.id.clone());
            response.context_id = task.context_id.clone();
            task.messages.push(response);
        }

        task.status.state = TaskState::Completed;
        task.status.timestamp = Some(Utc::now());
        task.status.message = Some("Response ready".to_string());
        if let Some(stats) = stats_payload {
            task.metadata = Some(json!({
                "generationStats": {
                    "avgCompletionTokPerSec": stats.avg_completion_tok_per_sec,
                    "completionTokens": stats.completion_tokens,
                    "promptTokens": stats.prompt_tokens,
                    "timeToFirstTokenSec": stats.time_to_first_token_sec,
                    "stopReason": stats.stop_reason,
                }
            }));
        }

        self.persist_task(task.clone()).await;
        Ok(task)
    }
}

#[async_trait]
impl A2AHandler for AgentA2AHandler {
    async fn handle_message(
        &self,
        mut message: Message,
        task_id: Option<String>,
    ) -> A2AResult<Task> {
        let mut task = Task::new();
        task.context_id = message.context_id.clone();
        if let Some(id) = task_id.or_else(|| message.task_id.clone()) {
            task.id = id;
        }
        message.task_id = Some(task.id.clone());
        self.execute_task(task, message).await
    }

    async fn handle_streaming_message(
        &self,
        _message: Message,
        _task_id: Option<String>,
    ) -> A2AResult<mpsc::Receiver<StreamEvent>> {
        Err(A2AError::UnsupportedOperation(
            "streaming is not enabled for this agent".to_string(),
        ))
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
        _task_id: &str,
    ) -> A2AResult<mpsc::Receiver<StreamEvent>> {
        Err(A2AError::UnsupportedOperation(
            "task subscriptions are not implemented".to_string(),
        ))
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

    #[test]
    fn default_card_has_required_fields() {
        let card = default_agent_card("http://localhost:9000/rpc");
        assert_eq!(card.name, "nite-agent");
        assert!(card.supports_streaming() == false);
        assert_eq!(card.skills.len(), 1);
    }
}
