use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use mistralrs::{
    ChatCompletionChunkResponse, ChatCompletionResponse, Choice, ChunkChoice, Delta, Response,
    ResponseMessage, Usage,
};
use mistralrs_core::{CalledFunction, ToolCallResponse, ToolCallType};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::openai_wire::{self, OpenAiStreamToolCall, OpenAiUsage};

pub use self::super::openai_wire::{current_timestamp, estimate_tokens};

#[derive(Clone, Default)]
pub struct StreamingToolCallState {
    pub id: Option<String>,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Copy, Debug)]
pub struct StreamMetrics {
    pub completion_tokens: usize,
    pub total_time_sec: f32,
    pub time_to_first_token_sec: f32,
}

impl StreamMetrics {
    pub fn from_timing(
        start: Instant,
        first_token: Option<Instant>,
        completion_tokens: usize,
    ) -> Self {
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

pub fn update_streaming_tool_calls(
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

pub fn finalize_stream_tool_calls(
    state: &[StreamingToolCallState],
) -> Option<Vec<ToolCallResponse>> {
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

pub fn send_final_done(
    tx: &tokio::sync::mpsc::UnboundedSender<Response>,
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

pub fn usage_from_openai(usage: Option<OpenAiUsage>, content: &str) -> Usage {
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

pub async fn process_sse_stream(
    response: reqwest::Response,
    fallback_model: String,
    tx: tokio::sync::mpsc::UnboundedSender<Response>,
    request_start: Instant,
    shared_usage: Arc<Mutex<Option<Usage>>>,
) -> Result<(), anyhow::Error> {
    use super::openai_wire::OpenAiStreamDelta;

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
    let mut latest_usage: Option<openai_wire::OpenAiUsage> = None;
    let mut first_token_time: Option<Instant> = None;
    let mut estimated_tokens: usize = 0;

    while let Some(item) = body_stream.next().await {
        if tx.is_closed() {
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

            let parsed: openai_wire::OpenAiStreamResponse = match serde_json::from_str(&data) {
                Ok(value) => value,
                Err(err) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(error) = value.get("error") {
                            return Err(anyhow::anyhow!("Remote error: {}", error));
                        }
                    }
                    return Err(anyhow::anyhow!("Failed to parse stream chunk: {}", err));
                }
            };

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
                        accumulated_content.push_str(&text);
                        if first_token_time.is_none() {
                            first_token_time = Some(Instant::now());
                        }
                        estimated_tokens += estimate_tokens(&text);
                        delta_content = Some(text);
                    }
                }

                let mut delta_tool_calls = Vec::new();
                if let Some(tool_call_vec) = tool_calls {
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
