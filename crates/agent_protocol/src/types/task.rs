//! Task types for the A2A protocol
//!
//! Tasks represent units of work that agents perform. They have a defined
//! lifecycle with state transitions and can contain messages and artifacts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::artifact::Artifact;
use super::message::Message;

/// A Task represents a unit of work being performed by an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Unique identifier for this task
    pub id: String,

    /// Optional context ID for grouping related tasks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,

    /// Current status of the task
    pub status: TaskStatus,

    /// Messages exchanged during this task
    #[serde(default)]
    pub messages: Vec<Message>,

    /// Artifacts produced by this task
    #[serde(default)]
    pub artifacts: Vec<Artifact>,

    /// History of state transitions (if enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<TaskStatusUpdate>>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Current status of a task
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    /// Current state
    pub state: TaskState,

    /// Timestamp of this status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,

    /// Human-readable message about the status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Error details if state is failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskError>,
}

/// Task state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// Task has been created and acknowledged
    Submitted,

    /// Task is actively being processed
    Working,

    /// Task is waiting for user input
    InputRequired,

    /// Task completed successfully (terminal)
    Completed,

    /// Task encountered an error (terminal)
    Failed,

    /// Task was cancelled by user (terminal)
    Cancelled,

    /// Task was rejected by the agent (terminal)
    Rejected,

    /// Task requires additional authentication
    AuthRequired,
}

impl TaskState {
    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::Rejected
        )
    }

    /// Check if task is actively being processed
    pub fn is_active(&self) -> bool {
        matches!(self, TaskState::Submitted | TaskState::Working)
    }
}

/// Error information for failed tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    /// Error code
    pub code: i32,

    /// Error message
    pub message: String,

    /// Additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Record of a status update
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdate {
    /// The status at this point in time
    pub status: TaskStatus,

    /// When this update occurred
    pub timestamp: DateTime<Utc>,
}

/// Event for task status updates (used in streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    /// Task ID
    pub task_id: String,

    /// Context ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,

    /// The status update
    pub status: TaskStatus,

    /// Whether this is the final update
    #[serde(default)]
    pub final_update: bool,
}

/// Event for artifact updates (used in streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    /// Task ID
    pub task_id: String,

    /// Context ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,

    /// The artifact being updated
    pub artifact: Artifact,

    /// Index of this artifact
    pub index: usize,

    /// Whether this artifact is complete
    #[serde(default)]
    pub complete: bool,
}

impl Task {
    /// Create a new task with submitted state
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            context_id: None,
            status: TaskStatus {
                state: TaskState::Submitted,
                timestamp: Some(Utc::now()),
                message: None,
                error: None,
            },
            messages: vec![],
            artifacts: vec![],
            history: None,
            metadata: None,
        }
    }

    /// Create a new task with a specific context
    pub fn with_context(context_id: String) -> Self {
        let mut task = Self::new();
        task.context_id = Some(context_id);
        task
    }

    /// Create a new task with a specific ID
    pub fn with_id(id: String) -> Self {
        let mut task = Self::new();
        task.id = id;
        task
    }

    /// Update the task state
    pub fn set_state(&mut self, state: TaskState, message: Option<String>) {
        let update = TaskStatusUpdate {
            status: self.status.clone(),
            timestamp: Utc::now(),
        };

        self.status = TaskStatus {
            state,
            timestamp: Some(Utc::now()),
            message,
            error: None,
        };

        if let Some(ref mut history) = self.history {
            history.push(update);
        }
    }

    /// Mark task as working
    pub fn start(&mut self) {
        self.set_state(TaskState::Working, Some("Processing...".to_string()));
    }

    /// Mark task as completed
    pub fn complete(&mut self, message: Option<String>) {
        self.set_state(TaskState::Completed, message);
    }

    /// Mark task as failed
    pub fn fail(&mut self, error: TaskError) {
        self.status = TaskStatus {
            state: TaskState::Failed,
            timestamp: Some(Utc::now()),
            message: Some(error.message.clone()),
            error: Some(error),
        };
    }

    /// Mark task as cancelled
    pub fn cancel(&mut self) {
        self.set_state(TaskState::Cancelled, Some("Task cancelled by user".to_string()));
    }

    /// Mark task as requiring input
    pub fn request_input(&mut self, prompt: String) {
        self.set_state(TaskState::InputRequired, Some(prompt));
    }

    /// Add a message to the task
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Add an artifact to the task
    pub fn add_artifact(&mut self, artifact: Artifact) {
        self.artifacts.push(artifact);
    }

    /// Enable history tracking
    pub fn enable_history(&mut self) {
        if self.history.is_none() {
            self.history = Some(vec![]);
        }
    }

    /// Check if task is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.status.state.is_terminal()
    }

    /// Check if task is active
    pub fn is_active(&self) -> bool {
        self.status.state.is_active()
    }
}

impl Default for Task {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_lifecycle() {
        let mut task = Task::new();
        assert_eq!(task.status.state, TaskState::Submitted);

        task.start();
        assert_eq!(task.status.state, TaskState::Working);

        task.complete(Some("Done!".to_string()));
        assert_eq!(task.status.state, TaskState::Completed);
        assert!(task.is_terminal());
    }

    #[test]
    fn test_task_failure() {
        let mut task = Task::new();
        task.fail(TaskError {
            code: -1,
            message: "Something went wrong".to_string(),
            data: None,
        });

        assert_eq!(task.status.state, TaskState::Failed);
        assert!(task.status.error.is_some());
    }
}
