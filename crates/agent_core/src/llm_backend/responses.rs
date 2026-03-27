use anyhow::Result;
use either::Either;
use futures::stream::Stream as FuturesStream;
use mistralrs::{
    ChatCompletionChunkResponse, Choice, ChunkChoice, Delta, RequestBuilder, RequestLike, Response,
    ResponseMessage, ToolChoice, Usage,
};
use mistralrs_core::{CalledFunction, MessageContent, ToolCallResponse, ToolCallType};
use serde_json::{Value, json};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

use super::{
    HttpBackend, OpenAiUsage, estimate_tokens, message_adapter::assistant_tool_calls_from_message,
    usage_from_openai,
};

const DEFAULT_INSTRUCTIONS: &str = "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand.";

pub(super) async fn stream_responses_request(
    backend: &HttpBackend,
    mut request_builder: RequestBuilder,
) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
    let model_name = {
        let guard = backend.model.lock().await;
        guard.clone()
    };

    let payload = build_responses_payload(backend, &mut request_builder, &model_name);

    let request_start = Instant::now();
    let mut request = backend
        .client
        .post(format!(
            "{}/{}",
            backend.base_url,
            backend.completions_path.trim_start_matches('/')
        ))
        .json(&payload);

    if let Some(header) = backend.auth_header() {
        request = request.header("Authorization", header);
    }
    if let Some(account_id) = backend.chatgpt_account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Responses API Error {}: {}",
            status.as_u16(),
            body
        ));
    }

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let sender = tx;
        if let Err(err) =
            process_responses_sse(response, model_name, sender.clone(), request_start).await
        {
            let _ = sender.send(Response::InternalError(err.into()));
        }
    });

    Ok(Box::new(UnboundedReceiverStream::new(rx)))
}

fn build_responses_payload(
    backend: &HttpBackend,
    request_builder: &mut RequestBuilder,
    model_name: &str,
) -> Value {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    let sampling_params = request_builder.take_sampling_params();

    for msg in request_builder.messages_ref() {
        let role = msg
            .get("role")
            .and_then(content_as_text)
            .unwrap_or_default();
        let content = msg
            .get("content")
            .and_then(content_as_text)
            .unwrap_or_default();

        match role.as_str() {
            "system" => {
                if !content.trim().is_empty() {
                    instructions.push(content.trim().to_string());
                }
            }
            "user" => {
                input.push(json!({
                    "role": "user",
                    "content": [{ "type": "input_text", "text": content }],
                }));
            }
            "assistant" => {
                for tool in assistant_tool_calls_from_message(msg) {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": tool.id,
                        "name": tool.function.name,
                        "arguments": tool.function.arguments,
                    }));
                }
                if !content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": content }],
                    }));
                }
            }
            "tool" => {
                let call_id = msg
                    .get("tool_call_id")
                    .and_then(content_as_text)
                    .unwrap_or_default();
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": content,
                }));
            }
            _ => {}
        }
    }

    let mut payload = json!({
        "model": model_name,
        "input": input,
        "stream": true,
    });

    if let Some(store) = backend.profile.maybe_store_flag() {
        payload["store"] = store;
    }

    backend
        .profile
        .apply_max_tokens(&mut payload, sampling_params.max_len);

    if backend.profile.requires_instructions() {
        payload["instructions"] = Value::String(if instructions.is_empty() {
            DEFAULT_INSTRUCTIONS.to_string()
        } else {
            instructions.join("\n\n")
        });
    }

    if let Some((tools, tool_choice)) = request_builder.take_tools() {
        let serialized_tools: Vec<Value> = tools
            .into_iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.function.name,
                    "description": tool.function.description.unwrap_or_default(),
                    "parameters": tool.function.parameters.unwrap_or_else(|| {
                        let mut default_params = std::collections::HashMap::new();
                        default_params.insert("type".to_string(), json!("object"));
                        default_params.insert("properties".to_string(), json!({}));
                        default_params
                    }),
                })
            })
            .collect();
        if !serialized_tools.is_empty() {
            payload["tools"] = Value::Array(serialized_tools);
            payload["tool_choice"] = responses_tool_choice_to_value(tool_choice);
        }
    }

    payload
}

fn responses_tool_choice_to_value(tool_choice: ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::Tool(tool) => json!({
            "type": "function",
            "name": tool.function.name,
        }),
    }
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

#[derive(Clone, Default)]
struct ResponseToolState {
    item_id: Option<String>,
    call_id: Option<String>,
    name: String,
    arguments: String,
}

async fn process_responses_sse(
    response: reqwest::Response,
    fallback_model: String,
    tx: mpsc::UnboundedSender<Response>,
    request_start: Instant,
) -> Result<()> {
    let mut body_stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut accumulated = String::new();
    let mut usage: Option<Usage> = None;
    let mut response_id: Option<String> = None;
    let mut response_model: Option<String> = None;
    let mut created: Option<u64> = None;
    let mut tool_state: Vec<ResponseToolState> = Vec::new();
    let mut first_token_time: Option<Instant> = None;
    let role = String::from("assistant");
    let mut saw_sse_event = false;

    while let Some(item) = futures::StreamExt::next(&mut body_stream).await {
        if tx.is_closed() {
            return Ok(());
        }

        let chunk = item?;
        buffer.extend_from_slice(&chunk);

        while let Some(event_bytes) = extract_sse_event(&mut buffer) {
            saw_sse_event = true;
            let Some(data) = sse_data_from_event(&event_bytes) else {
                continue;
            };
            let trimmed = data.trim();
            if trimmed.is_empty() || trimmed == "[DONE]" {
                continue;
            }

            let value: Value = serde_json::from_str(trimmed)?;
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();

            match kind {
                "response.created" => {
                    let response_value = value.get("response").unwrap_or(&value);
                    response_id = response_value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    response_model = response_value
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    created = response_value.get("created_at").and_then(Value::as_u64);
                }
                "response.output_text.delta" => {
                    let delta = value
                        .get("delta")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if delta.is_empty() {
                        continue;
                    }
                    accumulated.push_str(&delta);
                    if first_token_time.is_none() {
                        first_token_time = Some(Instant::now());
                    }
                    let chunk = make_text_chunk(
                        response_id.clone(),
                        response_model.clone(),
                        created,
                        &fallback_model,
                        role.clone(),
                        delta,
                    );
                    if tx.send(Response::Chunk(chunk)).is_err() {
                        return Ok(());
                    }
                }
                "response.output_text.done" => {
                    if accumulated.is_empty() {
                        if let Some(text) = value.get("text").and_then(Value::as_str) {
                            accumulated.push_str(text);
                        }
                    }
                }
                "response.output_item.added" | "response.output_item.done" => {
                    if let Some(item_value) = value.get("item") {
                        ingest_function_call_item(item_value, &mut tool_state);
                    }
                }
                "response.function_call_arguments.delta" => {
                    let idx = tool_index_for_event(&value, &mut tool_state);
                    if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                        tool_state[idx].arguments.push_str(delta);
                    }
                }
                "response.function_call_arguments.done" => {
                    let idx = tool_index_for_event(&value, &mut tool_state);
                    if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
                        tool_state[idx].arguments = arguments.to_string();
                    }
                }
                "response.refusal.delta" | "response.refusal.done" => {}
                "error" | "response.error" => {
                    return Err(anyhow::anyhow!("Responses stream error: {}", trimmed));
                }
                "response.completed" => {
                    let response_value = value.get("response").unwrap_or(&value);
                    response_id = response_value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or(response_id);
                    response_model = response_value
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or(response_model);
                    created = response_value
                        .get("created_at")
                        .and_then(Value::as_u64)
                        .or(created);

                    if accumulated.is_empty() {
                        accumulated = extract_text_from_output(response_value);
                    }
                    usage = Some(extract_usage(response_value, &accumulated));

                    let done = make_done_response(
                        response_id,
                        response_model,
                        created,
                        &fallback_model,
                        role,
                        accumulated,
                        finalize_tools(&tool_state),
                        usage.unwrap_or_else(|| extract_usage(response_value, "")),
                        request_start,
                        first_token_time,
                    );
                    let _ = tx.send(Response::Done(done));
                    return Ok(());
                }
                "response.failed" => {
                    return Err(anyhow::anyhow!("Responses stream failed: {}", trimmed));
                }
                _ => {}
            }
        }
    }

    if !saw_sse_event && !buffer.is_empty() {
        return process_responses_json_fallback(
            String::from_utf8_lossy(&buffer).to_string(),
            fallback_model,
            tx,
            request_start,
        );
    }

    let done = make_done_response(
        response_id,
        response_model,
        created,
        &fallback_model,
        role,
        accumulated.clone(),
        finalize_tools(&tool_state),
        usage.unwrap_or_else(|| usage_from_openai(None, &accumulated)),
        request_start,
        first_token_time,
    );
    let _ = tx.send(Response::Done(done));
    Ok(())
}

fn process_responses_json_fallback(
    body: String,
    fallback_model: String,
    tx: mpsc::UnboundedSender<Response>,
    request_start: Instant,
) -> Result<()> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let looks_like_sse = trimmed.contains("event:")
        || trimmed.contains("\ndata:")
        || trimmed.starts_with("data:")
        || trimmed.contains("\"type\":\"response.created\"");
    if looks_like_sse {
        return process_responses_sse_text(body, fallback_model, tx, request_start);
    }

    let value: Value = serde_json::from_str(trimmed)?;
    if let Some(error) = value.get("error") {
        return Err(anyhow::anyhow!(
            "Responses API returned error payload: {}",
            error
        ));
    }

    let response_id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let response_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let created = value.get("created_at").and_then(Value::as_u64);
    let accumulated = extract_text_from_output(&value);
    let usage = extract_usage(&value, &accumulated);

    let done = make_done_response(
        response_id,
        response_model,
        created,
        &fallback_model,
        String::from("assistant"),
        accumulated,
        None,
        usage,
        request_start,
        None,
    );
    let _ = tx.send(Response::Done(done));
    Ok(())
}

fn process_responses_sse_text(
    body: String,
    fallback_model: String,
    tx: mpsc::UnboundedSender<Response>,
    request_start: Instant,
) -> Result<()> {
    let mut buffer = body.into_bytes();
    let mut accumulated = String::new();
    let mut usage: Option<Usage> = None;
    let mut response_id: Option<String> = None;
    let mut response_model: Option<String> = None;
    let mut created: Option<u64> = None;
    let mut tool_state: Vec<ResponseToolState> = Vec::new();
    let mut first_token_time: Option<Instant> = None;
    let role = String::from("assistant");

    while let Some(event_bytes) = extract_sse_event(&mut buffer) {
        let Some(data) = sse_data_from_event(&event_bytes) else {
            continue;
        };
        let trimmed = data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            continue;
        }

        let value: Value = serde_json::from_str(trimmed)?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match kind {
            "response.created" => {
                let response_value = value.get("response").unwrap_or(&value);
                response_id = response_value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                response_model = response_value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                created = response_value.get("created_at").and_then(Value::as_u64);
            }
            "response.output_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if delta.is_empty() {
                    continue;
                }
                accumulated.push_str(&delta);
                if first_token_time.is_none() {
                    first_token_time = Some(Instant::now());
                }
                let chunk = make_text_chunk(
                    response_id.clone(),
                    response_model.clone(),
                    created,
                    &fallback_model,
                    role.clone(),
                    delta,
                );
                if tx.send(Response::Chunk(chunk)).is_err() {
                    return Ok(());
                }
            }
            "response.output_text.done" => {
                if accumulated.is_empty() {
                    if let Some(text) = value.get("text").and_then(Value::as_str) {
                        accumulated.push_str(text);
                    }
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item_value) = value.get("item") {
                    ingest_function_call_item(item_value, &mut tool_state);
                }
            }
            "response.function_call_arguments.delta" => {
                let idx = tool_index_for_event(&value, &mut tool_state);
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    tool_state[idx].arguments.push_str(delta);
                }
            }
            "response.function_call_arguments.done" => {
                let idx = tool_index_for_event(&value, &mut tool_state);
                if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
                    tool_state[idx].arguments = arguments.to_string();
                }
            }
            "response.refusal.delta" | "response.refusal.done" => {}
            "error" | "response.error" => {
                return Err(anyhow::anyhow!("Responses stream error: {}", trimmed));
            }
            "response.completed" => {
                let response_value = value.get("response").unwrap_or(&value);
                response_id = response_value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or(response_id);
                response_model = response_value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or(response_model);
                created = response_value
                    .get("created_at")
                    .and_then(Value::as_u64)
                    .or(created);

                if accumulated.is_empty() {
                    accumulated = extract_text_from_output(response_value);
                }
                usage = Some(extract_usage(response_value, &accumulated));

                let done = make_done_response(
                    response_id,
                    response_model,
                    created,
                    &fallback_model,
                    role,
                    accumulated,
                    finalize_tools(&tool_state),
                    usage.unwrap_or_else(|| extract_usage(response_value, "")),
                    request_start,
                    first_token_time,
                );
                let _ = tx.send(Response::Done(done));
                return Ok(());
            }
            _ => {}
        }
    }

    let done = make_done_response(
        response_id,
        response_model,
        created,
        &fallback_model,
        role,
        accumulated.clone(),
        finalize_tools(&tool_state),
        usage.unwrap_or_else(|| usage_from_openai(None, &accumulated)),
        request_start,
        first_token_time,
    );
    let _ = tx.send(Response::Done(done));
    Ok(())
}

fn make_text_chunk(
    response_id: Option<String>,
    response_model: Option<String>,
    created: Option<u64>,
    fallback_model: &str,
    role: String,
    delta_text: String,
) -> ChatCompletionChunkResponse {
    ChatCompletionChunkResponse {
        id: response_id.unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4())),
        choices: vec![ChunkChoice {
            finish_reason: None,
            index: 0,
            delta: Delta {
                content: Some(delta_text),
                role,
                tool_calls: None,
                reasoning_content: None,
            },
            logprobs: None,
        }],
        created: created.unwrap_or_else(current_timestamp) as u128,
        model: response_model.unwrap_or_else(|| fallback_model.to_string()),
        system_fingerprint: String::new(),
        object: "chat.completion.chunk".to_string(),
        usage: None,
    }
}

fn make_done_response(
    response_id: Option<String>,
    response_model: Option<String>,
    created: Option<u64>,
    fallback_model: &str,
    role: String,
    content: String,
    tool_calls: Option<Vec<ToolCallResponse>>,
    mut usage: Usage,
    request_start: Instant,
    first_token_time: Option<Instant>,
) -> mistralrs::ChatCompletionResponse {
    if usage.completion_tokens == 0 && !content.is_empty() {
        let estimate = estimate_tokens(&content);
        usage.completion_tokens = estimate;
        usage.total_tokens = usage.prompt_tokens + estimate;
    }
    let elapsed = request_start.elapsed().as_secs_f32();
    if usage.avg_compl_tok_per_sec == 0.0 && usage.completion_tokens > 0 && elapsed > 0.0 {
        usage.avg_compl_tok_per_sec = usage.completion_tokens as f32 / elapsed.max(0.001);
        usage.total_completion_time_sec = elapsed;
        usage.total_time_sec = elapsed;
    }
    if usage.total_prompt_time_sec == 0.0 {
        if let Some(first) = first_token_time {
            usage.total_prompt_time_sec = first.duration_since(request_start).as_secs_f32();
        }
    }

    mistralrs::ChatCompletionResponse {
        id: response_id.unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4())),
        choices: vec![Choice {
            finish_reason: "stop".to_string(),
            index: 0,
            message: ResponseMessage {
                content: Some(content),
                role,
                tool_calls,
                reasoning_content: None,
            },
            logprobs: None,
        }],
        created: created.unwrap_or_else(current_timestamp),
        model: response_model.unwrap_or_else(|| fallback_model.to_string()),
        system_fingerprint: String::new(),
        object: "chat.completion".to_string(),
        usage,
    }
}

fn ingest_function_call_item(item: &Value, state: &mut Vec<ResponseToolState>) {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return;
    }

    let idx = item
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or_else(|| {
            state
                .iter()
                .position(|entry| {
                    entry.item_id.as_deref() == item.get("id").and_then(Value::as_str)
                })
                .unwrap_or(state.len())
        });
    if state.len() <= idx {
        state.resize_with(idx + 1, ResponseToolState::default);
    }
    state[idx].item_id = item
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state[idx].call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    if let Some(name) = item.get("name").and_then(Value::as_str) {
        state[idx].name = name.to_string();
    }
    if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
        state[idx].arguments = arguments.to_string();
    }
}

fn tool_index_for_event(event: &Value, state: &mut Vec<ResponseToolState>) -> usize {
    if let Some(item_id) = event.get("item_id").and_then(Value::as_str) {
        if let Some(idx) = state
            .iter()
            .position(|entry| entry.item_id.as_deref() == Some(item_id))
        {
            return idx;
        }
    }
    let idx = event
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(state.len());
    if state.len() <= idx {
        state.resize_with(idx + 1, ResponseToolState::default);
    }
    if state[idx].item_id.is_none() {
        state[idx].item_id = event
            .get("item_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }
    idx
}

fn finalize_tools(state: &[ResponseToolState]) -> Option<Vec<ToolCallResponse>> {
    let calls: Vec<ToolCallResponse> = state
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.name.is_empty() || !entry.arguments.is_empty())
        .map(|(idx, entry)| ToolCallResponse {
            index: idx,
            id: entry
                .call_id
                .clone()
                .or_else(|| entry.item_id.clone())
                .unwrap_or_else(|| format!("call-{}", idx)),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: entry.name.clone(),
                arguments: entry.arguments.clone(),
            },
        })
        .collect();
    if calls.is_empty() { None } else { Some(calls) }
}

fn extract_text_from_output(response: &Value) -> String {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

fn extract_usage(response: &Value, content: &str) -> Usage {
    let usage = response.get("usage");
    let completion_tokens = usage
        .and_then(|value| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let prompt_tokens = usage
        .and_then(|value| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let total_tokens = usage
        .and_then(|value| value.get("total_tokens"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .or_else(|| {
            completion_tokens
                .zip(prompt_tokens)
                .map(|(out, inp)| out + inp)
        });

    usage_from_openai(
        Some(OpenAiUsage {
            completion_tokens,
            prompt_tokens,
            total_tokens,
        }),
        content,
    )
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
        if i + 3 < len
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            let event = buffer[..i].to_vec();
            buffer.drain(..i + 4);
            return Some(event);
        }
        i += 1;
    }
    None
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

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
