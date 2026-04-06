use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::{self, Stream as FuturesStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use either::Either;
use indexmap::IndexMap;
use mistralrs::{
    ChatCompletionChunkResponse, ChatCompletionResponse, Choice, ChunkChoice, Delta,
    GgufModelBuilder, Model, RequestBuilder, RequestLike, Response, ResponseMessage, Usage,
};
use mistralrs_core::{CalledFunction, MessageContent, ToolCallResponse, ToolCallType, ToolChoice};
use once_cell::sync::OnceCell;
use reqwest::{Client, Url, header::CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::IpAddr, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

mod anthropic;
mod claude_auth;
mod openai_auth;
mod openai_options;
mod responses;

use claude_auth::ClaudeCodeAuthState;
use openai_auth::OpenAiAuthState;

#[async_trait]
pub trait LLMBackend: Send + Sync {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>>;

    async fn load_model(&self) -> Result<()>;

    async fn reload_model(&self, model_filename: String) -> Result<()>;

    async fn get_model(&self) -> Result<Arc<Model>>;

    /// Return the most recent usage stats observed from the streaming backend.
    /// Called on cancellation to recover prompt/completion token counts that
    /// the API already sent before the stream was dropped.
    async fn get_latest_usage(&self) -> Option<Usage> {
        None
    }
}

/// Stub backend used when no model or provider is configured.
/// Allows the TUI to start up and render the `/connect` modal.
pub struct NoneBackend;

#[async_trait]
impl LLMBackend for NoneBackend {
    async fn stream_chat_request(
        &self,
        _request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        anyhow::bail!(
            "No model configured. Use /connect to add a provider or /model to select a local model."
        )
    }

    async fn load_model(&self) -> Result<()> {
        Ok(())
    }

    async fn reload_model(&self, _model_filename: String) -> Result<()> {
        Ok(())
    }

    async fn get_model(&self) -> Result<Arc<Model>> {
        anyhow::bail!("No model configured")
    }
}

pub struct LocalBackend {
    model: Arc<Mutex<Option<Arc<Model>>>>,
    model_path: String,
    model_files: Arc<Mutex<Vec<String>>>,
}

impl LocalBackend {
    pub fn new(model_path: String, model_files: Vec<String>) -> Self {
        Self {
            model: Arc::new(Mutex::new(None)),
            model_path,
            model_files: Arc::new(Mutex::new(model_files)),
        }
    }
}

#[async_trait]
impl LLMBackend for LocalBackend {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        let model_guard = self.model.lock().await;
        let model = model_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model not loaded"))?
            .clone();
        drop(model_guard);

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            match model.stream_chat_request(request).await {
                Ok(mut stream) => {
                    while let Some(response) = stream.next().await {
                        if tx.send(response).is_err() {
                            break;
                        }
                    }
                }
                Err(err) => {
                    let boxed: Box<dyn std::error::Error + Send + Sync> = err.into();
                    let _ = tx.send(Response::InternalError(boxed));
                }
            }
        });

        Ok(Box::new(UnboundedReceiverStream::new(rx)))
    }

    async fn get_model(&self) -> Result<Arc<Model>> {
        let model_guard = self.model.lock().await;
        if let Some(m) = model_guard.as_ref() {
            return Ok(m.clone());
        }
        drop(model_guard);
        self.load_model().await?;
        let model_guard = self.model.lock().await;
        Ok(model_guard.as_ref().unwrap().clone())
    }

    async fn load_model(&self) -> Result<()> {
        let mut model_guard = self.model.lock().await;
        if model_guard.is_none() {
            // Redirect stdout/stderr to /dev/null during model loading to suppress progress bars
            // (Platform specific loading logic omitted for brevity in this first pass, but included in full implementation)
            let model_files = self.model_files.lock().await.clone();
            let model = GgufModelBuilder::new(self.model_path.clone(), model_files)
                .build()
                .await?;

            *model_guard = Some(Arc::new(model));
        }
        Ok(())
    }

    async fn reload_model(&self, new_model_filename: String) -> Result<()> {
        let mut model_guard = self.model.lock().await;
        *model_guard = None;
        drop(model_guard);

        let mut model_files_guard = self.model_files.lock().await;
        *model_files_guard = vec![new_model_filename];
        drop(model_files_guard);
        Ok(())
    }
}

pub struct HttpBackend {
    client: Client,
    base_url: String,
    api_key: String,
    model: Mutex<String>,
    completions_path: String,
    requires_model_load: bool,
    supports_thinking_param: bool,
    provider_id: Option<String>,
    auth_kind: Option<String>,
    chatgpt_account_id: Option<String>,
    openai_auth: Option<OpenAiAuthState>,
    claude_auth: Option<ClaudeCodeAuthState>,
    /// Most recent usage stats observed from SSE events during streaming.
    /// Written by the streaming task, read by the agent on cancellation.
    latest_usage: Arc<Mutex<Option<Usage>>>,
}

fn http_debug_enabled() -> bool {
    static FLAG: OnceCell<bool> = OnceCell::new();
    *FLAG.get_or_init(|| {
        std::env::var("NITE_DEBUG_HTTP")
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

fn http_debug_log(message: impl AsRef<str>) {
    if http_debug_enabled() {
        eprintln!("[NITE HTTP] {}", message.as_ref());
    }
}

fn preview_chunk(text: &str) -> String {
    let mut preview: String = text.chars().take(80).collect();
    if text.chars().count() > 80 {
        preview.push_str("…");
    }
    preview
}

impl HttpBackend {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        completions_path: String,
        requires_model_load: bool,
    ) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        let supports_thinking_param = Self::should_send_thinking_param(&base_url);
        let completions_path = if completions_path.starts_with('/') {
            completions_path
        } else {
            format!("/{}", completions_path)
        };
        let chatgpt_account_id = std::env::var("NITE_HTTP_ACCOUNT_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let provider_id = std::env::var("NITE_HTTP_PROVIDER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let auth_kind = std::env::var("NITE_HTTP_AUTH_KIND")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let openai_auth = if completions_path
            .to_ascii_lowercase()
            .contains("backend-api/codex/responses")
        {
            OpenAiAuthState::from_env(api_key.clone())
        } else {
            None
        };
        let claude_auth = if provider_id.as_deref() == Some("anthropic")
            && auth_kind.as_deref() == Some("claude_code")
        {
            ClaudeCodeAuthState::from_env(api_key.clone())
        } else {
            None
        };
        Self {
            client: Client::new(),
            base_url,
            api_key,
            model: Mutex::new(model),
            completions_path,
            requires_model_load,
            supports_thinking_param,
            provider_id,
            auth_kind,
            chatgpt_account_id,
            openai_auth,
            claude_auth,
            latest_usage: Arc::new(Mutex::new(None)),
        }
    }

    fn should_send_thinking_param(base_url: &str) -> bool {
        if let Some(flag) = Self::env_bool("NITE_HTTP_ENABLE_THINKING") {
            return flag;
        }

        if let Some((host, port)) = Self::extract_host_parts(base_url) {
            if Self::host_is_local_network(&host) || Self::host_matches_allowlist(&host, port) {
                return true;
            }
        } else if Self::host_matches_allowlist(base_url, None) {
            return true;
        }

        let lowered = base_url.to_lowercase();
        lowered.contains("127.0.0.1") || lowered.contains("localhost")
    }

    fn env_bool(var: &str) -> Option<bool> {
        std::env::var(var)
            .ok()
            .and_then(|value| match value.trim().to_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            })
    }

    fn extract_host_parts(base_url: &str) -> Option<(String, Option<u16>)> {
        let parsed = Url::parse(base_url)
            .or_else(|_| Url::parse(&format!("http://{}", base_url)))
            .ok()?;
        let host = parsed.host_str()?.to_string();
        let port = parsed.port();
        Some((host, port))
    }

    fn host_matches_allowlist(host: &str, port: Option<u16>) -> bool {
        let Ok(entries) = std::env::var("NITE_HTTP_ENABLE_THINKING_HOSTS") else {
            return false;
        };

        let host_lower = host.to_lowercase();
        let host_with_port = port.map(|value| format!("{}:{}", host, value));
        let host_with_port_lower = host_with_port.as_ref().map(|value| value.to_lowercase());

        for entry in entries
            .split(',')
            .map(|value| value.trim())
            .filter(|v| !v.is_empty())
        {
            let entry_lower = entry.to_lowercase();
            if entry_lower == host_lower {
                return true;
            }
            if let Some(host_port) = &host_with_port_lower {
                if entry_lower == *host_port {
                    return true;
                }
            }
            if host_lower.ends_with(&entry_lower) {
                return true;
            }
        }

        false
    }

    fn host_is_local_network(host: &str) -> bool {
        if host.eq_ignore_ascii_case("localhost")
            || host.eq_ignore_ascii_case("host.docker.internal")
            || host.eq_ignore_ascii_case("0.0.0.0")
        {
            return true;
        }

        if !host.contains('.') && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return true;
        }

        if let Ok(ip) = host.parse::<IpAddr>() {
            return match ip {
                IpAddr::V4(v4) => {
                    if v4.is_loopback() {
                        return true;
                    }
                    let octets = v4.octets();
                    matches!(
                        octets,
                        [10, _, _, _] | [127, _, _, _] | [192, 168, _, _] | [169, 254, _, _]
                    ) || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                }
                IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
            };
        }

        host.ends_with(".local") || host.ends_with(".lan")
    }

    fn auth_header(&self) -> Option<String> {
        if self.api_key.trim().is_empty() {
            None
        } else {
            Some(format!("Bearer {}", self.api_key))
        }
    }

    fn provider_uses_bearer_auth(&self) -> bool {
        self.provider_id.as_deref() != Some("anthropic")
            || self.auth_kind.as_deref() == Some("claude_code")
    }

    fn has_claude_auth(&self) -> bool {
        self.claude_auth.is_some()
    }

    fn is_anthropic_api(&self) -> bool {
        self.provider_id.as_deref() == Some("anthropic")
            || self
                .completions_path
                .to_ascii_lowercase()
                .contains("/v1/messages")
    }

    async fn ensure_fresh_claude_auth(&self) -> Result<()> {
        if let Some(state) = &self.claude_auth {
            state.ensure_fresh(&self.client, false).await?;
        }
        Ok(())
    }

    async fn force_refresh_claude_auth(&self) -> Result<()> {
        if let Some(state) = &self.claude_auth {
            state.ensure_fresh(&self.client, true).await?;
        }
        Ok(())
    }

    async fn claude_auth_header(&self) -> Option<String> {
        if let Some(state) = &self.claude_auth {
            return state.auth_header().await;
        }
        None
    }

    async fn claude_organization_id(&self) -> Option<String> {
        if let Some(state) = &self.claude_auth {
            return state.organization_id().await;
        }
        std::env::var("NITE_HTTP_ORGANIZATION_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn apply_claude_request_headers(
        &self,
        request: reqwest::RequestBuilder,
        model_id: &str,
    ) -> reqwest::RequestBuilder {
        if let Some(state) = &self.claude_auth {
            return state.apply_request_headers(request, model_id);
        }
        request
    }

    fn user_project_header(&self) -> Option<String> {
        // Prefer explicit user project env var; fall back to project id guess if provided in completions path
        if let Ok(project) = std::env::var("NITE_GOOGLE_USER_PROJECT") {
            if !project.trim().is_empty() {
                return Some(project);
            }
        }
        None
    }

    fn is_google_api(&self) -> bool {
        self.base_url.contains("generativelanguage.googleapis.com")
            && !self
                .completions_path
                .to_lowercase()
                .contains("chat/completions")
    }

    fn is_responses_api(&self) -> bool {
        let path = self.completions_path.to_ascii_lowercase();
        path.contains("/responses") || path.contains("backend-api/codex/responses")
    }

    async fn stream_json_response(
        &self,
        response: reqwest::Response,
        fallback_model: String,
        request_start: Instant,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        let chat_response: OpenAiChatResponse = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("Failed to parse response JSON ({}): {}", status, e))?;
        let id = chat_response
            .id
            .clone()
            .unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4()));
        let model_name = chat_response
            .model
            .clone()
            .unwrap_or_else(|| fallback_model.clone());
        let created = chat_response.created.unwrap_or_else(|| current_timestamp());
        let system_fingerprint = chat_response.system_fingerprint.clone().unwrap_or_default();
        let content = chat_response
            .choices
            .get(0)
            .map(|choice| choice.message.content_text())
            .unwrap_or_default();
        let role = chat_response
            .choices
            .get(0)
            .and_then(|choice| choice.message.role.clone())
            .unwrap_or_else(|| "assistant".to_string());
        let tool_calls = chat_response
            .choices
            .get(0)
            .and_then(|choice| choice.message.tool_calls.as_ref())
            .map(|calls| convert_tool_calls(calls));
        let finish_reason = chat_response
            .choices
            .get(0)
            .and_then(|choice| choice.finish_reason.clone())
            .unwrap_or_else(|| "stop".to_string());

        let chunk_choice = ChunkChoice {
            finish_reason: Some(finish_reason.clone()),
            index: 0,
            delta: Delta {
                content: Some(content.clone()),
                role: role.clone(),
                tool_calls: tool_calls.clone(),
                reasoning_content: None,
            },
            logprobs: None,
        };

        let chunk = ChatCompletionChunkResponse {
            id: id.clone(),
            choices: vec![chunk_choice],
            created: created as u128,
            model: model_name.clone(),
            system_fingerprint: system_fingerprint.clone(),
            object: chat_response
                .object
                .clone()
                .unwrap_or_else(|| "chat.completion.chunk".to_string()),
            usage: None,
        };

        let mut usage = usage_from_openai(chat_response.usage.clone(), &content);
        let token_estimate = estimate_tokens(&content);
        if usage.completion_tokens == 0 {
            usage.completion_tokens = token_estimate;
            usage.total_tokens = token_estimate;
        }
        let elapsed = request_start.elapsed().as_secs_f32();
        if usage.avg_compl_tok_per_sec == 0.0 && token_estimate > 0 && elapsed > 0.0 {
            usage.avg_compl_tok_per_sec = token_estimate as f32 / elapsed.max(0.001);
            usage.total_completion_time_sec = elapsed;
            usage.total_time_sec = elapsed;
        }
        if usage.total_prompt_time_sec == 0.0 && elapsed > 0.0 {
            usage.total_prompt_time_sec = elapsed;
        }

        let response_choice = Choice {
            finish_reason,
            index: 0,
            message: ResponseMessage {
                content: Some(content.clone()),
                role,
                tool_calls,
                reasoning_content: None,
            },
            logprobs: None,
        };

        let done = ChatCompletionResponse {
            id,
            choices: vec![response_choice],
            created,
            model: model_name,
            system_fingerprint,
            object: chat_response
                .object
                .unwrap_or_else(|| "chat.completion".to_string()),
            usage,
        };

        let response_stream = stream::iter(vec![Response::Chunk(chunk), Response::Done(done)]);
        Ok(Box::new(response_stream))
    }

    async fn stream_sse_response(
        &self,
        response: reqwest::Response,
        fallback_model: String,
        request_start: Instant,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared_usage = self.latest_usage.clone();

        tokio::spawn(async move {
            let sender = tx;
            let helper_sender = sender.clone();
            if let Err(err) = process_sse_stream(
                response,
                fallback_model,
                helper_sender,
                request_start,
                shared_usage,
            )
            .await
            {
                let _ = sender.send(Response::InternalError(err.into()));
            }
        });

        Ok(Box::new(UnboundedReceiverStream::new(rx)))
    }
}

#[async_trait]
impl LLMBackend for HttpBackend {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        // Clear stale usage from any previous request
        *self.latest_usage.lock().await = None;

        if self.is_responses_api() {
            return responses::stream_responses_request(self, request).await;
        }
        if self.is_anthropic_api() {
            return anthropic::stream_anthropic_request(self, request).await;
        }

        let mut request_builder = request;
        let enable_thinking: Option<bool> = None;
        let model_name = {
            let guard = self.model.lock().await;
            guard.clone()
        };

        let openai_messages = serialize_messages(request_builder.messages_ref());
        let tools_payload = request_builder.take_tools();

        let mut payload = json!({
            "model": model_name.clone(),
            "messages": openai_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        if self.supports_thinking_param {
            if let Some(enable_thinking) = enable_thinking {
                payload["enable_thinking"] = json!(enable_thinking);
            }
        }

        if let Some((tools, tool_choice)) = tools_payload {
            let is_google_api = self.is_google_api();
            if !tools.is_empty() {
                if is_google_api {
                    let function_declarations: Vec<Value> = tools
                        .into_iter()
                        .map(|tool| {
                            let parameters = tool.function.parameters.unwrap_or_else(|| {
                                let mut default_params = std::collections::HashMap::new();
                                default_params.insert("type".to_string(), json!("object"));
                                default_params.insert("properties".to_string(), json!({}));
                                default_params
                            });
                            json!({
                                "name": tool.function.name,
                                "description": tool.function.description.unwrap_or_default(),
                                "parameters": parameters,
                            })
                        })
                        .collect();

                    if !function_declarations.is_empty() {
                        payload["tools"] = json!([{
                            "functionDeclarations": function_declarations
                        }]);
                    }
                } else {
                    let serialized_tools: Vec<Value> = tools
                        .into_iter()
                        .map(|tool| {
                            json!({
                                "type": "function",
                                "function": {
                                    "name": tool.function.name,
                                    "description": tool.function.description.unwrap_or_default(),
                                    "parameters": tool.function.parameters.unwrap_or_else(|| {
                                        let mut default_params = std::collections::HashMap::new();
                                        default_params.insert("type".to_string(), json!("object"));
                                        default_params.insert("properties".to_string(), json!({}));
                                        default_params
                                    }),
                                }
                            })
                        })
                        .collect();

                    if !serialized_tools.is_empty() {
                        payload["tools"] = Value::Array(serialized_tools);
                    }
                }
            }

            if !is_google_api {
                // Only send tool_choice for OpenAI-compatible APIs
                let tool_choice_value = tool_choice_to_value(tool_choice);
                payload["tool_choice"] = tool_choice_value;
            }
        }

        http_debug_log(format!(
            "Dispatching HTTP request to {}{} with payload {}",
            self.base_url, self.completions_path, payload
        ));
        let request_start = Instant::now();

        let mut request = self
            .client
            .post(format!(
                "{}/{}",
                self.base_url,
                self.completions_path.trim_start_matches('/')
            ))
            .json(&payload);

        if let Some(header) = self.auth_header() {
            request = request.header("Authorization", header);
        }
        if let Some(project) = self.user_project_header() {
            request = request.header("x-goog-user-project", project);
        }
        if !self.api_key.trim().is_empty() {
            request = request.header("x-goog-api-key", &self.api_key);
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API Error {}: {}", status.as_u16(), body));
        }

        let is_streaming = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);

        if is_streaming {
            self.stream_sse_response(response, model_name.clone(), request_start)
                .await
        } else {
            self.stream_json_response(response, model_name.clone(), request_start)
                .await
        }
    }

    async fn load_model(&self) -> Result<()> {
        if !self.requires_model_load {
            return Ok(());
        }

        let model = {
            let guard = self.model.lock().await;
            guard.clone()
        };

        let mut request = self
            .client
            .post(format!("{}/api/load", self.base_url))
            .json(&serde_json::json!({
                "model": model,
                "keep_alive": serde_json::Value::Null,
                "pinned": false,
            }));

        if let Some(header) = self.auth_header() {
            request = request.header("Authorization", header);
        }
        if let Some(project) = self.user_project_header() {
            request = request.header("x-goog-user-project", project);
        }
        if !self.api_key.trim().is_empty() {
            request = request.header("x-goog-api-key", &self.api_key);
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "HTTP backend failed to load model ({}): {}",
                status.as_u16(),
                body
            ));
        }
        Ok(())
    }

    async fn reload_model(&self, model_identifier: String) -> Result<()> {
        let mut guard = self.model.lock().await;
        *guard = model_identifier;
        Ok(())
    }

    async fn get_model(&self) -> Result<Arc<Model>> {
        Err(anyhow::anyhow!(
            "Direct model access not supported in HttpBackend"
        ))
    }

    async fn get_latest_usage(&self) -> Option<Usage> {
        self.latest_usage.lock().await.take()
    }
}

#[derive(Debug, Deserialize, Clone)]
struct OpenAiUsage {
    completion_tokens: Option<usize>,
    prompt_tokens: Option<usize>,
    total_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatMessage {
    #[serde(default)]
    content: Option<OpenAiChatMessageContent>,
    role: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OpenAiChatMessageContent {
    Text(String),
    Parts(Vec<OpenAiChatMessagePart>),
}

#[derive(Debug, Deserialize)]
struct OpenAiChatMessagePart {
    #[serde(rename = "type")]
    _part_type: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiStreamToolCall {
    #[serde(default)]
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    _typ: Option<String>,
    function: Option<OpenAiToolFunction>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiToolFunction {
    name: Option<String>,
    arguments: Option<String>,
}

impl OpenAiChatMessageContent {
    fn to_text(&self) -> String {
        match self {
            OpenAiChatMessageContent::Text(text) => text.clone(),
            OpenAiChatMessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|part| part.text.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl OpenAiChatMessage {
    fn content_text(&self) -> String {
        self.content
            .as_ref()
            .map(|content| content.to_text())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatChoice {
    message: OpenAiChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    id: Option<String>,
    model: Option<String>,
    created: Option<u64>,
    system_fingerprint: Option<String>,
    object: Option<String>,
    choices: Vec<OpenAiChatChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamResponse {
    id: Option<String>,
    model: Option<String>,
    created: Option<u64>,
    system_fingerprint: Option<String>,
    object: Option<String>,
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    index: usize,
    delta: OpenAiStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamDelta {
    #[serde(default)]
    content: Option<OpenAiChatMessageContent>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn serialize_messages(messages: &[IndexMap<String, MessageContent>]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut obj = serde_json::Map::new();
            for (key, value) in message.iter() {
                let normalized_key = if key == "function" {
                    "tool_calls".to_string()
                } else {
                    key.clone()
                };
                match value {
                    Either::Left(text) => {
                        obj.insert(normalized_key, Value::String(text.clone()));
                    }
                    Either::Right(entries) => {
                        let arr = entries
                            .iter()
                            .map(|entry| {
                                let mut inner = serde_json::Map::new();
                                for (k, v) in entry.iter() {
                                    inner.insert(k.clone(), v.clone());
                                }
                                Value::Object(inner)
                            })
                            .collect();
                        obj.insert(normalized_key, Value::Array(arr));
                    }
                }
            }
            Value::Object(obj)
        })
        .collect()
}

fn convert_tool_calls(calls: &[OpenAiStreamToolCall]) -> Vec<ToolCallResponse> {
    calls
        .iter()
        .enumerate()
        .map(|(idx, call)| {
            let index = call.index.unwrap_or(idx);
            let (name, arguments) = if let Some(function) = &call.function {
                (
                    function.name.clone().unwrap_or_default(),
                    function
                        .arguments
                        .clone()
                        .unwrap_or_else(|| "{}".to_string()),
                )
            } else {
                (String::new(), "{}".to_string())
            };

            ToolCallResponse {
                index,
                id: call.id.clone().unwrap_or_else(|| format!("call-{}", index)),
                tp: ToolCallType::Function,
                function: CalledFunction { name, arguments },
            }
        })
        .collect()
}

#[derive(Clone, Default)]
struct StreamingToolCallState {
    id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Clone, Copy, Debug)]
struct StreamMetrics {
    completion_tokens: usize,
    total_time_sec: f32,
    time_to_first_token_sec: f32,
}

impl StreamMetrics {
    fn from_timing(start: Instant, first_token: Option<Instant>, completion_tokens: usize) -> Self {
        let total_time_sec = start.elapsed().as_secs_f32();
        let time_to_first_token_sec = first_token
            .map(|ts| ts.duration_since(start).as_secs_f32())
            .unwrap_or(0.0);
        Self {
            completion_tokens,
            total_time_sec,
            time_to_first_token_sec,
        }
    }
}

fn tool_choice_to_value(tool_choice: ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::Tool(tool) => serde_json::to_value(tool).unwrap_or(Value::Null),
    }
}

fn usage_from_openai(usage: Option<OpenAiUsage>, content: &str) -> Usage {
    if let Some(usage) = usage {
        let completion = usage
            .completion_tokens
            .unwrap_or_else(|| estimate_tokens(content));
        let prompt = usage.prompt_tokens.unwrap_or(0);
        let total = usage.total_tokens.unwrap_or_else(|| completion + prompt);
        Usage {
            completion_tokens: completion,
            prompt_tokens: prompt,
            total_tokens: total,
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        }
    } else {
        Usage {
            completion_tokens: estimate_tokens(content),
            prompt_tokens: 0,
            total_tokens: estimate_tokens(content),
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        }
    }
}

fn update_streaming_tool_calls(
    delta_calls: Vec<OpenAiStreamToolCall>,
    state: &mut Vec<StreamingToolCallState>,
) -> Vec<ToolCallResponse> {
    let mut responses = Vec::new();
    for call in delta_calls {
        let idx = call.index.unwrap_or(state.len());
        if state.len() <= idx {
            state.resize_with(idx + 1, StreamingToolCallState::default);
        }
        if let Some(id) = call.id.clone() {
            state[idx].id = Some(id);
        }
        if let Some(function) = call.function.clone() {
            if let Some(name) = function.name {
                if !name.is_empty() {
                    state[idx].name = name;
                }
            }
            if let Some(arguments) = function.arguments {
                state[idx].arguments.push_str(&arguments);
            }
        }

        let response = ToolCallResponse {
            index: idx,
            id: state[idx]
                .id
                .clone()
                .unwrap_or_else(|| format!("call-{}", idx)),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: state[idx].name.clone(),
                arguments: state[idx].arguments.clone(),
            },
        };
        responses.push(response);
    }
    responses
}

fn finalize_stream_tool_calls(state: &[StreamingToolCallState]) -> Option<Vec<ToolCallResponse>> {
    if state.is_empty() {
        return None;
    }

    let mut calls = Vec::new();
    for (idx, entry) in state.iter().enumerate() {
        if entry.name.is_empty() && entry.arguments.is_empty() {
            continue;
        }

        calls.push(ToolCallResponse {
            index: idx,
            id: entry.id.clone().unwrap_or_else(|| format!("call-{}", idx)),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: entry.name.clone(),
                arguments: entry.arguments.clone(),
            },
        });
    }

    if calls.is_empty() { None } else { Some(calls) }
}

fn find_sse_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    let len = buffer.len();
    let mut i = 0;
    while i + 1 < len {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if i + 3 < len
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

fn extract_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if let Some((idx, sep_len)) = find_sse_separator(buffer) {
        let event = buffer[..idx].to_vec();
        buffer.drain(..idx + sep_len);
        Some(event)
    } else {
        None
    }
}

fn sse_data_from_event(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let mut data_lines = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

fn send_final_done(
    tx: &mpsc::UnboundedSender<Response>,
    response_id: Option<String>,
    response_model: Option<String>,
    created: Option<u64>,
    fingerprint: Option<String>,
    usage: Option<OpenAiUsage>,
    accumulated_content: &str,
    tool_state: &[StreamingToolCallState],
    final_role: &str,
    final_finish_reason: &str,
    fallback_model: &str,
    metrics: Option<StreamMetrics>,
) -> bool {
    let finish_reason = if final_finish_reason.is_empty() {
        "stop".to_string()
    } else {
        final_finish_reason.to_string()
    };

    let message = ResponseMessage {
        content: Some(accumulated_content.to_string()),
        role: final_role.to_string(),
        tool_calls: finalize_stream_tool_calls(tool_state),
        reasoning_content: None,
    };

    let choice = Choice {
        finish_reason,
        index: 0,
        message,
        logprobs: None,
    };

    let mut usage = usage_from_openai(usage, accumulated_content);
    if let Some(metrics) = metrics {
        if usage.completion_tokens == 0 && metrics.completion_tokens > 0 {
            usage.completion_tokens = metrics.completion_tokens;
            usage.total_tokens = metrics.completion_tokens;
        }
        if usage.avg_compl_tok_per_sec == 0.0
            && metrics.completion_tokens > 0
            && metrics.total_time_sec > 0.0
        {
            usage.avg_compl_tok_per_sec =
                metrics.completion_tokens as f32 / metrics.total_time_sec.max(0.001);
            usage.total_completion_time_sec = metrics.total_time_sec;
            usage.total_time_sec = metrics.total_time_sec;
        }
        if usage.total_prompt_time_sec == 0.0 && metrics.time_to_first_token_sec > 0.0 {
            usage.total_prompt_time_sec = metrics.time_to_first_token_sec;
        }
    }

    let done = ChatCompletionResponse {
        id: response_id.unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4())),
        choices: vec![choice],
        created: created.unwrap_or_else(|| current_timestamp()),
        model: response_model.unwrap_or_else(|| fallback_model.to_string()),
        system_fingerprint: fingerprint.unwrap_or_default(),
        object: "chat.completion".to_string(),
        usage,
    };

    tx.send(Response::Done(done)).is_ok()
}

async fn process_sse_stream(
    response: reqwest::Response,
    fallback_model: String,
    tx: mpsc::UnboundedSender<Response>,
    request_start: Instant,
    shared_usage: Arc<Mutex<Option<Usage>>>,
) -> Result<()> {
    let mut body_stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut accumulated_content = String::new();
    let mut tool_state: Vec<StreamingToolCallState> = Vec::new();
    let mut final_finish_reason = String::from("stop");
    let mut final_role = String::from("assistant");
    let mut response_id: Option<String> = None;
    let mut response_model: Option<String> = None;
    let mut created_ts: Option<u64> = None;
    let mut fingerprint: Option<String> = None;
    let mut latest_usage: Option<OpenAiUsage> = None;
    let mut first_token_time: Option<Instant> = None;
    let mut estimated_tokens: usize = 0;

    while let Some(item) = body_stream.next().await {
        if tx.is_closed() {
            http_debug_log("SSE downstream closed; aborting stream read");
            // Propagate whatever usage we have before aborting
            if let Some(ref usage) = latest_usage {
                let converted = usage_from_openai(Some(usage.clone()), &accumulated_content);
                *shared_usage.lock().await = Some(converted);
            }
            return Ok(());
        }

        let chunk = item?;
        buffer.extend_from_slice(&chunk);

        while let Some(event_bytes) = extract_sse_event(&mut buffer) {
            if tx.is_closed() {
                http_debug_log("SSE downstream closed during event processing; aborting");
                if let Some(ref usage) = latest_usage {
                    let converted = usage_from_openai(Some(usage.clone()), &accumulated_content);
                    *shared_usage.lock().await = Some(converted);
                }
                return Ok(());
            }

            if event_bytes.is_empty() {
                continue;
            }

            let Some(data) = sse_data_from_event(&event_bytes) else {
                continue;
            };

            let trimmed = data.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed == "[DONE]" {
                http_debug_log("SSE stream received [DONE]");
                let metrics = Some(StreamMetrics::from_timing(
                    request_start,
                    first_token_time,
                    estimated_tokens,
                ));
                send_final_done(
                    &tx,
                    response_id.clone(),
                    response_model.clone(),
                    created_ts,
                    fingerprint.clone(),
                    latest_usage.clone(),
                    &accumulated_content,
                    &tool_state,
                    &final_role,
                    &final_finish_reason,
                    &fallback_model,
                    metrics,
                );
                return Ok(());
            }

            let parsed: OpenAiStreamResponse = match serde_json::from_str(&data) {
                Ok(value) => value,
                Err(err) => {
                    http_debug_log(format!("Failed to parse SSE chunk: {}", err));
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(error) = value.get("error") {
                            return Err(anyhow::anyhow!("Remote error: {}", error));
                        }
                    }
                    return Err(anyhow::anyhow!("Failed to parse stream chunk: {}", err));
                }
            };

            http_debug_log(format!(
                "Parsed SSE chunk with {} choice(s)",
                parsed.choices.len()
            ));

            if response_id.is_none() {
                response_id = parsed.id.clone();
            }
            if response_model.is_none() {
                response_model = parsed.model.clone();
            }
            if parsed.created.is_some() && created_ts.is_none() {
                created_ts = parsed.created;
            }
            if parsed.system_fingerprint.is_some() && fingerprint.is_none() {
                fingerprint = parsed.system_fingerprint.clone();
            }
            if parsed.usage.is_some() {
                latest_usage = parsed.usage.clone();
                // Eagerly propagate to shared state so cancellation can read it
                if let Some(ref usage) = latest_usage {
                    let converted = usage_from_openai(Some(usage.clone()), &accumulated_content);
                    *shared_usage.lock().await = Some(converted);
                }
            }

            let chunk_id = parsed
                .id
                .clone()
                .or_else(|| response_id.clone())
                .unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4()));
            let chunk_model = parsed
                .model
                .clone()
                .or_else(|| response_model.clone())
                .unwrap_or_else(|| fallback_model.clone());
            let chunk_created = parsed
                .created
                .or(created_ts)
                .unwrap_or_else(|| current_timestamp());
            let chunk_fingerprint = parsed
                .system_fingerprint
                .clone()
                .or_else(|| fingerprint.clone())
                .unwrap_or_default();
            let chunk_object = parsed
                .object
                .clone()
                .unwrap_or_else(|| "chat.completion.chunk".to_string());

            for choice in parsed.choices {
                if let Some(reason) = &choice.finish_reason {
                    final_finish_reason = reason.clone();
                }

                let OpenAiStreamDelta {
                    content,
                    role,
                    tool_calls,
                } = choice.delta;

                if let Some(new_role) = role.clone() {
                    final_role = new_role;
                }
                let mut delta_role = final_role.clone();
                if let Some(role_override) = role {
                    delta_role = role_override;
                }

                let mut delta_content = None;
                if let Some(content_value) = content {
                    let text = content_value.to_text();
                    if !text.is_empty() {
                        http_debug_log(format!(
                            "delta content len={} preview=\"{}\"",
                            text.chars().count(),
                            preview_chunk(&text)
                        ));
                        accumulated_content.push_str(&text);
                        if first_token_time.is_none() {
                            first_token_time = Some(Instant::now());
                        }
                        estimated_tokens += estimate_tokens(&text);
                        delta_content = Some(text);
                    }
                } else {
                    http_debug_log("delta content missing".to_string());
                }

                let mut delta_tool_calls = Vec::new();
                if let Some(tool_call_vec) = tool_calls {
                    http_debug_log(format!(
                        "delta includes {} tool call(s)",
                        tool_call_vec.len()
                    ));
                    delta_tool_calls = update_streaming_tool_calls(tool_call_vec, &mut tool_state);
                }

                if delta_content.is_none()
                    && delta_tool_calls.is_empty()
                    && choice.finish_reason.is_none()
                {
                    continue;
                }

                let delta = Delta {
                    content: delta_content.clone(),
                    role: delta_role,
                    tool_calls: if delta_tool_calls.is_empty() {
                        None
                    } else {
                        Some(delta_tool_calls.clone())
                    },
                    reasoning_content: None,
                };

                let chunk_choice = ChunkChoice {
                    finish_reason: choice.finish_reason.clone(),
                    index: choice.index,
                    delta,
                    logprobs: None,
                };

                let chunk_response = ChatCompletionChunkResponse {
                    id: chunk_id.clone(),
                    choices: vec![chunk_choice],
                    created: chunk_created as u128,
                    model: chunk_model.clone(),
                    system_fingerprint: chunk_fingerprint.clone(),
                    object: chunk_object.clone(),
                    usage: None,
                };

                if tx.send(Response::Chunk(chunk_response)).is_err() {
                    http_debug_log("SSE downstream closed while sending chunk; aborting");
                    return Ok(());
                }
            }
        }
    }

    let metrics = Some(StreamMetrics::from_timing(
        request_start,
        first_token_time,
        estimated_tokens,
    ));

    let _ = send_final_done(
        &tx,
        response_id,
        response_model,
        created_ts,
        fingerprint,
        latest_usage,
        &accumulated_content,
        &tool_state,
        &final_role,
        &final_finish_reason,
        &fallback_model,
        metrics,
    );

    Ok(())
}
