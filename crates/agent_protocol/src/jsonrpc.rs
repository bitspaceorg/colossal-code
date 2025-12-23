//! JSON-RPC 2.0 protocol binding for A2A
//!
//! This module implements the JSON-RPC 2.0 protocol as specified by A2A,
//! providing request/response types and method definitions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::A2AError;
use crate::types::message::{Message, SendMessageParams};
use crate::types::task::{Task, TaskArtifactUpdateEvent, TaskStatusUpdateEvent};

/// JSON-RPC 2.0 version string
pub const JSONRPC_VERSION: &str = "2.0";

/// A2A protocol methods
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2AMethod {
    /// Send a message to start or continue a task
    SendMessage,
    /// Send a message with streaming response
    SendStreamingMessage,
    /// Get current state of a task
    GetTask,
    /// List tasks with optional filters
    ListTasks,
    /// Cancel a running task
    CancelTask,
    /// Subscribe to task updates (streaming)
    SubscribeToTask,
    /// Get extended agent card (authenticated)
    GetExtendedAgentCard,
    /// Set push notification configuration
    SetPushNotificationConfig,
    /// Get push notification configuration
    GetPushNotificationConfig,
}

impl A2AMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            A2AMethod::SendMessage => "message/send",
            A2AMethod::SendStreamingMessage => "message/stream",
            A2AMethod::GetTask => "tasks/get",
            A2AMethod::ListTasks => "tasks/list",
            A2AMethod::CancelTask => "tasks/cancel",
            A2AMethod::SubscribeToTask => "tasks/subscribe",
            A2AMethod::GetExtendedAgentCard => "agent/card",
            A2AMethod::SetPushNotificationConfig => "tasks/pushNotificationConfig/set",
            A2AMethod::GetPushNotificationConfig => "tasks/pushNotificationConfig/get",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "message/send" => Some(A2AMethod::SendMessage),
            "message/stream" => Some(A2AMethod::SendStreamingMessage),
            "tasks/get" => Some(A2AMethod::GetTask),
            "tasks/list" => Some(A2AMethod::ListTasks),
            "tasks/cancel" => Some(A2AMethod::CancelTask),
            "tasks/subscribe" => Some(A2AMethod::SubscribeToTask),
            "agent/card" => Some(A2AMethod::GetExtendedAgentCard),
            "tasks/pushNotificationConfig/set" => Some(A2AMethod::SetPushNotificationConfig),
            "tasks/pushNotificationConfig/get" => Some(A2AMethod::GetPushNotificationConfig),
            _ => None,
        }
    }
}

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be "2.0"
    pub jsonrpc: String,

    /// Method name
    pub method: String,

    /// Request parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,

    /// Request ID (string or number)
    pub id: RequestId,
}

/// Request ID can be a string or number
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        RequestId::String(s)
    }
}

impl From<&str> for RequestId {
    fn from(s: &str) -> Self {
        RequestId::String(s.to_string())
    }
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        RequestId::Number(n)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::String(s) => write!(f, "{}", s),
            RequestId::Number(n) => write!(f, "{}", n),
        }
    }
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Must be "2.0"
    pub jsonrpc: String,

    /// Result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,

    /// Error (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,

    /// Request ID this is responding to
    pub id: RequestId,
}

/// JSON-RPC 2.0 Error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,

    /// Error message
    pub message: String,

    /// Additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request
    pub fn new(method: A2AMethod, params: Option<Value>, id: impl Into<RequestId>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.as_str().to_string(),
            params,
            id: id.into(),
        }
    }

    /// Create a SendMessage request
    pub fn send_message(message: Message, id: impl Into<RequestId>) -> Self {
        let params = SendMessageParams {
            message,
            spec_step: None,
            configuration: None,
        };
        Self::new(
            A2AMethod::SendMessage,
            Some(serde_json::to_value(params).unwrap()),
            id,
        )
    }

    /// Create a SendStreamingMessage request
    pub fn send_streaming_message(message: Message, id: impl Into<RequestId>) -> Self {
        let params = SendMessageParams {
            message,
            spec_step: None,
            configuration: None,
        };
        Self::new(
            A2AMethod::SendStreamingMessage,
            Some(serde_json::to_value(params).unwrap()),
            id,
        )
    }

    /// Create a GetTask request
    pub fn get_task(task_id: impl Into<String>, id: impl Into<RequestId>) -> Self {
        Self::new(
            A2AMethod::GetTask,
            Some(serde_json::json!({ "taskId": task_id.into() })),
            id,
        )
    }

    /// Create a CancelTask request
    pub fn cancel_task(task_id: impl Into<String>, id: impl Into<RequestId>) -> Self {
        Self::new(
            A2AMethod::CancelTask,
            Some(serde_json::json!({ "taskId": task_id.into() })),
            id,
        )
    }

    /// Create a ListTasks request
    pub fn list_tasks(
        context_id: Option<String>,
        limit: Option<usize>,
        offset: Option<usize>,
        id: impl Into<RequestId>,
    ) -> Self {
        let mut params = serde_json::Map::new();
        if let Some(ctx) = context_id {
            params.insert("contextId".to_string(), Value::String(ctx));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), Value::Number(l.into()));
        }
        if let Some(o) = offset {
            params.insert("offset".to_string(), Value::Number(o.into()));
        }
        Self::new(A2AMethod::ListTasks, Some(Value::Object(params)), id)
    }

    /// Create a SubscribeToTask request
    pub fn subscribe_to_task(task_id: impl Into<String>, id: impl Into<RequestId>) -> Self {
        Self::new(
            A2AMethod::SubscribeToTask,
            Some(serde_json::json!({ "taskId": task_id.into() })),
            id,
        )
    }

    /// Get the A2A method enum
    pub fn a2a_method(&self) -> Option<A2AMethod> {
        A2AMethod::from_str(&self.method)
    }

    /// Parse params as a specific type
    pub fn parse_params<T: for<'de> Deserialize<'de>>(&self) -> Result<T, A2AError> {
        match &self.params {
            Some(params) => {
                serde_json::from_value(params.clone()).map_err(|e| A2AError::InvalidParams(e.to_string()))
            }
            None => Err(A2AError::InvalidParams("Missing params".to_string())),
        }
    }
}

impl JsonRpcResponse {
    /// Create a success response
    pub fn success(result: Value, id: RequestId) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Create an error response
    pub fn error(error: JsonRpcError, id: RequestId) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: None,
            error: Some(error),
            id,
        }
    }

    /// Create a response from an A2AError
    pub fn from_error(err: A2AError, id: RequestId) -> Self {
        Self::error(
            JsonRpcError {
                code: err.code(),
                message: err.to_string(),
                data: None,
            },
            id,
        )
    }

    /// Create a task response
    pub fn task(task: Task, id: RequestId) -> Self {
        Self::success(serde_json::to_value(task).unwrap(), id)
    }

    /// Check if this is an error response
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Parse result as a specific type
    pub fn parse_result<T: for<'de> Deserialize<'de>>(&self) -> Result<T, A2AError> {
        match &self.result {
            Some(result) => {
                serde_json::from_value(result.clone()).map_err(|e| A2AError::ParseError(e.to_string()))
            }
            None => {
                if let Some(err) = &self.error {
                    Err(A2AError::InternalError(err.message.clone()))
                } else {
                    Err(A2AError::ParseError("No result in response".to_string()))
                }
            }
        }
    }
}

impl From<A2AError> for JsonRpcError {
    fn from(err: A2AError) -> Self {
        JsonRpcError {
            code: err.code(),
            message: err.to_string(),
            data: None,
        }
    }
}

/// Server-Sent Event for streaming responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamEvent {
    /// Full task state
    Task { task: Task },
    /// A message in the conversation
    Message { message: Message },
    /// Status update event
    StatusUpdate {
        #[serde(rename = "statusUpdate")]
        status_update: TaskStatusUpdateEvent,
    },
    /// Artifact update event
    ArtifactUpdate {
        #[serde(rename = "artifactUpdate")]
        artifact_update: TaskArtifactUpdateEvent,
    },
}

impl StreamEvent {
    /// Create a task event
    pub fn task(task: Task) -> Self {
        StreamEvent::Task { task }
    }

    /// Create a message event
    pub fn message(message: Message) -> Self {
        StreamEvent::Message { message }
    }

    /// Create a status update event
    pub fn status_update(event: TaskStatusUpdateEvent) -> Self {
        StreamEvent::StatusUpdate {
            status_update: event,
        }
    }

    /// Create an artifact update event
    pub fn artifact_update(event: TaskArtifactUpdateEvent) -> Self {
        StreamEvent::ArtifactUpdate {
            artifact_update: event,
        }
    }

    /// Serialize to SSE data line
    pub fn to_sse_data(&self) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(format!("data: {}\n\n", json))
    }
}

/// Parameters for GetTask
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTaskParams {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_length: Option<usize>,
}

/// Parameters for ListTasks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// Response for ListTasks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResult {
    pub tasks: Vec<Task>,
    pub total: usize,
}

/// Parameters for CancelTask
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTaskParams {
    pub task_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::Message;

    #[test]
    fn test_request_creation() {
        let msg = Message::user("Hello!");
        let req = JsonRpcRequest::send_message(msg, "req-1");

        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "message/send");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_response_success() {
        let task = Task::new();
        let resp = JsonRpcResponse::task(task.clone(), "req-1".into());

        assert!(!resp.is_error());
        let parsed: Task = resp.parse_result().unwrap();
        assert_eq!(parsed.id, task.id);
    }

    #[test]
    fn test_response_error() {
        let resp = JsonRpcResponse::from_error(
            A2AError::TaskNotFound("task-123".to_string()),
            "req-1".into(),
        );

        assert!(resp.is_error());
        assert_eq!(resp.error.as_ref().unwrap().code, -32001);
    }

    #[test]
    fn test_stream_event_serialization() {
        let task = Task::new();
        let event = StreamEvent::task(task);
        let sse = event.to_sse_data().unwrap();

        assert!(sse.starts_with("data: "));
        assert!(sse.ends_with("\n\n"));
    }
}
