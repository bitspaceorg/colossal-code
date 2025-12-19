//! A2A Client implementation
//!
//! This module provides an HTTP client for communicating with other A2A agents,
//! supporting both synchronous and streaming interactions.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use futures::stream::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest_eventsource::{Event as SseEvent, EventSource};

use crate::error::{A2AError, A2AResult};
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse, RequestId, StreamEvent};
use crate::types::agent_card::AgentCard;
use crate::types::message::Message;
use crate::types::task::Task;
use crate::AGENT_CARD_PATH;

/// A2A Client for communicating with other agents
pub struct A2AClient {
    /// The remote agent's card
    agent_card: AgentCard,

    /// HTTP client
    http_client: reqwest::Client,

    /// JSON-RPC endpoint URL
    rpc_url: String,

    /// Authentication header value
    auth_header: Option<String>,

    /// Request ID counter
    request_id: AtomicI64,
}

/// Builder for creating A2A clients
pub struct A2AClientBuilder {
    agent_card: Option<AgentCard>,
    card_url: Option<String>,
    auth_header: Option<String>,
    timeout: Duration,
}

impl A2AClientBuilder {
    pub fn new() -> Self {
        Self {
            agent_card: None,
            card_url: None,
            auth_header: None,
            timeout: Duration::from_secs(30),
        }
    }

    /// Set the agent card directly
    pub fn agent_card(mut self, card: AgentCard) -> Self {
        self.agent_card = Some(card);
        self
    }

    /// Set the URL to fetch the agent card from
    pub fn card_url(mut self, url: impl Into<String>) -> Self {
        self.card_url = Some(url.into());
        self
    }

    /// Set bearer token authentication
    pub fn bearer_auth(mut self, token: impl Into<String>) -> Self {
        self.auth_header = Some(format!("Bearer {}", token.into()));
        self
    }

    /// Set API key authentication
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.auth_header = Some(key.into());
        self
    }

    /// Set custom authorization header
    pub fn auth_header(mut self, header: impl Into<String>) -> Self {
        self.auth_header = Some(header.into());
        self
    }

    /// Set request timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the client
    pub async fn build(self) -> A2AResult<A2AClient> {
        let http_client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| A2AError::ConnectionError(e.to_string()))?;

        let agent_card = if let Some(card) = self.agent_card {
            card
        } else if let Some(url) = self.card_url {
            // Fetch the agent card
            let card_url = if url.ends_with("/agent.json") || url.contains("/.well-known/") {
                url
            } else {
                format!("{}{}", url.trim_end_matches('/'), AGENT_CARD_PATH)
            };

            let resp = http_client
                .get(&card_url)
                .send()
                .await
                .map_err(|e| A2AError::ConnectionError(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(A2AError::InvalidAgentCard(format!(
                    "Failed to fetch agent card: HTTP {}",
                    resp.status()
                )));
            }

            resp.json::<AgentCard>()
                .await
                .map_err(|e| A2AError::InvalidAgentCard(e.to_string()))?
        } else {
            return Err(A2AError::InvalidAgentCard(
                "Either agent_card or card_url must be provided".to_string(),
            ));
        };

        let rpc_url = agent_card
            .jsonrpc_url()
            .ok_or_else(|| A2AError::InvalidAgentCard("No JSON-RPC endpoint in agent card".to_string()))?
            .to_string();

        Ok(A2AClient {
            agent_card,
            http_client,
            rpc_url,
            auth_header: self.auth_header,
            request_id: AtomicI64::new(1),
        })
    }
}

impl Default for A2AClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl A2AClient {
    /// Create a new client builder
    pub fn builder() -> A2AClientBuilder {
        A2AClientBuilder::new()
    }

    /// Create a client from an agent card URL
    pub async fn from_url(url: impl Into<String>) -> A2AResult<Self> {
        A2AClientBuilder::new().card_url(url).build().await
    }

    /// Create a client from an existing agent card
    pub fn from_card(card: AgentCard) -> A2AResult<Self> {
        let rpc_url = card
            .jsonrpc_url()
            .ok_or_else(|| A2AError::InvalidAgentCard("No JSON-RPC endpoint in agent card".to_string()))?
            .to_string();

        Ok(Self {
            agent_card: card,
            http_client: reqwest::Client::new(),
            rpc_url,
            auth_header: None,
            request_id: AtomicI64::new(1),
        })
    }

    /// Get the remote agent's card
    pub fn agent_card(&self) -> &AgentCard {
        &self.agent_card
    }

    /// Get the next request ID
    fn next_request_id(&self) -> RequestId {
        RequestId::Number(self.request_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Build headers for a request
    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        if let Some(ref auth) = self.auth_header {
            if let Ok(value) = HeaderValue::from_str(auth) {
                headers.insert(AUTHORIZATION, value);
            }
        }

        headers
    }

    /// Send a JSON-RPC request and get the response
    async fn send_request(&self, request: JsonRpcRequest) -> A2AResult<JsonRpcResponse> {
        let resp = self
            .http_client
            .post(&self.rpc_url)
            .headers(self.build_headers())
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(A2AError::HttpError(format!(
                "HTTP error: {}",
                resp.status()
            )));
        }

        let response: JsonRpcResponse = resp.json().await?;
        Ok(response)
    }

    /// Send a message to the agent (synchronous)
    pub async fn send_message(&self, message: Message) -> A2AResult<Task> {
        let request = JsonRpcRequest::send_message(message, self.next_request_id());
        let response = self.send_request(request).await?;
        response.parse_result()
    }

    /// Send a text message to the agent
    pub async fn send_text(&self, text: impl Into<String>) -> A2AResult<Task> {
        self.send_message(Message::user(text)).await
    }

    /// Send a message with streaming response
    pub async fn send_message_streaming(
        &self,
        message: Message,
    ) -> A2AResult<impl Stream<Item = A2AResult<StreamEvent>>> {
        let request = JsonRpcRequest::send_streaming_message(message, self.next_request_id());

        let mut headers = self.build_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        let request_builder = self
            .http_client
            .post(&self.rpc_url)
            .headers(headers)
            .json(&request);

        let event_source = EventSource::new(request_builder)
            .map_err(|e| A2AError::ConnectionError(e.to_string()))?;

        Ok(event_source.filter_map(|event| async {
            match event {
                Ok(SseEvent::Open) => None,
                Ok(SseEvent::Message(msg)) => {
                    match serde_json::from_str::<StreamEvent>(&msg.data) {
                        Ok(event) => Some(Ok(event)),
                        Err(e) => Some(Err(A2AError::ParseError(e.to_string()))),
                    }
                }
                Err(e) => Some(Err(A2AError::StreamError(e.to_string()))),
            }
        }))
    }

    /// Send a text message with streaming response
    pub async fn send_text_streaming(
        &self,
        text: impl Into<String>,
    ) -> A2AResult<impl Stream<Item = A2AResult<StreamEvent>>> {
        self.send_message_streaming(Message::user(text)).await
    }

    /// Get a task by ID
    pub async fn get_task(&self, task_id: impl Into<String>) -> A2AResult<Task> {
        let request = JsonRpcRequest::get_task(task_id, self.next_request_id());
        let response = self.send_request(request).await?;
        response.parse_result()
    }

    /// List tasks
    pub async fn list_tasks(
        &self,
        context_id: Option<String>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> A2AResult<(Vec<Task>, usize)> {
        let request = JsonRpcRequest::list_tasks(context_id, limit, offset, self.next_request_id());
        let response = self.send_request(request).await?;

        #[derive(serde::Deserialize)]
        struct ListResult {
            tasks: Vec<Task>,
            total: usize,
        }

        let result: ListResult = response.parse_result()?;
        Ok((result.tasks, result.total))
    }

    /// Cancel a task
    pub async fn cancel_task(&self, task_id: impl Into<String>) -> A2AResult<Task> {
        let request = JsonRpcRequest::cancel_task(task_id, self.next_request_id());
        let response = self.send_request(request).await?;
        response.parse_result()
    }

    /// Subscribe to task updates
    pub async fn subscribe_to_task(
        &self,
        task_id: impl Into<String>,
    ) -> A2AResult<impl Stream<Item = A2AResult<StreamEvent>>> {
        let request = JsonRpcRequest::subscribe_to_task(task_id, self.next_request_id());

        let mut headers = self.build_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        let request_builder = self
            .http_client
            .post(&self.rpc_url)
            .headers(headers)
            .json(&request);

        let event_source = EventSource::new(request_builder)
            .map_err(|e| A2AError::ConnectionError(e.to_string()))?;

        Ok(event_source.filter_map(|event| async {
            match event {
                Ok(SseEvent::Open) => None,
                Ok(SseEvent::Message(msg)) => {
                    match serde_json::from_str::<StreamEvent>(&msg.data) {
                        Ok(event) => Some(Ok(event)),
                        Err(e) => Some(Err(A2AError::ParseError(e.to_string()))),
                    }
                }
                Err(e) => Some(Err(A2AError::StreamError(e.to_string()))),
            }
        }))
    }

    /// Check if the remote agent supports streaming
    pub fn supports_streaming(&self) -> bool {
        self.agent_card.supports_streaming()
    }

    /// Check if the remote agent supports push notifications
    pub fn supports_push_notifications(&self) -> bool {
        self.agent_card.supports_push_notifications()
    }

    /// Find a skill by ID on the remote agent
    pub fn find_skill(&self, id: &str) -> Option<&crate::types::agent_card::Skill> {
        self.agent_card.find_skill(id)
    }
}

/// Convenience function to discover an agent and create a client
pub async fn discover_agent(base_url: impl Into<String>) -> A2AResult<A2AClient> {
    A2AClient::from_url(base_url).await
}

/// Multi-agent client for managing connections to multiple agents
pub struct MultiAgentClient {
    agents: std::collections::HashMap<String, A2AClient>,
}

impl MultiAgentClient {
    pub fn new() -> Self {
        Self {
            agents: std::collections::HashMap::new(),
        }
    }

    /// Add an agent by URL
    pub async fn add_agent(&mut self, name: impl Into<String>, url: impl Into<String>) -> A2AResult<()> {
        let client = A2AClient::from_url(url).await?;
        self.agents.insert(name.into(), client);
        Ok(())
    }

    /// Add an agent with an existing client
    pub fn add_client(&mut self, name: impl Into<String>, client: A2AClient) {
        self.agents.insert(name.into(), client);
    }

    /// Get an agent by name
    pub fn get(&self, name: &str) -> Option<&A2AClient> {
        self.agents.get(name)
    }

    /// Get a mutable reference to an agent
    pub fn get_mut(&mut self, name: &str) -> Option<&mut A2AClient> {
        self.agents.get_mut(name)
    }

    /// Remove an agent
    pub fn remove(&mut self, name: &str) -> Option<A2AClient> {
        self.agents.remove(name)
    }

    /// List all connected agent names
    pub fn list_agents(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Send a message to a specific agent
    pub async fn send_to(&self, agent_name: &str, message: Message) -> A2AResult<Task> {
        let client = self
            .agents
            .get(agent_name)
            .ok_or_else(|| A2AError::ConnectionError(format!("Agent '{}' not found", agent_name)))?;

        client.send_message(message).await
    }

    /// Broadcast a message to all agents
    pub async fn broadcast(&self, message: Message) -> Vec<(String, A2AResult<Task>)> {
        let mut results = Vec::new();

        for (name, client) in &self.agents {
            let result = client.send_message(message.clone()).await;
            results.push((name.clone(), result));
        }

        results
    }
}

impl Default for MultiAgentClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_builder() {
        let card = AgentCard::builder()
            .name("test")
            .base_url("http://localhost:8080")
            .build()
            .unwrap();

        let client = A2AClient::from_card(card).unwrap();
        assert_eq!(client.agent_card().name, "test");
    }

    #[test]
    fn test_multi_agent_client() {
        let mut multi = MultiAgentClient::new();

        let card = AgentCard::builder()
            .name("test-agent")
            .base_url("http://localhost:8080")
            .build()
            .unwrap();

        let client = A2AClient::from_card(card).unwrap();
        multi.add_client("test", client);

        assert!(multi.get("test").is_some());
        assert_eq!(multi.list_agents(), vec!["test"]);
    }
}
