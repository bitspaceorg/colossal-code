use anyhow::Result;
use either::Either;
use futures::StreamExt;
use indexmap::IndexMap;
use mistralrs::{
    ChatCompletionChunkResponse, Choice, ChunkChoice, Delta, RequestBuilder, RequestLike, Response,
    ResponseMessage, Usage,
};
use mistralrs_core::{CalledFunction, MessageContent, ToolCallResponse, ToolCallType, ToolChoice};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

use super::HttpBackend;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const SYSTEM_IDENTITY_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const TOOL_PREFIX: &str = "mcp_";

/// Budget token counts for each effort level sent via the `effort-2025-11-24` beta.
/// Anthropic maps effort to thinking budget rather than an enum string.
const THINKING_BUDGET_LOW: u64 = 1_024;
const THINKING_BUDGET_MEDIUM: u64 = 8_000;
const THINKING_BUDGET_HIGH: u64 = 16_000;

fn anthropic_thinking_budget() -> Option<u64> {
    let effort = std::env::var("NITE_REASONING_EFFORT")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())?;
    match effort.as_str() {
        "low" => Some(THINKING_BUDGET_LOW),
        "medium" => Some(THINKING_BUDGET_MEDIUM),
        "high" => Some(THINKING_BUDGET_HIGH),
        _ => None,
    }
}

pub(super) async fn stream_anthropic_request(
    backend: &HttpBackend,
    mut request_builder: RequestBuilder,
) -> Result<Box<dyn futures::stream::Stream<Item = Response> + Unpin + Send>> {
    backend.ensure_fresh_claude_auth().await?;

    let model_name = {
        let guard = backend.model.lock().await;
        guard.clone()
    };

    let messages = serialize_anthropic_messages(request_builder.messages_ref());
    let system = extract_system_prompt(request_builder.messages_ref());
    let tools_payload = request_builder.take_tools();

    let thinking_budget = anthropic_thinking_budget();
    // When extended thinking is active, max_tokens must exceed budget_tokens.
    let max_tokens: u64 = match thinking_budget {
        Some(budget) => (budget + 16_000).max(32_000),
        None => 32_000,
    };

    let mut payload = json!({
        "model": model_name.clone(),
        "messages": messages,
        "stream": true,
        "max_tokens": max_tokens,
    });

    if let Some(budget) = thinking_budget {
        payload["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if !system.is_empty() {
        payload["system"] = json!(system_blocks(&system));
    }

    if let Some((tools, tool_choice)) = tools_payload {
        if !tools.is_empty() {
            let serialized_tools: Vec<Value> = tools
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": prefixed_tool_name(&tool.function.name),
                        "description": tool.function.description.unwrap_or_default(),
                        "input_schema": tool.function.parameters.unwrap_or_else(|| {
                            let mut default_params = std::collections::HashMap::new();
                            default_params.insert("type".to_string(), json!("object"));
                            default_params.insert("properties".to_string(), json!({}));
                            default_params
                        }),
                    })
                })
                .collect();
            payload["tools"] = Value::Array(serialized_tools);
            payload["tool_choice"] = anthropic_tool_choice_to_value(tool_choice);
        }
    }

    let response = send_anthropic_request(backend, &model_name, &payload).await?;

    let (tx, rx) = mpsc::unbounded_channel();
    let request_start = Instant::now();
    let shared_usage = backend.latest_usage.clone();
    tokio::spawn(async move {
        let sender = tx;
        if let Err(err) = process_anthropic_sse(
            response,
            model_name,
            sender.clone(),
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

async fn send_anthropic_request(
    backend: &HttpBackend,
    model_name: &str,
    payload: &Value,
) -> Result<reqwest::Response> {
    let response = build_anthropic_request(backend, model_name, payload)
        .await
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();

    if status.as_u16() == 401
        && backend.has_claude_auth()
        && backend.can_force_refresh_claude_auth()
    {
        backend.force_refresh_claude_auth().await?;
        let retried = build_anthropic_request(backend, model_name, payload)
            .await
            .send()
            .await?;
        let retried_status = retried.status();
        if retried_status.is_success() {
            return Ok(retried);
        }
        let retried_body = retried.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Anthropic API Error {}: {}",
            retried_status.as_u16(),
            retried_body
        ));
    }

    if should_retry_without_tools(
        status.as_u16(),
        payload.get("tools").is_some(),
        &body,
        backend,
    ) {
        let mut retry_payload = payload.clone();
        if let Some(object) = retry_payload.as_object_mut() {
            object.remove("tools");
            object.remove("tool_choice");
        }
        let retried = build_anthropic_request(backend, model_name, &retry_payload)
            .await
            .send()
            .await?;
        let retried_status = retried.status();
        if retried_status.is_success() {
            return Ok(retried);
        }
        let retried_body = retried.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Anthropic API Error {}: {}",
            retried_status.as_u16(),
            retried_body
        ));
    }

    Err(anyhow::anyhow!(
        "Anthropic API Error {}: {}",
        status.as_u16(),
        body
    ))
}

fn should_retry_without_tools(
    status: u16,
    had_tools: bool,
    body: &str,
    backend: &HttpBackend,
) -> bool {
    status == 400
        && had_tools
        && body.contains("invalid_request_error")
        && !backend.has_claude_auth()
}

async fn build_anthropic_request(
    backend: &HttpBackend,
    model_name: &str,
    payload: &Value,
) -> reqwest::RequestBuilder {
    let mut request = backend
        .client
        .post(format!(
            "{}/{}",
            backend.base_url,
            backend.completions_path.trim_start_matches('/')
        ))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(payload);

    if backend.provider_uses_bearer_auth() {
        request = backend.apply_claude_request_headers(request, model_name);
        if let Some(header) = backend
            .claude_auth_header()
            .await
            .or_else(|| backend.auth_header())
        {
            request = request.header("Authorization", header);
        }
    } else if !backend.api_key.trim().is_empty() {
        request = request.header("x-api-key", &backend.api_key);
    }

    if let Some(org_id) = backend.claude_organization_id().await {
        request = request.header("x-organization-uuid", org_id);
    }

    request
}

fn anthropic_tool_choice_to_value(tool_choice: ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::None => json!({ "type": "none" }),
        ToolChoice::Auto => json!({ "type": "auto" }),
        ToolChoice::Tool(tool) => json!({
            "type": "tool",
            "name": prefixed_tool_name(&tool.function.name),
        }),
    }
}

fn extract_system_prompt(messages: &[IndexMap<String, MessageContent>]) -> String {
    let mut parts = vec![SYSTEM_IDENTITY_PREFIX.to_string()];

    for message in messages {
        let role = message
            .get("role")
            .and_then(content_as_text)
            .unwrap_or_default();
        if role != "system" {
            continue;
        }

        let content = message
            .get("content")
            .and_then(content_as_text)
            .unwrap_or_default();
        if should_forward_system_message(&content) {
            parts.push(content);
        }
    }

    parts.join("\n\n")
}

fn should_forward_system_message(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.contains("<system-reminder>") && trimmed.contains("</system-reminder>")
}

fn system_blocks(system: &str) -> Vec<Value> {
    vec![json!({
        "type": "text",
        "text": system,
    })]
}

fn serialize_anthropic_messages(messages: &[IndexMap<String, MessageContent>]) -> Vec<Value> {
    let mut serialized = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(content_as_text)
            .unwrap_or_default();
        if role == "system" {
            continue;
        }

        if role == "tool" {
            let tool_use_id = message
                .get("tool_call_id")
                .and_then(content_as_text)
                .unwrap_or_default();
            let content = message
                .get("content")
                .and_then(content_as_text)
                .unwrap_or_default();
            serialized.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                }],
            }));
            continue;
        }

        let mut content = Vec::new();
        let text = message
            .get("content")
            .and_then(content_as_text)
            .unwrap_or_default();
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }

        if role == "assistant" {
            for tool in assistant_tool_calls_from_message(message) {
                content.push(json!({
                    "type": "tool_use",
                    "id": tool.id,
                    "name": prefixed_tool_name(&tool.function.name),
                    "input": serde_json::from_str::<Value>(&tool.function.arguments)
                        .unwrap_or_else(|_| json!({})),
                }));
            }
        }

        serialized.push(json!({
            "role": role,
            "content": content,
        }));
    }

    serialized
}

fn content_as_text(value: &MessageContent) -> Option<String> {
    match value {
        Either::Left(text) => Some(text.clone()),
        Either::Right(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        ),
    }
}

fn assistant_tool_calls_from_message(
    msg: &indexmap::IndexMap<String, MessageContent>,
) -> Vec<ToolCallResponse> {
    let Some(value) = msg.get("tool_calls").or_else(|| msg.get("function")) else {
        return Vec::new();
    };
    let Either::Right(parts) = value else {
        return Vec::new();
    };

    parts
        .iter()
        .enumerate()
        .map(|(idx, tool)| {
            let index = tool
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(idx);
            let id = tool
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = tool
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = tool
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();

            ToolCallResponse {
                index,
                id,
                tp: ToolCallType::Function,
                function: CalledFunction { name, arguments },
            }
        })
        .collect()
}

fn prefixed_tool_name(name: &str) -> String {
    if name.starts_with(TOOL_PREFIX) {
        name.to_string()
    } else {
        format!("{}{}", TOOL_PREFIX, name)
    }
}

fn stripped_tool_name(name: &str) -> String {
    name.strip_prefix(TOOL_PREFIX).unwrap_or(name).to_string()
}

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    message: Option<AnthropicMessage>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    error: Option<AnthropicStreamError>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamError {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<usize>,
    #[serde(default)]
    output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "partial_json")]
    partial_json: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Clone, Default)]
struct ToolState {
    id: Option<String>,
    name: String,
    arguments: String,
}

async fn process_anthropic_sse(
    response: reqwest::Response,
    fallback_model: String,
    tx: mpsc::UnboundedSender<Response>,
    request_start: Instant,
    shared_usage: Arc<Mutex<Option<Usage>>>,
) -> Result<()> {
    let mut body_stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut accumulated_content = String::new();
    let mut first_token_time: Option<Instant> = None;
    let mut response_id: Option<String> = None;
    let mut response_model: Option<String> = None;
    let mut final_finish_reason = "stop".to_string();
    let mut usage = Usage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        avg_prompt_tok_per_sec: 0.0,
        avg_compl_tok_per_sec: 0.0,
        avg_tok_per_sec: 0.0,
        total_prompt_time_sec: 0.0,
        total_completion_time_sec: 0.0,
        total_time_sec: 0.0,
    };
    let mut tool_state: Vec<ToolState> = Vec::new();
    let mut current_tool_index: Option<usize> = None;
    let stream_completed = false;

    while let Some(item) = body_stream.next().await {
        let chunk = item?;
        buffer.extend_from_slice(&chunk);

        while let Some(event_bytes) = extract_sse_event(&mut buffer) {
            let Some(data) = sse_data_from_event(&event_bytes) else {
                continue;
            };
            let trimmed = data.trim();
            if trimmed.is_empty() {
                continue;
            }

            let parsed: AnthropicEvent = serde_json::from_str(trimmed)?;
            match parsed.event_type.as_str() {
                "message_start" => {
                    if let Some(message) = parsed.message {
                        response_id = message.id;
                        response_model = message.model;
                        if let Some(message_usage) = message.usage {
                            usage.prompt_tokens = message_usage.input_tokens.unwrap_or(0);
                            // Eagerly propagate prompt_tokens so cancellation can read it
                            *shared_usage.lock().await = Some(usage.clone());
                        }
                    }
                }
                "content_block_start" => {
                    if let Some(block) = parsed.content_block {
                        match block.block_type.as_str() {
                            "text" => {
                                if let Some(text) = block.text {
                                    accumulated_content.push_str(&text);
                                }
                            }
                            "tool_use" => {
                                current_tool_index = Some(tool_state.len());
                                tool_state.push(ToolState {
                                    id: block.id,
                                    name: stripped_tool_name(&block.name.unwrap_or_default()),
                                    arguments: String::new(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(delta) = parsed.delta {
                        match delta.delta_type.as_deref() {
                            Some("text_delta") => {
                                if let Some(text) = delta.text {
                                    if first_token_time.is_none() {
                                        first_token_time = Some(Instant::now());
                                    }
                                    accumulated_content.push_str(&text);
                                    let chunk = ChatCompletionChunkResponse {
                                        id: response_id.clone().unwrap_or_else(|| {
                                            format!("nite-http-{}", Uuid::new_v4())
                                        }),
                                        choices: vec![ChunkChoice {
                                            finish_reason: None,
                                            index: 0,
                                            delta: Delta {
                                                content: Some(text),
                                                role: "assistant".to_string(),
                                                tool_calls: None,
                                                reasoning_content: None,
                                            },
                                            logprobs: None,
                                        }],
                                        created: current_timestamp() as u128,
                                        model: response_model
                                            .clone()
                                            .unwrap_or_else(|| fallback_model.clone()),
                                        system_fingerprint: String::new(),
                                        object: "chat.completion.chunk".to_string(),
                                        usage: None,
                                    };
                                    if tx.send(Response::Chunk(chunk)).is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(idx) = current_tool_index {
                                    if let Some(partial_json) = delta.partial_json {
                                        if let Some(entry) = tool_state.get_mut(idx) {
                                            entry.arguments.push_str(&partial_json);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_stop" => {
                    current_tool_index = None;
                }
                "message_delta" => {
                    if let Some(delta) = parsed.delta {
                        if let Some(reason) = delta.stop_reason {
                            final_finish_reason = reason;
                        }
                    }
                    if let Some(delta_usage) = parsed.usage {
                        usage.completion_tokens = delta_usage.output_tokens.unwrap_or(0);
                        usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
                        *shared_usage.lock().await = Some(usage.clone());
                    }
                }
                "message_stop" => {
                    let elapsed = request_start.elapsed().as_secs_f32();
                    if usage.completion_tokens == 0 && !accumulated_content.is_empty() {
                        usage.completion_tokens =
                            accumulated_content.split_whitespace().count().max(1);
                        usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
                    }
                    usage.total_time_sec = elapsed;
                    usage.total_completion_time_sec = elapsed;
                    usage.avg_compl_tok_per_sec = if elapsed > 0.0 {
                        usage.completion_tokens as f32 / elapsed.max(0.001)
                    } else {
                        0.0
                    };
                    usage.avg_tok_per_sec = usage.avg_compl_tok_per_sec;
                    if let Some(first) = first_token_time {
                        usage.total_prompt_time_sec =
                            first.duration_since(request_start).as_secs_f32();
                    }

                    let tool_calls = finalize_tools(&tool_state);
                    let done = mistralrs::ChatCompletionResponse {
                        id: response_id.unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4())),
                        choices: vec![Choice {
                            finish_reason: final_finish_reason,
                            index: 0,
                            message: ResponseMessage {
                                content: Some(accumulated_content),
                                role: "assistant".to_string(),
                                tool_calls,
                                reasoning_content: None,
                            },
                            logprobs: None,
                        }],
                        created: current_timestamp(),
                        model: response_model.unwrap_or(fallback_model),
                        system_fingerprint: String::new(),
                        object: "chat.completion".to_string(),
                        usage,
                    };
                    let _ = tx.send(Response::Done(done));
                    return Ok(());
                }
                "error" => {
                    let error_msg = parsed
                        .error
                        .as_ref()
                        .and_then(|err| err.message.as_deref())
                        .unwrap_or("unknown error");
                    let error_type = parsed
                        .error
                        .as_ref()
                        .and_then(|err| err.error_type.as_deref())
                        .unwrap_or("error");
                    return Err(anyhow::anyhow!(
                        "Anthropic stream error ({}): {}",
                        error_type,
                        error_msg
                    ));
                }
                _ => {}
            }
        }
    }

    if !stream_completed {
        return Err(anyhow::anyhow!(
            "Anthropic stream ended without completing — the connection may have been interrupted"
        ));
    }

    Ok(())
}

fn finalize_tools(state: &[ToolState]) -> Option<Vec<ToolCallResponse>> {
    let calls: Vec<ToolCallResponse> = state
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.name.is_empty())
        .map(|(idx, entry)| ToolCallResponse {
            index: idx,
            id: entry.id.clone().unwrap_or_else(|| format!("call-{}", idx)),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: entry.name.clone(),
                arguments: if entry.arguments.is_empty() {
                    "{}".to_string()
                } else {
                    entry.arguments.clone()
                },
            },
        })
        .collect();
    if calls.is_empty() { None } else { Some(calls) }
}

fn extract_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let len = buffer.len();
    let mut i = 0;
    while i + 1 < len {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            let event = buffer[..i].to_vec();
            buffer.drain(..i + 2);
            return Some(event);
        }
        i += 1;
    }
    None
}

fn sse_data_from_event(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let data_lines: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect();
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_backend::claude_auth::ClaudeCodeAuthState;
    use either::Either;

    fn text_message(role: &str, content: &str) -> IndexMap<String, MessageContent> {
        let mut message = IndexMap::new();
        message.insert("role".to_string(), Either::Left(role.to_string()));
        message.insert("content".to_string(), Either::Left(content.to_string()));
        message
    }

    #[test]
    fn extract_system_prompt_forwards_only_system_reminders() {
        let messages = vec![
            text_message("system", "base system prompt should stay out"),
            text_message(
                "system",
                "<system-reminder>\nPlan mode active.\n</system-reminder>",
            ),
            text_message("user", "hello"),
        ];

        let system = extract_system_prompt(&messages);

        assert!(system.starts_with(SYSTEM_IDENTITY_PREFIX));
        assert!(system.contains("<system-reminder>\nPlan mode active.\n</system-reminder>"));
        assert!(!system.contains("base system prompt should stay out"));
    }

    #[test]
    fn does_not_strip_tools_for_claude_code_requests() {
        let backend = HttpBackend {
            client: reqwest::Client::new(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "token".to_string(),
            model: Mutex::new("claude-sonnet-4-6".to_string()),
            completions_path: "/v1/messages".to_string(),
            requires_model_load: false,
            supports_thinking_param: false,
            provider_id: Some("anthropic".to_string()),
            auth_kind: Some("claude_code".to_string()),
            chatgpt_account_id: None,
            openai_auth: None,
            claude_auth: ClaudeCodeAuthState::from_env("token".to_string()),
            latest_usage: Arc::new(Mutex::new(None)),
        };

        assert!(!should_retry_without_tools(
            400,
            true,
            "invalid_request_error",
            &backend,
        ));
    }
}
