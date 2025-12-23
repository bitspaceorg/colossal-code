//! A2A Server implementation
//!
//! This module provides an HTTP server that implements the A2A protocol,
//! allowing other agents to discover and communicate with this agent.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::State,
    http::{header, Method},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use tokio::sync::{mpsc, RwLock};
use tower_http::cors::{Any, CorsLayer};

use crate::error::{A2AError, A2AResult};
use crate::jsonrpc::{
    CancelTaskParams, GetTaskParams, JsonRpcRequest, JsonRpcResponse, ListTasksParams,
    ListTasksResult, StreamEvent, A2AMethod,
};
use crate::types::agent_card::AgentCard;
use crate::types::message::{Message, SendMessageParams};
use crate::types::spec::SpecStepRef;
use crate::types::task::Task;
use crate::AGENT_CARD_PATH;

/// Handler trait for processing A2A requests
///
/// Implement this trait to handle incoming agent requests.
#[async_trait]
pub trait A2AHandler: Send + Sync + 'static {
    /// Handle an incoming message and return a task
    async fn handle_message(
        &self,
        message: Message,
        task_id: Option<String>,
        spec_step: Option<SpecStepRef>,
    ) -> A2AResult<Task>;

    /// Handle a streaming message request
    /// Returns a stream of events for SSE
    async fn handle_streaming_message(
        &self,
        message: Message,
        task_id: Option<String>,
        spec_step: Option<SpecStepRef>,
    ) -> A2AResult<mpsc::Receiver<StreamEvent>>;

    /// Get a task by ID
    async fn get_task(&self, task_id: &str) -> A2AResult<Task>;

    /// List tasks with optional filters
    async fn list_tasks(
        &self,
        context_id: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> A2AResult<(Vec<Task>, usize)>;

    /// Cancel a task
    async fn cancel_task(&self, task_id: &str) -> A2AResult<Task>;

    /// Subscribe to task updates
    async fn subscribe_to_task(&self, task_id: &str) -> A2AResult<mpsc::Receiver<StreamEvent>>;
}

/// A2A Server state
struct ServerState<H: A2AHandler> {
    agent_card: AgentCard,
    handler: Arc<H>,
}

/// A2A Protocol Server
pub struct A2AServer<H: A2AHandler> {
    agent_card: AgentCard,
    handler: Arc<H>,
    router: Option<Router>,
}

impl<H: A2AHandler> A2AServer<H> {
    /// Create a new A2A server
    pub fn new(agent_card: AgentCard, handler: H) -> Self {
        Self {
            agent_card,
            handler: Arc::new(handler),
            router: None,
        }
    }

    /// Build the router
    pub fn build(mut self) -> Self {
        let state = Arc::new(ServerState {
            agent_card: self.agent_card.clone(),
            handler: self.handler.clone(),
        });

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

        let router = Router::new()
            // Agent card discovery endpoint
            .route("/.well-known/agent.json", get(handle_agent_card::<H>))
            // JSON-RPC endpoint
            .route("/", post(handle_jsonrpc::<H>))
            // Alternative JSON-RPC endpoint
            .route("/rpc", post(handle_jsonrpc::<H>))
            .layer(cors)
            .with_state(state);

        self.router = Some(router);
        self
    }

    /// Get the router for integration with existing servers
    pub fn router(self) -> Router {
        match self.router {
            Some(r) => r,
            None => self.build().router.unwrap(),
        }
    }

    /// Start the server on the given address
    pub async fn serve(self, addr: SocketAddr) -> Result<(), std::io::Error> {
        let router = self.build().router.unwrap();

        tracing::info!("A2A server listening on {}", addr);
        tracing::info!(
            "Agent card available at http://{}{}",
            addr,
            AGENT_CARD_PATH
        );

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await
    }

    /// Get a reference to the agent card
    pub fn agent_card(&self) -> &AgentCard {
        &self.agent_card
    }
}

/// Handle agent card discovery
async fn handle_agent_card<H: A2AHandler>(
    State(state): State<Arc<ServerState<H>>>,
) -> impl IntoResponse {
    Json(state.agent_card.clone())
}

/// Handle JSON-RPC requests
async fn handle_jsonrpc<H: A2AHandler>(
    State(state): State<Arc<ServerState<H>>>,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    let method = match request.a2a_method() {
        Some(m) => m,
        None => {
            return Json(JsonRpcResponse::from_error(
                A2AError::MethodNotFound(request.method.clone()),
                request.id,
            ))
            .into_response();
        }
    };

    match method {
        A2AMethod::SendMessage => handle_send_message(&state, request).await,
        A2AMethod::SendStreamingMessage => handle_streaming_message(&state, request).await,
        A2AMethod::GetTask => handle_get_task(&state, request).await,
        A2AMethod::ListTasks => handle_list_tasks(&state, request).await,
        A2AMethod::CancelTask => handle_cancel_task(&state, request).await,
        A2AMethod::SubscribeToTask => handle_subscribe_to_task(&state, request).await,
        A2AMethod::GetExtendedAgentCard => {
            // For extended card, return the same card (could add auth check)
            Json(JsonRpcResponse::success(
                serde_json::to_value(&state.agent_card).unwrap(),
                request.id,
            ))
            .into_response()
        }
        _ => Json(JsonRpcResponse::from_error(
            A2AError::UnsupportedOperation(format!("Method {} not implemented", request.method)),
            request.id,
        ))
        .into_response(),
    }
}

/// Handle SendMessage requests
async fn handle_send_message<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: SendMessageParams = match request.parse_params() {
        Ok(p) => p,
        Err(e) => return Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    };

    match state
        .handler
        .handle_message(params.message, None, params.spec_step)
        .await
    {
        Ok(task) => Json(JsonRpcResponse::task(task, request.id)).into_response(),
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Handle SendStreamingMessage requests
async fn handle_streaming_message<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: SendMessageParams = match request.parse_params() {
        Ok(p) => p,
        Err(e) => return Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    };

    match state
        .handler
        .handle_streaming_message(params.message, None, params.spec_step)
        .await
    {
        Ok(rx) => create_sse_response(rx).into_response(),
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Handle GetTask requests
async fn handle_get_task<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: GetTaskParams = match request.parse_params() {
        Ok(p) => p,
        Err(e) => return Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    };

    match state.handler.get_task(&params.task_id).await {
        Ok(task) => Json(JsonRpcResponse::task(task, request.id)).into_response(),
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Handle ListTasks requests
async fn handle_list_tasks<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: ListTasksParams = request.parse_params().unwrap_or(ListTasksParams {
        context_id: None,
        limit: None,
        offset: None,
    });

    match state
        .handler
        .list_tasks(params.context_id.as_deref(), params.limit, params.offset)
        .await
    {
        Ok((tasks, total)) => {
            let result = ListTasksResult { tasks, total };
            Json(JsonRpcResponse::success(
                serde_json::to_value(result).unwrap(),
                request.id,
            ))
            .into_response()
        }
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Handle CancelTask requests
async fn handle_cancel_task<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: CancelTaskParams = match request.parse_params() {
        Ok(p) => p,
        Err(e) => return Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    };

    match state.handler.cancel_task(&params.task_id).await {
        Ok(task) => Json(JsonRpcResponse::task(task, request.id)).into_response(),
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Handle SubscribeToTask requests (streaming)
async fn handle_subscribe_to_task<H: A2AHandler>(
    state: &Arc<ServerState<H>>,
    request: JsonRpcRequest,
) -> Response {
    let params: GetTaskParams = match request.parse_params() {
        Ok(p) => p,
        Err(e) => return Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    };

    match state.handler.subscribe_to_task(&params.task_id).await {
        Ok(rx) => create_sse_response(rx).into_response(),
        Err(e) => Json(JsonRpcResponse::from_error(e, request.id)).into_response(),
    }
}

/// Create an SSE response from a stream of events
fn create_sse_response(mut rx: mpsc::Receiver<StreamEvent>) -> impl IntoResponse {
    let stream = async_stream::stream! {
        while let Some(event) = rx.recv().await {
            match serde_json::to_string(&event) {
                Ok(data) => {
                    yield Ok::<_, std::convert::Infallible>(Event::default().data(data));
                }
                Err(e) => {
                    tracing::error!("Failed to serialize stream event: {}", e);
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
}

/// Simple in-memory task store for testing/examples
pub struct InMemoryTaskStore {
    tasks: RwLock<HashMap<String, Task>>,
}

impl InMemoryTaskStore {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    pub async fn insert(&self, task: Task) {
        let mut tasks = self.tasks.write().await;
        tasks.insert(task.id.clone(), task);
    }

    pub async fn get(&self, id: &str) -> Option<Task> {
        let tasks = self.tasks.read().await;
        tasks.get(id).cloned()
    }

    pub async fn update(&self, id: &str, f: impl FnOnce(&mut Task)) -> Option<Task> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(id) {
            f(task);
            Some(task.clone())
        } else {
            None
        }
    }

    pub async fn list(
        &self,
        context_id: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> (Vec<Task>, usize) {
        let tasks = self.tasks.read().await;
        let filtered: Vec<_> = tasks
            .values()
            .filter(|t| {
                if let Some(ctx) = context_id {
                    t.context_id.as_deref() == Some(ctx)
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        let total = filtered.len();
        let offset = offset.unwrap_or(0);
        let limit = limit.unwrap_or(100);

        let paginated = filtered
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();

        (paginated, total)
    }
}

impl Default for InMemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHandler;

    #[async_trait]
    impl A2AHandler for TestHandler {
        async fn handle_message(
            &self,
            message: Message,
            _task_id: Option<String>,
            _spec_step: Option<SpecStepRef>,
        ) -> A2AResult<Task> {
            let mut task = Task::new();
            task.add_message(message);
            task.add_message(Message::agent("Hello from test handler!"));
            task.complete(Some("Done".to_string()));
            Ok(task)
        }

        async fn handle_streaming_message(
            &self,
            message: Message,
            task_id: Option<String>,
            spec_step: Option<SpecStepRef>,
        ) -> A2AResult<mpsc::Receiver<StreamEvent>> {
            let (tx, rx) = mpsc::channel(10);
            let task = self.handle_message(message, task_id, spec_step).await?;
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::task(task)).await;
            });
            Ok(rx)
        }

        async fn get_task(&self, task_id: &str) -> A2AResult<Task> {
            Err(A2AError::TaskNotFound(task_id.to_string()))
        }

        async fn list_tasks(
            &self,
            _context_id: Option<&str>,
            _limit: Option<usize>,
            _offset: Option<usize>,
        ) -> A2AResult<(Vec<Task>, usize)> {
            Ok((vec![], 0))
        }

        async fn cancel_task(&self, task_id: &str) -> A2AResult<Task> {
            Err(A2AError::TaskNotFound(task_id.to_string()))
        }

        async fn subscribe_to_task(&self, task_id: &str) -> A2AResult<mpsc::Receiver<StreamEvent>> {
            Err(A2AError::TaskNotFound(task_id.to_string()))
        }
    }

    #[test]
    fn test_server_creation() {
        let card = AgentCard::builder()
            .name("test")
            .base_url("http://localhost:8080")
            .build()
            .unwrap();

        let server = A2AServer::new(card.clone(), TestHandler);
        assert_eq!(server.agent_card().name, "test");
    }
}
