use std::{
    collections::HashSet,
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    extract::{Extension, Path, Query, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware,
    response::{sse::Event, sse::KeepAlive, sse::Sse, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use dashmap::DashMap;
use mistralrs_server_config::{AuthSection, ConfigManager, ServerConfig};
use mistralrs_server_core::{
    ActiveModel, ChatCompletionChunkResponse, ChatRequest, ChatStreamWrapper,
    CompletionChunkResponse, CompletionStreamWrapper, DynModelManager, EmbeddingRequest,
    EmbeddingResponse, EngineResponse, EngineUsage, GenerateRequest,
    LoadModelRequest, ModelManagerError, ModelMetadata, ModelMetrics, ModelRecord,
    ModelScheduler, StructuredLog, Usage,
};
use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tokio_stream::{Stream, StreamExt};
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tracing::instrument;
use uuid::Uuid;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ManagerFactory = Arc<
    dyn Fn(
            &ServerConfig,
            Arc<dyn ModelScheduler>,
            &HttpMetrics,
        ) -> BoxFuture<'static, Result<DynModelManager>>
        + Send
        + Sync,
>;
pub type SchedulerFactory = Arc<dyn Fn(&ServerConfig) -> Arc<dyn ModelScheduler> + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<RwLock<DynModelManager>>,
    pub factory: ManagerFactory,
    pub scheduler_factory: SchedulerFactory,
    pub config: ConfigManager,
    pub scheduler: Arc<RwLock<Arc<dyn ModelScheduler>>>,
    pub model_metrics: Arc<ModelMetrics>,
    pub metrics: HttpMetrics,
    pub auth: AuthState,
}

#[derive(Clone)]
pub struct HttpMetrics {
    registry: Registry,
    requests: IntCounterVec,
    model_metrics: Arc<ModelMetrics>,
}

impl HttpMetrics {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["endpoint", "method", "status"],
        )?;
        registry.register(Box::new(requests.clone()))?;
        let model_metrics = Arc::new(ModelMetrics::register(&registry)?);
        Ok(Self {
            registry,
            requests,
            model_metrics,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn model_metrics(&self) -> Arc<ModelMetrics> {
        Arc::clone(&self.model_metrics)
    }

    pub fn record(&self, endpoint: &str, method: &str, status: u16) {
        let labels = [endpoint, method, &status.to_string()];
        if let Ok(metric) = self.requests.get_metric_with_label_values(&labels) {
            metric.inc();
        }
    }

    pub fn render(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&families, &mut buffer)?;
        Ok(String::from_utf8(buffer)?)
    }
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T>
where
    T: Serialize,
{
    pub data: Option<T>,
    pub meta: Meta,
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct Meta {
    pub request_id: String,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    headers: Vec<(header::HeaderName, header::HeaderValue)>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            headers: Vec::new(),
        }
    }

    fn status(&self) -> StatusCode {
        self.status
    }

    fn with_header(mut self, name: header::HeaderName, value: header::HeaderValue) -> Self {
        self.headers.push((name, value));
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let request_id = Uuid::new_v4().to_string();
        let body = Json(ApiResponse::<serde_json::Value> {
            data: None,
            meta: Meta {
                request_id: request_id.clone(),
            },
            error: Some(ApiErrorBody {
                code: self.code.into(),
                message: self.message.clone(),
            }),
        });
        let mut response = (self.status, body).into_response();
        response.headers_mut().insert(
            "x-request-id",
            header::HeaderValue::from_str(&request_id).unwrap(),
        );
        for (name, value) in self.headers {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

impl From<ModelManagerError> for ApiError {
    fn from(value: ModelManagerError) -> Self {
        match value {
            ModelManagerError::NotFound(model) => {
                ApiError::new(StatusCode::NOT_FOUND, "not_found", model)
            }
            ModelManagerError::MaxParallel(model) => ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "concurrency",
                format!("busy: {model}"),
            ),
            ModelManagerError::Scheduler(msg) => {
                ApiError::new(StatusCode::BAD_REQUEST, "scheduler", msg)
            }
            ModelManagerError::ServiceUnavailable => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "global request limit reached",
            ),
            ModelManagerError::Other(msg) => {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "error", msg)
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct AuthContext {
    pub api_key: Option<String>,
}

#[derive(Clone)]
pub struct AuthState {
    enabled: bool,
    keys: HashSet<String>,
    limiter: Option<RateLimiter>,
}

impl AuthState {
    pub fn from_section(section: &AuthSection) -> Self {
        Self {
            enabled: section.enabled,
            keys: section.api_keys.iter().cloned().collect(),
            limiter: section
                .rate_limit
                .as_ref()
                .map(|cfg| RateLimiter::new(cfg.requests_per_minute)),
        }
    }

    pub fn validate(&self, headers: &HeaderMap) -> Result<AuthContext, ApiError> {
        if !self.enabled {
            return Ok(AuthContext::default());
        }
        let key = headers
            .get("x-api-key")
            .or_else(|| headers.get(header::AUTHORIZATION))
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim_start_matches("Bearer ").to_string())
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "auth", "missing api key"))?;
        if !self.keys.contains(&key) {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "auth",
                "invalid api key",
            ));
        }
        if let Some(limiter) = &self.limiter {
            let decision = limiter.check(&key);
            if decision.throttled {
                let mut err = ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit",
                    "too many requests",
                )
                .with_header(
                    header::HeaderName::from_static("x-ratelimit-limit"),
                    header::HeaderValue::from_str(&decision.limit.to_string())
                        .unwrap_or_else(|_| header::HeaderValue::from_static("0")),
                )
                .with_header(
                    header::HeaderName::from_static("x-ratelimit-remaining"),
                    header::HeaderValue::from_str(&decision.remaining.to_string())
                        .unwrap_or_else(|_| header::HeaderValue::from_static("0")),
                );
                if let Some(retry) = decision.retry_after_secs {
                    err = err.with_header(
                        header::HeaderName::from_static("retry-after"),
                        header::HeaderValue::from_str(&retry.to_string())
                            .unwrap_or_else(|_| header::HeaderValue::from_static("1")),
                    );
                }
                return Err(err);
            }
        }
        Ok(AuthContext { api_key: Some(key) })
    }
}

#[derive(Clone, Default)]
pub struct RequestContext(Arc<Mutex<RequestLogData>>);

#[derive(Clone, Default)]
struct RequestLogData {
    endpoint: Option<String>,
    model_id: Option<String>,
    tokens_in: Option<u32>,
    tokens_out: Option<u32>,
}

impl RequestContext {
    pub fn set_endpoint(&self, endpoint: impl Into<String>) {
        if let Ok(mut guard) = self.0.lock() {
            guard.endpoint = Some(endpoint.into());
        }
    }

    pub fn set_model(&self, model: impl Into<String>) {
        if let Ok(mut guard) = self.0.lock() {
            guard.model_id = Some(model.into());
        }
    }

    pub fn record_usage(&self, usage: &Usage) {
        if let Ok(mut guard) = self.0.lock() {
            guard.tokens_in = Some(usage.prompt_tokens);
            guard.tokens_out = Some(usage.completion_tokens);
        }
    }

    fn snapshot(&self) -> RequestLogData {
        self.0.lock().map(|d| d.clone()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct RateLimiter {
    limit: u32,
    buckets: Arc<DashMap<String, RateBucket>>,
}

#[derive(Clone)]
struct RateBucket {
    count: u32,
    started_at: Instant,
}

impl RateLimiter {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            buckets: Arc::new(DashMap::new()),
        }
    }

    fn check(&self, key: &str) -> RateLimitDecision {
        let now = Instant::now();
        let mut entry = self.buckets.entry(key.to_string()).or_insert(RateBucket {
            count: 0,
            started_at: now,
        });
        if now.duration_since(entry.started_at) >= std::time::Duration::from_secs(60) {
            entry.count = 0;
            entry.started_at = now;
        }
        if entry.count >= self.limit {
            let elapsed = now
                .checked_duration_since(entry.started_at)
                .unwrap_or_default()
                .as_secs();
            let retry_after = (60i64 - elapsed as i64).max(1) as u64;
            return RateLimitDecision {
                limit: self.limit,
                remaining: 0,
                retry_after_secs: Some(retry_after),
                throttled: true,
            };
        }
        entry.count += 1;
        RateLimitDecision {
            limit: self.limit,
            remaining: self.limit.saturating_sub(entry.count),
            retry_after_secs: None,
            throttled: false,
        }
    }
}

struct RateLimitDecision {
    limit: u32,
    remaining: u32,
    retry_after_secs: Option<u64>,
    throttled: bool,
}

pub struct AuthRejection(ApiError);

impl IntoResponse for AuthRejection {
    fn into_response(self) -> axum::response::Response {
        self.0.into_response()
    }
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<axum::body::Body>,
    next: middleware::Next,
) -> Result<axum::response::Response, AuthRejection> {
    match state.auth.validate(request.headers()) {
        Ok(ctx) => {
            request.extensions_mut().insert(ctx);
            Ok(next.run(request).await)
        }
        Err(err) => Err(AuthRejection(err)),
    }
}

async fn log_requests(
    mut request: Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let start = Instant::now();
    let context = RequestContext::default();
    request.extensions_mut().insert(context.clone());
    let response = next.run(request).await;
    let elapsed = start.elapsed().as_millis() as u128;
    let snapshot = context.snapshot();
    let endpoint_name = snapshot.endpoint.as_deref().unwrap_or(path.as_str());
    let model_name = snapshot.model_id.as_deref().unwrap_or("-");
    tracing::info!(
        target = "model.request",
        request_id = request_id.as_deref().unwrap_or("-"),
        endpoint = endpoint_name,
        model_id = model_name,
        method = %method,
        latency_ms = elapsed,
        tokens_in = snapshot.tokens_in.unwrap_or(0),
        tokens_out = snapshot.tokens_out.unwrap_or(0),
        status = %response.status()
    );
    response
}

mod openai_routes;

pub async fn build_router(state: AppState) -> Result<Router> {
    let cors = CorsLayer::new().allow_origin(Any);
    let make_request_id = MakeRequestUuid::default();
    let request_header = header::HeaderName::from_static("x-request-id");
    let router = Router::new()
        .route("/api/generate", post(handle_generate))
        .route("/api/chat", post(handle_chat))
        .route("/api/embeddings", post(handle_embeddings))
        .route("/api/tags", get(handle_tags))
        .route("/api/show/:model", get(handle_show))
        .route("/api/ps", get(handle_ps))
        .route("/api/pull", post(handle_pull))
        .route("/api/delete", delete(handle_delete))
        .route("/api/load", post(handle_load))
        .route("/api/unload", post(handle_unload))
        .route("/api/jobs/:id", get(handle_job_status))
        .route("/api/logs/:model", get(handle_logs))
        .route("/v1/chat/completions", post(openai_routes::handle_chat_completions))
        .route("/v1/completions", post(openai_routes::handle_completions))
        .route("/v1/embeddings", post(openai_routes::handle_embeddings_openai))
        .route("/healthz", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .route("/admin/reload-config", post(handle_reload_config))
        .route("/admin/evict", post(handle_admin_evict))
        .with_state(state.clone());
    let router = router
        .layer(ServiceBuilder::new().layer(cors))
        .layer(SetRequestIdLayer::x_request_id(make_request_id))
        .layer(PropagateRequestIdLayer::new(request_header))
        .layer(middleware::from_fn(log_requests))
        .route_layer(middleware::from_fn_with_state(state, auth_middleware));
    Ok(router)
}

fn envelope<T: Serialize>(data: Option<T>, request_id: String) -> Json<ApiResponse<T>> {
    Json(ApiResponse {
        data,
        meta: Meta { request_id },
        error: None,
    })
}

fn record_success(state: &AppState, endpoint: &str, method: &str, status: StatusCode) {
    state.metrics.record(endpoint, method, status.as_u16());
}

fn sse_response<S>(stream: S, request_id: &str) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let keep_alive = KeepAlive::new().interval(Duration::from_secs(10));
    let mut response = Sse::new(stream).keep_alive(keep_alive).into_response();
    let _ = response.headers_mut().insert(
        header::HeaderName::from_static("x-request-id"),
        header::HeaderValue::from_str(request_id)
            .unwrap_or_else(|_| header::HeaderValue::from_static("unknown")),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("connection"),
        header::HeaderValue::from_static("keep-alive"),
    );
    response
}

#[derive(Serialize)]
struct SseEnvelope<'a, T> {
    data: Option<T>,
    meta: SseMeta<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiErrorBody>,
}

#[derive(Serialize)]
struct SseMeta<'a> {
    request_id: &'a str,
}

#[derive(Serialize)]
struct TokenPayload {
    token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

#[derive(Serialize)]
struct UsageEventPayload {
    usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

fn convert_usage(usage: &EngineUsage) -> Usage {
    Usage {
        prompt_tokens: usage.prompt_tokens.min(u32::MAX as usize) as u32,
        completion_tokens: usage.completion_tokens.min(u32::MAX as usize) as u32,
        total_tokens: usage.total_tokens.min(u32::MAX as usize) as u32,
    }
}

fn sse_event<T: Serialize>(
    event_name: &str,
    request_id: &str,
    data: Option<T>,
    error: Option<ApiErrorBody>,
) -> Event {
    let envelope = SseEnvelope {
        data,
        meta: SseMeta { request_id },
        error,
    };
    let payload = serde_json::to_string(&envelope).unwrap_or_else(|err| {
        let fallback = SseEnvelope {
            data: Option::<TokenPayload>::None,
            meta: SseMeta { request_id },
            error: Some(ApiErrorBody {
                code: "serialization_error".into(),
                message: err.to_string(),
            }),
        };
        serde_json::to_string(&fallback).unwrap_or_else(|_| "{}".into())
    });
    Event::default()
        .id(request_id.to_string())
        .event(event_name.to_string())
        .data(payload)
}

fn stream_error_event(request_id: &str, code: &'static str, message: String) -> Event {
    sse_event(
        "error",
        request_id,
        Option::<TokenPayload>::None,
        Some(ApiErrorBody {
            code: code.into(),
            message,
        }),
    )
}

fn completion_stream_events(
    stream: CompletionStreamWrapper,
    log: RequestContext,
) -> (String, impl Stream<Item = Result<Event, Infallible>>) {
    let request_id = stream.request_id().to_string();
    let rid = request_id.clone();
    let log_handle = log.clone();
    let mapped = stream.map(move |item| {
        let response = item.expect("infallible completion stream");
        let rid_local = rid.clone();
        let log = log_handle.clone();
        let event = match response {
            EngineResponse::CompletionChunk(chunk) => {
                sse_event("token", &rid_local, Some(completion_token_payload(chunk)), None)
            }
            EngineResponse::CompletionDone(done) => {
                let usage = convert_usage(&done.usage);
                log.record_usage(&usage);
                let finish_reason = done
                    .choices
                    .first()
                    .map(|choice| choice.finish_reason.clone());
                sse_event(
                    "usage",
                    &rid_local,
                    Some(UsageEventPayload { usage, finish_reason }),
                    None,
                )
            }
            EngineResponse::CompletionModelError(msg, _) => {
                stream_error_event(&rid_local, "model_error", msg)
            }
            EngineResponse::InternalError(err) => {
                stream_error_event(&rid_local, "internal_error", err.to_string())
            }
            EngineResponse::ValidationError(err) => {
                stream_error_event(&rid_local, "validation_error", err.to_string())
            }
            _other => stream_error_event(
                &rid_local,
                "stream_mismatch",
                format!("unexpected response variant"),
            ),
        };
        Ok(event)
    });
    (request_id, mapped)
}

fn chat_stream_events(
    stream: ChatStreamWrapper,
    log: RequestContext,
) -> (String, impl Stream<Item = Result<Event, Infallible>>) {
    let request_id = stream.request_id().to_string();
    let rid = request_id.clone();
    let log_handle = log.clone();
    let mapped = stream.map(move |item| {
        let response = item.expect("infallible chat stream");
        let rid_local = rid.clone();
        let log = log_handle.clone();
        let event = match response {
            EngineResponse::Chunk(chunk) => {
                sse_event("token", &rid_local, Some(chat_token_payload(chunk)), None)
            }
            EngineResponse::Done(done) => {
                let usage = convert_usage(&done.usage);
                log.record_usage(&usage);
                let finish_reason = done
                    .choices
                    .first()
                    .map(|choice| choice.finish_reason.clone());
                sse_event(
                    "usage",
                    &rid_local,
                    Some(UsageEventPayload { usage, finish_reason }),
                    None,
                )
            }
            EngineResponse::ModelError(msg, _) => {
                stream_error_event(&rid_local, "model_error", msg)
            }
            EngineResponse::InternalError(err) => {
                stream_error_event(&rid_local, "internal_error", err.to_string())
            }
            EngineResponse::ValidationError(err) => {
                stream_error_event(&rid_local, "validation_error", err.to_string())
            }
            _ => stream_error_event(
                &rid_local,
                "stream_mismatch",
                format!("unexpected response variant"),
            ),
        };
        Ok(event)
    });
    (request_id, mapped)
}

fn completion_token_payload(chunk: CompletionChunkResponse) -> TokenPayload {
    let token = chunk
        .choices
        .iter()
        .map(|choice| choice.text.clone())
        .collect::<String>();
    let index = chunk.choices.first().map(|choice| choice.index);
    let finish_reason = chunk
        .choices
        .iter()
        .find_map(|choice| choice.finish_reason.clone());
    TokenPayload {
        token,
        role: None,
        index,
        finish_reason,
    }
}

fn chat_token_payload(chunk: ChatCompletionChunkResponse) -> TokenPayload {
    let mut token = String::new();
    let mut role = None;
    let mut index = None;
    let mut finish_reason = None;
    for choice in &chunk.choices {
        if let Some(content) = &choice.delta.content {
            token.push_str(content);
        } else if let Some(tool_calls) = &choice.delta.tool_calls {
            if let Ok(serialized) = serde_json::to_string(tool_calls) {
                token.push_str(&serialized);
            }
        }
        if role.is_none() {
            role = Some(choice.delta.role.clone());
        }
        if index.is_none() {
            index = Some(choice.index);
        }
        if finish_reason.is_none() {
            finish_reason = choice.finish_reason.clone();
        }
    }
    TokenPayload {
        token,
        role,
        index,
        finish_reason,
    }
}

fn map_manager_error(
    state: &AppState,
    endpoint: &str,
    method: &str,
    err: ModelManagerError,
) -> ApiError {
    let api_error: ApiError = err.into();
    state
        .metrics
        .record(endpoint, method, api_error.status().as_u16());
    api_error
}

#[instrument(skip(state, log, stream_override, payload))]
async fn handle_generate(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    stream_override: Option<Query<StreamQueryParam>>,
    Json(mut payload): Json<GenerateRequest>,
) -> Result<Response, ApiError> {
    let endpoint = "generate";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    if let Some(Query(params)) = stream_override {
        if let Some(toggle) = params.stream {
            payload.stream = toggle;
        }
    }
    if payload.stream {
        let stream = manager
            .generate_stream(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
        let (request_id, events) = completion_stream_events(stream, log.clone());
        record_success(&state, endpoint, "POST", StatusCode::OK);
        Ok(sse_response(events, &request_id))
    } else {
        let resp = manager
            .generate(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
        log.record_usage(&resp.usage);
        let request_id = Uuid::new_v4().to_string();
        record_success(&state, endpoint, "POST", StatusCode::OK);
        Ok(envelope(Some(resp), request_id).into_response())
    }
}

#[instrument(skip(state, log, stream_override, payload))]
async fn handle_chat(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    stream_override: Option<Query<StreamQueryParam>>,
    Json(mut payload): Json<ChatRequest>,
) -> Result<Response, ApiError> {
    let endpoint = "chat";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    if let Some(Query(params)) = stream_override {
        if let Some(toggle) = params.stream {
            payload.stream = toggle;
        }
    }
    if payload.stream {
        let stream = manager
            .chat_stream(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
        let (request_id, events) = chat_stream_events(stream, log.clone());
        record_success(&state, endpoint, "POST", StatusCode::OK);
        Ok(sse_response(events, &request_id))
    } else {
        let resp = manager
            .chat(payload)
            .await
            .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
        log.record_usage(&resp.usage);
        let request_id = Uuid::new_v4().to_string();
        record_success(&state, endpoint, "POST", StatusCode::OK);
        Ok(envelope(Some(resp), request_id).into_response())
    }
}

#[instrument(skip(state, log, payload))]
async fn handle_embeddings(
    State(state): State<AppState>,
    Extension(log): Extension<RequestContext>,
    Json(payload): Json<EmbeddingRequest>,
) -> Result<Json<ApiResponse<EmbeddingResponse>>, ApiError> {
    let endpoint = "embeddings";
    let manager = state.manager.read().await.clone();
    log.set_endpoint(endpoint);
    log.set_model(payload.model.clone());
    let resp = manager
        .embeddings(payload)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
    log.record_usage(&resp.usage);
    let request_id = Uuid::new_v4().to_string();
    record_success(&state, endpoint, "POST", StatusCode::OK);
    Ok(envelope(Some(resp), request_id))
}

#[derive(Debug, Deserialize)]
struct PaginationQuery {
    #[serde(default)]
    page: usize,
    #[serde(default = "default_page_size")]
    page_size: usize,
}

fn default_page_size() -> usize {
    100
}

async fn handle_tags(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ApiResponse<Vec<ModelRecord>>>, ApiError> {
    let endpoint = "tags";
    let manager = state.manager.read().await.clone();
    let offset = pagination.page * pagination.page_size;
    let resp = manager
        .list_models(pagination.page_size, offset)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    let request_id = Uuid::new_v4().to_string();
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(resp), request_id))
}

async fn handle_show(
    State(state): State<AppState>,
    Path(model): Path<String>,
) -> Result<Json<ApiResponse<ModelMetadata>>, ApiError> {
    let endpoint = "show";
    let manager = state.manager.read().await.clone();
    let models = manager
        .list_models(usize::MAX, 0)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    let metadata = match models
        .into_iter()
        .find(|record| record.metadata.name == model)
        .map(|record| record.metadata)
    {
        Some(metadata) => metadata,
        None => {
            let err = ApiError::new(StatusCode::NOT_FOUND, "not_found", "model not found");
            state
                .metrics
                .record(endpoint, "GET", err.status().as_u16());
            return Err(err);
        }
    };
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(metadata), Uuid::new_v4().to_string()))
}

async fn handle_ps(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ActiveModel>>>, ApiError> {
    let endpoint = "ps";
    let manager = state.manager.read().await.clone();
    let resp = manager
        .active_models()
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(resp), Uuid::new_v4().to_string()))
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    model: String,
    source: String,
}

#[derive(Debug, Default, Deserialize)]
struct StreamQueryParam {
    #[serde(default)]
    stream: Option<bool>,
}

async fn handle_pull(
    State(state): State<AppState>,
    Json(payload): Json<PullRequest>,
) -> Result<Json<ApiResponse<Uuid>>, ApiError> {
    let endpoint = "pull";
    let manager = state.manager.read().await.clone();
    let job = manager
        .submit_pull_job(&payload.model, &payload.source)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
    record_success(&state, endpoint, "POST", StatusCode::OK);
    Ok(envelope(Some(job), Uuid::new_v4().to_string()))
}

#[derive(Debug, Deserialize)]
struct DeleteRequest {
    models: Vec<String>,
}

async fn handle_delete(
    State(state): State<AppState>,
    Json(payload): Json<DeleteRequest>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let manager = state.manager.read().await.clone();
    for model in &payload.models {
        let _ = manager.unload_model(model).await;
    }
    record_success(&state, "delete", "DELETE", StatusCode::OK);
    Ok(envelope(Some(payload.models), Uuid::new_v4().to_string()))
}

#[derive(Debug, Deserialize)]
struct LoadRequest {
    model: String,
    keep_alive: Option<u64>,
    pinned: Option<bool>,
}

async fn handle_load(
    State(state): State<AppState>,
    Json(payload): Json<LoadRequest>,
) -> Result<Json<ApiResponse<ModelMetadata>>, ApiError> {
    let endpoint = "load";
    let manager = state.manager.read().await.clone();
    let metadata = manager
        .load_model(LoadModelRequest {
            model: payload.model,
            keep_alive: payload.keep_alive.map(std::time::Duration::from_secs),
            pinned: payload.pinned.unwrap_or(false),
        })
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
    record_success(&state, endpoint, "POST", StatusCode::OK);
    Ok(envelope(Some(metadata), Uuid::new_v4().to_string()))
}

#[derive(Debug, Deserialize)]
struct UnloadRequest {
    model: String,
}

async fn handle_unload(
    State(state): State<AppState>,
    Json(payload): Json<UnloadRequest>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let endpoint = "unload";
    let manager = state.manager.read().await.clone();
    manager
        .unload_model(&payload.model)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "POST", err))?;
    record_success(&state, endpoint, "POST", StatusCode::OK);
    Ok(envelope(Some(payload.model), Uuid::new_v4().to_string()))
}

async fn handle_job_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<mistralrs_server_core::JobStatus>>, ApiError> {
    let endpoint = "job_status";
    let manager = state.manager.read().await.clone();
    let resp = manager
        .job_status(id)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(resp), Uuid::new_v4().to_string()))
}

async fn handle_logs(
    State(state): State<AppState>,
    Path(model): Path<String>,
) -> Result<Json<ApiResponse<Vec<StructuredLog>>>, ApiError> {
    let endpoint = "logs";
    let manager = state.manager.read().await.clone();
    let logs = manager
        .logs(&model, 200)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(logs), Uuid::new_v4().to_string()))
}

async fn handle_health(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let endpoint = "healthz";
    let manager = state.manager.read().await.clone();
    let models = manager
        .list_models(usize::MAX, 0)
        .await
        .map_err(|err| map_manager_error(&state, endpoint, "GET", err))?;
    let status = json!({
        "status": "ok",
        "models": models.len(),
    });
    record_success(&state, endpoint, "GET", StatusCode::OK);
    Ok(envelope(Some(status), Uuid::new_v4().to_string()))
}

async fn handle_metrics(State(state): State<AppState>) -> Response {
    match state.metrics.render() {
        Ok(body) => {
            record_success(&state, "metrics", "GET", StatusCode::OK);
            (StatusCode::OK, body).into_response()
        }
        Err(err) => {
            let api_error = ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metrics",
                err.to_string(),
            );
            state
                .metrics
                .record("metrics", "GET", api_error.status().as_u16());
            api_error.into_response()
        }
    }
}

async fn handle_reload_config(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<ServerConfig>>, ApiError> {
    let endpoint = "reload_config";
    let new_config = state.config.reload().await.map_err(|err| {
        let api_error = ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "config", err.to_string());
        state
            .metrics
            .record(endpoint, "POST", api_error.status().as_u16());
        api_error
    })?;

    let new_scheduler = (state.scheduler_factory)(&new_config);
    
    let new_manager = (state.factory)(&new_config, new_scheduler.clone(), &state.metrics)
        .await
        .map_err(|err| {
            let api_error =
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "config", err.to_string());
            state
                .metrics
                .record(endpoint, "POST", api_error.status().as_u16());
            api_error
        })?;

    *state.scheduler.write().await = new_scheduler;
    *state.manager.write().await = new_manager;

    record_success(&state, endpoint, "POST", StatusCode::ACCEPTED);
    Ok(envelope(Some(new_config), Uuid::new_v4().to_string()))
}

#[derive(Debug, Deserialize)]
struct EvictRequest {
    models: Option<Vec<String>>,
}

async fn handle_admin_evict(
    State(state): State<AppState>,
    Json(payload): Json<EvictRequest>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let endpoint = "admin_evict";
    let candidates = if let Some(models) = payload.models {
        models
    } else {
        state.scheduler.read().await.advise_evict().await
    };

    let mut unloaded = Vec::new();
    let manager = state.manager.read().await.clone();
    for model in candidates {
        if manager.unload_model(&model).await.is_ok() {
            unloaded.push(model);
        }
    }
    record_success(&state, endpoint, "POST", StatusCode::OK);
    Ok(envelope(Some(unloaded), Uuid::new_v4().to_string()))
}

#[cfg(all(test, feature = "mock-manager"))]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::util::ServiceExt;

    use mistralrs_server_config::{ConfigManager, ConfigSource, ServerConfig};
    use mistralrs_server_core::{
        InMemoryModelManager, ManagerConfig, NoopScheduler, SystemClock,
    };

    use super::*;

    #[tokio::test]
    async fn healthz() {
        let manager: DynModelManager = Arc::new(InMemoryModelManager::new(
            ManagerConfig {
                keep_alive_default: std::time::Duration::from_secs(1),
                max_loaded_models: 2,
                max_parallel_requests_per_model: 2,
            },
            Arc::new(NoopScheduler),
            SystemClock,
        ));
        let config = ConfigManager::load(ConfigSource::Inline(ServerConfig::default()))
            .await
            .unwrap();
        let cfg_snapshot = config.get().await;
        let metrics = HttpMetrics::new().unwrap();
        let scheduler_factory: SchedulerFactory = Arc::new(|_| Arc::new(NoopScheduler));
        let state = AppState {
            manager: Arc::new(RwLock::new(manager)),
            factory: Arc::new(|_, _, _| {
                let manager: DynModelManager = Arc::new(InMemoryModelManager::new(
                    ManagerConfig {
                        keep_alive_default: std::time::Duration::from_secs(1),
                        max_loaded_models: 2,
                        max_parallel_requests_per_model: 2,
                    },
                    Arc::new(NoopScheduler),
                    SystemClock,
                ));
                Box::pin(async move { Ok(manager) })
            }),
            scheduler_factory,
            config,
            scheduler: Arc::new(RwLock::new(Arc::new(NoopScheduler))),
            model_metrics: metrics.model_metrics(),
            metrics,
            auth: AuthState::from_section(&cfg_snapshot.auth),
        };
        let app = build_router(state).await.unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reload_swaps_manager() {
        let initial_manager: DynModelManager = Arc::new(InMemoryModelManager::new(
            ManagerConfig {
                keep_alive_default: std::time::Duration::from_secs(1),
                max_loaded_models: 2,
                max_parallel_requests_per_model: 2,
            },
            Arc::new(NoopScheduler),
            SystemClock,
        ));
        let config = ConfigManager::load(ConfigSource::Inline(ServerConfig::default()))
            .await
            .unwrap();
        let cfg_snapshot = config.get().await;
        let metrics = HttpMetrics::new().unwrap();
        
        let factory_called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_called_clone = factory_called.clone();
        
        let factory: ManagerFactory = Arc::new(move |_, _, _| {
            factory_called_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let fut = async move {
                let manager: DynModelManager = Arc::new(InMemoryModelManager::new(
                    ManagerConfig {
                        keep_alive_default: std::time::Duration::from_secs(1),
                        max_loaded_models: 2,
                        max_parallel_requests_per_model: 2,
                    },
                    Arc::new(NoopScheduler),
                    SystemClock,
                ));
                Ok(manager)
            };
            Box::pin(fut)
        });

        let scheduler_factory: SchedulerFactory = Arc::new(|_| Arc::new(NoopScheduler));
        let state = AppState {
            manager: Arc::new(RwLock::new(initial_manager)),
            factory,
            scheduler_factory,
            config,
            scheduler: Arc::new(RwLock::new(Arc::new(NoopScheduler))),
            model_metrics: metrics.model_metrics(),
            metrics,
            auth: AuthState::from_section(&cfg_snapshot.auth),
        };
        let app = build_router(state).await.unwrap();
        
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload-config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
            
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(factory_called.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}