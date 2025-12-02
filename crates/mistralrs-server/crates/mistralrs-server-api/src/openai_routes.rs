use std::convert::Infallible;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, sse::Sse, IntoResponse, Response},
    Json,
};
use mistralrs_server_core::{
    ChatRequest, EmbeddingRequest, EngineResponse, GenerateRequest, Usage,
};
use serde::Serialize;
use serde_json::json;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::{map_manager_error, record_success, AppState, RequestContext};

// =================================================================================================
// OpenAI Response Schemas
// =================================================================================================

#[derive(Serialize)]
pub struct OpenAIChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChatChoice>,
    pub usage: Usage,
}

#[derive(Serialize)]
pub struct OpenAIChatChoice {
    pub index: usize,
    pub message: OpenAIMessage,
    pub finish_reason: String,
}

#[derive(Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
pub struct OpenAICompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAICompletionChoice>,
    pub usage: Usage,
}

#[derive(Serialize)]
pub struct OpenAICompletionChoice {
    pub text: String,
    pub index: usize,
    pub logprobs: Option<serde_json::Value>,
    pub finish_reason: String,
}

#[derive(Serialize)]
pub struct OpenAIEmbeddingResponse {
    pub object: String,
    pub data: Vec<OpenAIEmbeddingData>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Serialize)]
pub struct OpenAIEmbeddingData {
    pub object: String,
    pub embedding: Vec<f32>,
    pub index: usize,
}

// =================================================================================================
// Helpers
// =================================================================================================

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn finish_reason_to_str(reason: &mistralrs_server_core::FinishReason) -> String {
    match reason {
        mistralrs_server_core::FinishReason::Stop => "stop".to_string(),
        mistralrs_server_core::FinishReason::Length => "length".to_string(),
        mistralrs_server_core::FinishReason::ToolCall => "tool_calls".to_string(),
        mistralrs_server_core::FinishReason::Error => "content_filter".to_string(), // closest mapping
    }
}

// =================================================================================================
// Handlers
// =================================================================================================

pub async fn handle_chat_completions(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    Json(payload): Json<ChatRequest>,
) -> Result<Response, Response> {
    let endpoint = "v1/chat/completions";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    let model_id = payload.model.clone();
    let request_id = format!("chatcmpl-{}", Uuid::new_v4());

    if payload.stream {
        let stream = manager
            .chat_stream(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err).into_response())?;

        let stream_response = stream.map(move |result| {
            match result {
                Ok(EngineResponse::Chunk(chunk)) => {
                    match serde_json::to_string(&chunk) {
                        Ok(json) => Ok::<Event, Infallible>(Event::default().data(json)),
                        Err(_) => Ok::<Event, Infallible>(Event::default().event("error").data("serialization_error")),
                    }
                }
                Ok(EngineResponse::Done(_)) => {
                    Ok::<Event, Infallible>(Event::default().data("[DONE]"))
                }
                Ok(EngineResponse::ModelError(msg, _)) => {
                     let err = json!({
                        "error": {
                            "message": msg,
                            "type": "model_error",
                            "param": null,
                            "code": null
                        }
                    });
                    Ok::<Event, Infallible>(Event::default().data(err.to_string()))
                }
                Ok(EngineResponse::InternalError(err)) => {
                     let err = json!({
                        "error": {
                            "message": err.to_string(),
                            "type": "server_error",
                            "param": null,
                            "code": null
                        }
                    });
                    Ok::<Event, Infallible>(Event::default().data(err.to_string()))
                }
                Ok(_) => Ok::<Event, Infallible>(Event::default()),
                Err(_) => Ok::<Event, Infallible>(Event::default().event("error").data("stream_error")),
            }
        });

        record_success(&state, endpoint, "POST", StatusCode::OK);
        
        let keep_alive = KeepAlive::new().interval(std::time::Duration::from_secs(10));
        return Ok(Sse::new(stream_response).keep_alive(keep_alive).into_response());
    }

    let resp = manager
        .chat(payload)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err).into_response())?;

    log.record_usage(&resp.usage);
    record_success(&state, endpoint, "POST", StatusCode::OK);

    let mut content = String::new();
    for part in resp.message.content {
        match part {
             mistralrs_server_core::ChatContent::Text { text } => content.push_str(&text),
             _ => {}
        }
    }

    let role = match resp.message.role {
        mistralrs_server_core::MessageRole::System => "system",
        mistralrs_server_core::MessageRole::User => "user",
        mistralrs_server_core::MessageRole::Assistant => "assistant",
        mistralrs_server_core::MessageRole::Tool => "tool",
    };

    let response = OpenAIChatCompletionResponse {
        id: request_id,
        object: "chat.completion".to_string(),
        created: current_timestamp(),
        model: model_id,
        choices: vec![OpenAIChatChoice {
            index: 0,
            message: OpenAIMessage {
                role: role.to_string(),
                content,
                tool_calls: None,
            },
            finish_reason: resp.finish_reason,
        }],
        usage: resp.usage,
    };

    Ok(Json(response).into_response())
}

pub async fn handle_completions(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    Json(payload): Json<GenerateRequest>,
) -> Result<Response, Response> {
    let endpoint = "v1/completions";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    let model_id = payload.model.clone();
    let request_id = format!("cmpl-{}", Uuid::new_v4());

    if payload.stream {
         let stream = manager
            .generate_stream(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err).into_response())?;

        let stream_response = stream.map(move |result| {
            match result {
                Ok(EngineResponse::CompletionChunk(chunk)) => {
                    match serde_json::to_string(&chunk) {
                        Ok(json) => Ok::<Event, Infallible>(Event::default().data(json)),
                        Err(_) => Ok::<Event, Infallible>(Event::default().event("error").data("serialization_error")),
                    }
                }
                Ok(EngineResponse::CompletionDone(_)) => {
                    Ok::<Event, Infallible>(Event::default().data("[DONE]"))
                }
                 Ok(EngineResponse::CompletionModelError(msg, _)) => {
                     let err = json!({
                        "error": {
                            "message": msg,
                            "type": "model_error",
                            "param": null,
                            "code": null
                        }
                    });
                    Ok::<Event, Infallible>(Event::default().data(err.to_string()))
                }
                 Ok(_) => Ok::<Event, Infallible>(Event::default()),
                 Err(_) => Ok::<Event, Infallible>(Event::default().event("error").data("stream_error")),
            }
        });
        record_success(&state, endpoint, "POST", StatusCode::OK);
        let keep_alive = KeepAlive::new().interval(std::time::Duration::from_secs(10));
        return Ok(Sse::new(stream_response).keep_alive(keep_alive).into_response());
    }

    let resp = manager
        .generate(payload)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err).into_response())?;

    log.record_usage(&resp.usage);
    record_success(&state, endpoint, "POST", StatusCode::OK);

    let response = OpenAICompletionResponse {
        id: request_id,
        object: "text_completion".to_string(),
        created: current_timestamp(),
        model: model_id,
        choices: vec![OpenAICompletionChoice {
            text: resp.output,
            index: 0,
            logprobs: None,
            finish_reason: finish_reason_to_str(&resp.finish_reason),
        }],
        usage: resp.usage,
    };

    Ok(Json(response).into_response())
}

pub async fn handle_embeddings_openai(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    Json(payload): Json<EmbeddingRequest>,
) -> Result<Response, Response> {
    let endpoint = "v1/embeddings";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    let model_id = payload.model.clone();

    let resp = manager
        .embeddings(payload)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err).into_response())?;

    log.record_usage(&resp.usage);
    record_success(&state, endpoint, "POST", StatusCode::OK);

    let data = resp.embeddings.into_iter().enumerate().map(|(i, vec)| OpenAIEmbeddingData {
        object: "embedding".to_string(),
        embedding: vec,
        index: i,
    }).collect();

    let response = OpenAIEmbeddingResponse {
        object: "list".to_string(),
        data,
        model: model_id,
        usage: resp.usage,
    };

    Ok(Json(response).into_response())
}