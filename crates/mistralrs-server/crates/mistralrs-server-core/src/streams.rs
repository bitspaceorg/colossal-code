use std::{pin::Pin, sync::Arc, task::{Context, Poll}};

use axum::response::sse::Event;

use futures::Stream;
use mistralrs_core::{
    ChatCompletionResponse as EngineChatResponse,
    CompletionResponse as EngineCompletionResponse,
    Response as EngineResponse,
    Usage as EngineUsage,
};
use serde_json::json;
use tokio::sync::mpsc::Receiver;

use crate::{ActiveRequestGuard, Clock, ModelMetrics, StructuredLog, SystemClock, Usage};

#[derive(Clone, Copy)]
pub enum StreamKind {
    Completion,
    Chat,
}

pub struct StreamInstrumentation {
    pub model: String,
    pub metrics: Arc<ModelMetrics>,
    pub log_sink: Option<Arc<dyn Fn(StructuredLog) + Send + Sync>>,
    pub clock: SystemClock,
    pub kind: StreamKind,
}

impl StreamKind {
    fn log_name(&self) -> &'static str {
        match self {
            StreamKind::Completion => "generate_stream",
            StreamKind::Chat => "chat_stream",
        }
    }
}

impl StreamInstrumentation {
    fn record_completion(&self, response: &EngineCompletionResponse) {
        self.metrics.add_tokens(
            &self.model,
            clamp_tokens(response.usage.prompt_tokens),
            clamp_tokens(response.usage.completion_tokens),
        );
        if let Some(sink) = &self.log_sink {
            let usage = usage_from_engine(&response.usage);
            let finish_reason = response
                .choices
                .get(0)
                .map(|choice| choice.finish_reason.clone())
                .unwrap_or_else(|| "stop".to_string());
            sink(StructuredLog {
                timestamp: self.clock.now_system(),
                level: "info".into(),
                message: self.kind.log_name().into(),
                fields: json!({
                    "finish_reason": finish_reason,
                    "usage": usage,
                }),
            });
        }
    }

    fn record_chat(&self, response: &EngineChatResponse) {
        self.metrics.add_tokens(
            &self.model,
            clamp_tokens(response.usage.prompt_tokens),
            clamp_tokens(response.usage.completion_tokens),
        );
        if let Some(sink) = &self.log_sink {
            let usage = usage_from_engine(&response.usage);
            sink(StructuredLog {
                timestamp: self.clock.now_system(),
                level: "info".into(),
                message: self.kind.log_name().into(),
                fields: json!({"usage": usage}),
            });
        }
    }

    fn log_error(&self, message: String) {
        if let Some(sink) = &self.log_sink {
            sink(StructuredLog {
                timestamp: self.clock.now_system(),
                level: "error".into(),
                message: self.kind.log_name().into(),
                fields: json!({"error": message}),
            });
        }
    }
}

pub struct CompletionStreamWrapper {
    request_id: String,
    rx: Receiver<EngineResponse>,
    guard: Option<ActiveRequestGuard>,
    instrumentation: Option<StreamInstrumentation>,
}

impl CompletionStreamWrapper {
    pub(crate) fn new(
        request_id: String,
        rx: Receiver<EngineResponse>,
        guard: Option<ActiveRequestGuard>,
        instrumentation: Option<StreamInstrumentation>,
    ) -> Self {
        Self {
            request_id,
            rx,
            guard,
            instrumentation,
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl Stream for CompletionStreamWrapper {
    type Item = Result<EngineResponse, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut_self = self.get_mut();
        match Pin::new(&mut mut_self.rx).poll_recv(cx) {
            Poll::Ready(Some(response)) => {
                match &response {
                    EngineResponse::CompletionChunk(chunk) => {
                        // Count streaming tokens from chunk content
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            let token_count: u32 = chunk.choices.iter()
                                .map(|choice| {
                                    // Rough estimate: 1 token per ~4 characters
                                    (choice.text.len() / 4).max(1) as u32
                                })
                                .sum();
                            if token_count > 0 {
                                instr.metrics.add_stream_tokens(&instr.model, token_count);
                            }
                        }
                    }
                    EngineResponse::CompletionDone(done) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.record_completion(done);
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::CompletionModelError(msg, _) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(msg.clone());
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::InternalError(err) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(err.to_string());
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::ValidationError(err) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(err.to_string());
                        }
                        mut_self.guard.take();
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(response)))
            }
            Poll::Ready(None) => {
                mut_self.guard.take();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct ChatStreamWrapper {
    request_id: String,
    rx: Receiver<EngineResponse>,
    guard: Option<ActiveRequestGuard>,
    instrumentation: Option<StreamInstrumentation>,
}

impl ChatStreamWrapper {
    pub(crate) fn new(
        request_id: String,
        rx: Receiver<EngineResponse>,
        guard: Option<ActiveRequestGuard>,
        instrumentation: Option<StreamInstrumentation>,
    ) -> Self {
        Self {
            request_id,
            rx,
            guard,
            instrumentation,
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl Stream for ChatStreamWrapper {
    type Item = Result<EngineResponse, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut_self = self.get_mut();
        match Pin::new(&mut mut_self.rx).poll_recv(cx) {
            Poll::Ready(Some(response)) => {
                match &response {
                    EngineResponse::Chunk(chunk) => {
                        // Count streaming tokens from chunk content
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            let token_count: u32 = chunk.choices.iter()
                                .map(|choice| {
                                    // Rough estimate: 1 token per ~4 characters
                                    choice.delta.content.as_ref()
                                        .map(|text| (text.len() / 4).max(1) as u32)
                                        .unwrap_or(0)
                                })
                                .sum();
                            if token_count > 0 {
                                instr.metrics.add_stream_tokens(&instr.model, token_count);
                            }
                        }
                    }
                    EngineResponse::Done(done) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.record_chat(done);
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::ModelError(msg, _) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(msg.clone());
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::InternalError(err) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(err.to_string());
                        }
                        mut_self.guard.take();
                    }
                    EngineResponse::ValidationError(err) => {
                        if let Some(instr) = mut_self.instrumentation.as_ref() {
                            instr.log_error(err.to_string());
                        }
                        mut_self.guard.take();
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(response)))
            }
            Poll::Ready(None) => {
                mut_self.guard.take();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct EmbeddingStreamWrapper {
    request_id: String,
    rx: Receiver<Event>,
}

impl EmbeddingStreamWrapper {
    pub fn new(request_id: String, rx: Receiver<Event>) -> Self {
        Self { request_id, rx }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl Stream for EmbeddingStreamWrapper {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.get_mut().rx).poll_recv(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn usage_from_engine(usage: &EngineUsage) -> Usage {
    Usage {
        prompt_tokens: clamp_tokens(usage.prompt_tokens),
        completion_tokens: clamp_tokens(usage.completion_tokens),
        total_tokens: clamp_tokens(usage.total_tokens),
    }
}

fn clamp_tokens(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use mistralrs_core::{CompletionChunkChoice, CompletionChunkResponse};
    use tokio::sync::mpsc;
    use futures::StreamExt;

    #[tokio::test]
    async fn completion_stream_wraps_chunks() {
        let (tx, rx) = mpsc::channel(2);
        let mut stream = CompletionStreamWrapper::new("req".into(), rx, None, None);
        tx.send(EngineResponse::CompletionChunk(CompletionChunkResponse {
            id: "1".into(),
            choices: vec![CompletionChunkChoice {
                text: "hi".into(),
                index: 0,
                logprobs: None,
                finish_reason: Some("stop".into()),
            }],
            created: 0,
            model: "demo".into(),
            system_fingerprint: String::new(),
            object: "".into(),
        }))
        .await
        .unwrap();
        let event = stream.next().await.unwrap().unwrap();
        match event {
            EngineResponse::CompletionChunk(chunk) => {
                assert_eq!(chunk.id, "1");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
