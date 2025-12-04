use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
    fs,
};
use std::path::PathBuf;
use walkdir::WalkDir;

use candle_core::Device;

mod engine;
mod metrics;
mod runtime;
mod streams;
mod translate;
pub use engine::{EngineHandle, SharedMistralRsState};
pub use runtime::RuntimeAdapters;
use dashmap::{DashMap, DashSet};

use async_trait::async_trait;
#[cfg(feature = "mock-manager")]
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tracing::warn;
#[cfg(feature = "mock-manager")]
use tokio::sync::Semaphore;
#[cfg(feature = "mock-manager")]
use tokio::time::sleep;
#[cfg(feature = "mock-manager")]
use tracing::instrument;
use uuid::Uuid;
use tokio::sync::mpsc;
use upstream_mistralrs_server_core::handler_core::DEFAULT_CHANNEL_BUFFER_SIZE;

pub use metrics::ModelMetrics;
pub use streams::{ChatStreamWrapper, CompletionStreamWrapper, StreamInstrumentation, StreamKind};
use translate::{build_chat_request, build_embedding_request, build_generate_request};
pub use mistralrs_server_config::ServerConfig;
use mistralrs_server_config::{MistralBuilderConfig, ModelBuilderParams, TokenSource};
pub use mistralrs_core::{
    ChatCompletionChunkResponse,
    ChatCompletionResponse as EngineChatResponse,
    CompletionChunkResponse,
    CompletionResponse as EngineCompletionResponse,
    Response as EngineResponse,
    Usage as EngineUsage,
};
use mistralrs_core::{
    DeviceMapSetting,
    LoaderBuilder,
    EmbeddingResponse as EngineEmbeddingResponse,
    Request as EngineRequest,
    ResponseMessage as EngineResponseMessage,
    ToolCallResponse,
    TokenSource as UpTokenSource,
    get_auto_device_map_params,
    get_model_dtype,
};
#[cfg(feature = "mock-manager")]
use mistralrs_core::{
    ChunkChoice,
    CompletionChoice,
    Choice,
    CompletionChunkChoice,
    Delta,
};

/// Participant role attached to a chat message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Chat message exchanged with the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    /// Role producing the content.
    pub role: MessageRole,
    /// Multi-part message body.
    #[serde(deserialize_with = "deserialize_chat_content")]
    pub content: Vec<ChatContent>,
    /// Tool calls issued by the assistant message.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Optional author name.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional identifier linking to a tool invocation.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

fn deserialize_chat_content<'de, D>(deserializer: D) -> Result<Vec<ChatContent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ContentInput {
        String(String),
        Array(Vec<ChatContent>),
    }

    let input = ContentInput::deserialize(deserializer)?;
    match input {
        ContentInput::String(s) => Ok(vec![ChatContent::Text { text: s }]),
        ContentInput::Array(a) => Ok(a),
    }
}

/// Content building blocks that compose a chat message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    ImageUrl { url: String },
}

/// Tool definition compatible with OpenAI's schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name exposed to the model.
    pub name: String,
    /// Optional description surfaced in the prompt.
    pub description: Option<String>,
    /// JSON schema describing the expected arguments.
    pub parameters: serde_json::Value,
}

/// Result returned when a tool executes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Tool name executed.
    pub name: String,
    /// JSON encoded arguments.
    pub arguments: serde_json::Value,
    /// Unique identifier for correlation.
    pub id: String,
}

/// Structured output configuration used for JSON mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredOutputSchema {
    /// JSON schema definition.
    pub schema: serde_json::Value,
    /// Maximum retries before falling back to text.
    #[serde(default = "default_schema_retries")]
    pub max_retries: u8,
}

/// Request body for `/api/generate` and `/v1/completions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Target model identifier.
    pub model: String,
    /// Prompt passed to the model.
    pub prompt: String,
    /// Optional completion token cap.
    pub max_tokens: Option<u32>,
    /// Temperature applied to sampling.
    pub temperature: Option<f32>,
    /// Streams partial tokens when set to true.
    #[serde(default)]
    pub stream: bool,
    /// Optional keep-alive override string (e.g. `"30s"`).
    #[serde(default)]
    pub keep_alive: Option<String>,
    /// Optional OpenAI-style logit bias map keyed by token ID.
    #[serde(default)]
    pub logit_bias: Option<HashMap<String, f32>>,
    /// Tool selection behavior.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    /// Tool definitions exposed to the completion.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Structured output schema for JSON mode.
    #[serde(default)]
    pub structured_output: Option<StructuredOutputSchema>,
    /// Arbitrary metadata forwarded to logs.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Optional advanced sampling controls.
    #[serde(default)]
    pub context: Option<ContextControls>,
}

/// Advanced sampling controls accepted by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextControls {
    /// Context window limit override.
    pub num_ctx: Option<u32>,
    /// Restricts decoding to the top-K tokens.
    pub top_k: Option<u32>,
    /// Probability mass cutoff for nucleus sampling.
    pub top_p: Option<f32>,
    /// Minimum probability threshold.
    pub min_p: Option<f32>,
    /// Penalty applied to repeated tokens.
    pub repeat_penalty: Option<f32>,
    /// Encourages introducing new topics.
    pub presence_penalty: Option<f32>,
    /// Penalizes frequent tokens.
    pub frequency_penalty: Option<f32>,
}

/// Tool invocation strategy for chat/generate APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Tool(String),
}

impl Default for ToolChoice {
    fn default() -> Self {
        Self::Auto
    }
}

/// Response body emitted by `/api/generate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    /// Final text output.
    pub output: String,
    /// Completion termination reason.
    pub finish_reason: FinishReason,
    /// Token usage accounting.
    pub usage: Usage,
    /// Tool calls triggered during generation.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

/// Enumerates completion termination reasons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCall,
    Error,
}

/// Token usage counters shared across responses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    pub prompt_tokens: u32,
    /// Tokens produced by the model.
    pub completion_tokens: u32,
    /// Combined total tokens.
    pub total_tokens: u32,
}

/// Request body for `/api/chat` and `/v1/chat/completions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Target model identifier.
    pub model: String,
    /// Ordered list of chat messages.
    pub messages: Vec<ChatMessage>,
    /// Optional completion token cap.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature applied to sampling.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Advanced sampling controls.
    #[serde(default)]
    pub context: Option<ContextControls>,
    /// Streams partial deltas when true.
    #[serde(default)]
    pub stream: bool,
    /// Tool definitions exposed to the model.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Tool invocation strategy.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    /// Optional keep-alive override string (e.g. `"30s"`).
    #[serde(default)]
    pub keep_alive: Option<String>,
    /// Optional OpenAI-style logit bias map keyed by token ID.
    #[serde(default)]
    pub logit_bias: Option<HashMap<String, f32>>,
    /// Structured output schema for JSON responses.
    #[serde(default)]
    pub structured_output: Option<StructuredOutputSchema>,
}

/// Response payload for chat completions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Assistant message returned to the caller.
    pub message: ChatMessage,
    /// Token accounting for the request.
    pub usage: Usage,
    /// Completion termination reason.
    pub finish_reason: String,
}

/// Request body for `/api/embeddings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    /// Target embedding model identifier.
    pub model: String,
    /// Input text(s) to embed.
    pub input: EmbeddingInput,
    /// Normalizes the vectors when set to true.
    #[serde(default)]
    pub normalize: bool,
}

/// Embedding input wrapper for single or batched payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Multiple(Vec<String>),
}

/// Embedding response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// Embedding vectors for each input record.
    pub embeddings: Vec<Vec<f32>>,
    /// Token usage summary.
    pub usage: Usage,
}

/// Request body used to load a model into memory.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadModelRequest {
    /// Model identifier to load.
    pub model: String,
    /// Optional keep-alive duration expressed as `humantime`.
    #[serde(default)]
    #[serde(with = "humantime_serde::option")]
    pub keep_alive: Option<Duration>,
    /// Marks the model as pinned when true.
    #[serde(default)]
    pub pinned: bool,
}

/// Metadata describing a loaded model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    /// Model identifier.
    pub name: String,
    /// Estimated disk footprint in bytes.
    pub size_bytes: u64,
    /// Maximum context length supported by the model.
    pub context_length: u32,
    /// Quantization identifier if applicable.
    pub quantization: Option<String>,
    /// Whether the model is currently resident.
    pub loaded: bool,
    /// Keep-alive duration before eviction.
    #[serde(with = "humantime_serde")]
    pub keep_alive: Duration,
    /// Indicates pinned status.
    pub pinned: bool,
    /// Additional metadata forwarded to API clients.
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Model record describing last access metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRecord {
    /// Model metadata snapshot.
    pub metadata: ModelMetadata,
    /// Timestamp of last access.
    pub last_accessed: SystemTime,
}

/// Live activity metrics for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveModel {
    /// Model metadata snapshot.
    pub metadata: ModelMetadata,
    /// Number of active requests currently executing.
    pub active_requests: u32,
}

/// Known job status phases for pull operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatusKind {
    Queued,
    Running,
    Succeeded,
    Failed,
}

/// Job status payload exposed over the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    /// Job identifier.
    pub id: Uuid,
    /// Current status value.
    pub status: JobStatusKind,
    /// Completion percentage between 0.0 and 1.0.
    pub progress: f32,
    /// Optional human readable message.
    pub message: Option<String>,
}

/// Structured log entry stored by the mock manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredLog {
    /// Timestamp of the log entry.
    pub timestamp: SystemTime,
    /// Log level string.
    pub level: String,
    /// Human readable message.
    pub message: String,
    /// Structured JSON payload.
    pub fields: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum ModelManagerError {
    #[error("model not found: {0}")]
    NotFound(String),
    #[error("max parallel requests reached for {0}")]
    MaxParallel(String),
    #[error("scheduler rejected model load: {0}")]
    Scheduler(String),
    #[error("global concurrent request limit reached")]
    ServiceUnavailable,
    #[error("{0}")]
    Other(String),
}

pub type DynModelManager = Arc<dyn ModelManager>;

#[async_trait]
pub trait ModelManager: Send + Sync {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, ModelManagerError>;
    async fn generate_stream(
        &self,
        req: GenerateRequest,
    ) -> Result<CompletionStreamWrapper, ModelManagerError>;
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ModelManagerError>;
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStreamWrapper, ModelManagerError>;
    async fn embeddings(
        &self,
        req: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ModelManagerError>;
    async fn load_model(&self, req: LoadModelRequest) -> Result<ModelMetadata, ModelManagerError>;
    async fn unload_model(&self, model: &str) -> Result<(), ModelManagerError>;
    async fn list_models(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ModelRecord>, ModelManagerError>;
    async fn active_models(&self) -> Result<Vec<ActiveModel>, ModelManagerError>;
    async fn job_status(&self, id: Uuid) -> Result<JobStatus, ModelManagerError>;
    async fn logs(
        &self,
        model: &str,
        limit: usize,
    ) -> Result<Vec<StructuredLog>, ModelManagerError>;
    async fn submit_pull_job(&self, model: &str, source: &str) -> Result<Uuid, ModelManagerError>;
}

#[async_trait]
pub trait ModelScheduler: Send + Sync {
    async fn on_model_loaded(&self, metadata: &ModelMetadata);
    async fn on_model_unloaded(&self, model: &str);
    async fn advise_evict(&self) -> Vec<String>;
    async fn register_activity(&self, model: &str);
    fn set_keep_alive_lookup(
        &self,
        _lookup: Arc<dyn Fn(&str) -> Option<Instant> + Send + Sync>,
    ) {
    }
    fn can_load_model(&self, model_name: &str, estimated_size_bytes: u64) -> Result<(), ModelManagerError>;
    fn set_max_vram_bytes(&self, bytes: usize);
    fn register_metrics(&self, _registry: &prometheus::Registry) -> Result<(), anyhow::Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct NoopScheduler;

#[async_trait]
impl ModelScheduler for NoopScheduler {
    async fn on_model_loaded(&self, _metadata: &ModelMetadata) {}
    async fn on_model_unloaded(&self, _model: &str) {}
    async fn advise_evict(&self) -> Vec<String> {
        vec![]
    }
    async fn register_activity(&self, _model: &str) {}
    fn can_load_model(&self, _model_name: &str, _estimated_size_bytes: u64) -> Result<(), ModelManagerError> {
        Ok(())
    }
    fn set_max_vram_bytes(&self, _bytes: usize) {}
}

#[derive(Clone)]
pub struct ManagerConfig {
    pub keep_alive_default: Duration,
    pub max_loaded_models: usize,
    pub max_parallel_requests_per_model: usize,
    pub max_total_concurrent_requests: usize,
    pub paged_attn_gpu_mem: Option<usize>,
}

impl From<&ServerConfig> for ManagerConfig {
    fn from(value: &ServerConfig) -> Self {
        Self {
            keep_alive_default: value.scheduler.keep_alive_default,
            max_loaded_models: value.scheduler.max_loaded_models,
            max_parallel_requests_per_model: value.scheduler.max_parallel_requests_per_model,
            max_total_concurrent_requests: value.server.max_total_concurrent_requests,
            paged_attn_gpu_mem: value.scheduler.paged_attn_gpu_mem,
        }
    }
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn now_system(&self) -> SystemTime;
}

#[derive(Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn now_system(&self) -> SystemTime {
        SystemTime::now()
    }
}

const LOG_CAPACITY: usize = 200;

pub(crate) struct ActiveRequestGuard {
    model: String,
    counter: Arc<AtomicU32>,
    total: Arc<AtomicU64>,
    metrics: Arc<ModelMetrics>,
}

impl ActiveRequestGuard {
    fn try_new(
        model: String,
        counter: Arc<AtomicU32>,
        total: Arc<AtomicU64>,
        metrics: Arc<ModelMetrics>,
        limit: usize,
        global_limit: usize,
    ) -> Result<Self, ModelManagerError> {
        let prev = counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                if (current as usize) >= limit {
                    None
                } else {
                    Some(current + 1)
                }
            })
            .map_err(|_| ModelManagerError::MaxParallel(model.clone()))?;
        
        let total_res = total.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            if (current as usize) >= global_limit {
                None
            } else {
                Some(current + 1)
            }
        });

        if total_res.is_err() {
            counter.fetch_sub(1, Ordering::SeqCst);
            return Err(ModelManagerError::ServiceUnavailable);
        }

        let active = prev + 1;
        let total_now = total_res.unwrap() + 1;
        metrics.track_active(&model, active as i64);
        metrics.set_total_active(total_now as i64);
        Ok(Self {
            model,
            counter,
            total,
            metrics,
        })
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        let remaining = self.counter.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        let total_left = self.total.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        self.metrics
            .track_active(&self.model, remaining as i64);
        self.metrics
            .set_total_active(total_left as i64);
    }
}

struct ManagedModelState {
    metadata: ModelMetadata,
    last_accessed: SystemTime,
}

pub struct MistralModelManager {
    engine: Arc<dyn EngineHandle>,
    engine_state: Option<SharedMistralRsState>,
    scheduler: Arc<dyn ModelScheduler>,
    manager_cfg: ManagerConfig,
    builder_cfg: MistralBuilderConfig,
    model_params: HashMap<String, ModelBuilderParams>,
    models: DashMap<String, ManagedModelState>,
    logs: DashMap<String, VecDeque<StructuredLog>>,
    jobs: DashMap<Uuid, JobStatus>,
    keep_alive: DashMap<String, Instant>,
    pinned: DashSet<String>,
    active_requests: DashMap<String, Arc<AtomicU32>>,
    metrics: Arc<ModelMetrics>,
    rt: RuntimeAdapters,
    clock: SystemClock,
    total_active: Arc<AtomicU64>,
    request_counter: AtomicU64,
}

impl MistralModelManager {
    pub async fn new(
        cfg: &ServerConfig,
        scheduler: Arc<dyn ModelScheduler>,
        registry: &prometheus::Registry,
        metrics: Arc<ModelMetrics>,
        rt: RuntimeAdapters,
    ) -> Result<Self, ModelManagerError> {
        let builder_cfg = MistralBuilderConfig::try_from(cfg)
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let builder = builder_cfg
            .to_builder()
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let shared = builder
            .build()
            .await
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let shared_state = SharedMistralRsState::from(shared);
        let engine: Arc<dyn EngineHandle> = Arc::new(shared_state.clone());
        let manager_cfg = ManagerConfig::from(cfg);
        Self::from_components(
            builder_cfg,
            manager_cfg,
            scheduler,
            engine,
            Some(shared_state),
            registry,
            metrics,
            rt,
        )
    }

    pub fn from_components(
        builder_cfg: MistralBuilderConfig,
        manager_cfg: ManagerConfig,
        scheduler: Arc<dyn ModelScheduler>,
        engine: Arc<dyn EngineHandle>,
        engine_state: Option<SharedMistralRsState>,
        _registry: &prometheus::Registry,
        metrics: Arc<ModelMetrics>,
        rt: RuntimeAdapters,
    ) -> Result<Self, ModelManagerError> {
        let mut model_params = HashMap::new();
        let models = DashMap::new();
        let logs = DashMap::new();
        let jobs = DashMap::new();
        let keep_alive = DashMap::new();
        let pinned = DashSet::new();
        let active_requests = DashMap::new();
        for params in &builder_cfg.models {
            model_params.insert(params.model_id.clone(), params.clone());
            let metadata = Self::metadata_from_params(params, &manager_cfg);
            models.insert(
                params.model_id.clone(),
                ManagedModelState {
                    metadata: metadata.clone(),
                    last_accessed: SystemTime::now(),
                },
            );
            logs.insert(params.model_id.clone(), Self::new_log_buffer());
            keep_alive.insert(
                params.model_id.clone(),
                Instant::now() + metadata.keep_alive,
            );
            if metadata.pinned {
                pinned.insert(params.model_id.clone());
            }
            active_requests.insert(params.model_id.clone(), Arc::new(AtomicU32::new(0)));
        }
        let manager = Self {
            engine,
            engine_state,
            scheduler,
            manager_cfg,
            builder_cfg,
            model_params,
            models,
            logs,
            jobs,
            keep_alive,
            pinned,
            active_requests,
            metrics,
            rt,
            clock: SystemClock,
            total_active: Arc::new(AtomicU64::new(0)),
            request_counter: AtomicU64::new(1),
        };
        manager.register_keep_alive_callback();
        manager.register_existing_models();
        manager.scheduler.set_max_vram_bytes(manager.manager_cfg.paged_attn_gpu_mem.unwrap_or(0));
        Ok(manager)
    }

    fn register_existing_models(&self) {
        for entry in self.models.iter() {
            if entry.metadata.loaded {
                self.metrics.inc_loaded();
                self.metrics.track_active(&entry.metadata.name, 0);
                let metadata = entry.metadata.clone();
                let scheduler = self.scheduler.clone();
                self.rt.spawn_download(async move {
                    scheduler.on_model_loaded(&metadata).await;
                });
            }
        }
    }

    fn register_keep_alive_callback(&self) {
        let keep_alive = self.keep_alive.clone();
        self.scheduler.set_keep_alive_lookup(Arc::new(move |model| {
            keep_alive
                .get(model)
                .map(|entry| *entry.value())
        }));
    }

    fn ensure_model_exists(&self, model: &str) -> Result<ModelMetadata, ModelManagerError> {
        self.models
            .get(model)
            .map(|entry| entry.metadata.clone())
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))
    }

    fn metadata_from_params(
        params: &ModelBuilderParams,
        manager_cfg: &ManagerConfig,
    ) -> ModelMetadata {
        let context_length = params
            .context_length
            .map(|len| len.min(u32::MAX as usize) as u32)
            .unwrap_or(0);
        ModelMetadata {
            name: params.model_id.clone(),
            size_bytes: 0,
            context_length,
            quantization: params.quantization.clone(),
            loaded: true,
            keep_alive: Self::effective_keep_alive(params, manager_cfg),
            pinned: params.pinned,
            parameters: HashMap::new(),
        }
    }

    fn effective_keep_alive(
        params: &ModelBuilderParams,
        manager_cfg: &ManagerConfig,
    ) -> Duration {
        params
            .keep_alive_override
            .or(params.keep_alive)
            .unwrap_or(manager_cfg.keep_alive_default)
    }

    fn new_log_buffer() -> VecDeque<StructuredLog> {
        VecDeque::with_capacity(LOG_CAPACITY)
    }

    fn next_request_id(&self) -> usize {
        self.request_counter.fetch_add(1, Ordering::Relaxed) as usize
    }

    fn acquire_guard(&self, model: &str) -> Result<ActiveRequestGuard, ModelManagerError> {
        let counter = self
            .active_requests
            .get(model)
            .map(|entry| entry.clone())
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))?;
        
        let limit = self.parallel_limit(model);
        let global_limit = self.manager_cfg.max_total_concurrent_requests;

        ActiveRequestGuard::try_new(
            model.to_string(),
            counter,
            Arc::clone(&self.total_active),
            self.metrics.clone(),
            limit,
            global_limit,
        )
    }

    fn parallel_limit(&self, model: &str) -> usize {
        self.model_params
            .get(model)
            .and_then(|params| params.max_parallel_requests)
            .unwrap_or(self.manager_cfg.max_parallel_requests_per_model)
    }

    fn touch_model(&self, model: &str) {
        if let Some(mut entry) = self.models.get_mut(model) {
            entry.last_accessed = self.clock.now_system();
            let ttl = entry.metadata.keep_alive;
            self.keep_alive
                .insert(model.to_string(), self.clock.now() + ttl);
        }
    }

    fn apply_keep_alive_override(
        &self,
        model: &str,
        override_str: Option<&String>,
    ) -> Result<(), ModelManagerError> {
        let duration = if let Some(value) = override_str {
            Some(
                humantime::parse_duration(value)
                    .map_err(|err| ModelManagerError::Other(format!(
                        "invalid keep_alive override: {err}"
                    )))?,
            )
        } else {
            None
        };
        let ttl = duration.unwrap_or_else(|| {
            self.models
                .get(model)
                .map(|entry| entry.metadata.keep_alive)
                .unwrap_or(self.manager_cfg.keep_alive_default)
        });
        self.keep_alive
            .insert(model.to_string(), self.clock.now() + ttl);
        Ok(())
    }

    fn append_log(&self, model: &str, entry: StructuredLog) {
        Self::push_log_entry(&self.logs, model, entry);
    }

    fn push_info_log(&self, model: &str, message: &str, fields: Value) {
        let entry = StructuredLog {
            timestamp: self.clock.now_system(),
            level: "info".to_string(),
            message: message.to_string(),
            fields,
        };
        self.append_log(model, entry);
    }

    fn push_log_entry(
        logs: &DashMap<String, VecDeque<StructuredLog>>,
        model: &str,
        entry: StructuredLog,
    ) {
        if let Some(mut buffer) = logs.get_mut(model) {
            if buffer.len() >= LOG_CAPACITY {
                buffer.pop_front();
            }
            buffer.push_back(entry);
        }
    }

    fn ensure_log_buffer(&self, model: &str) {
        if self.logs.get(model).is_none() {
            self.logs.insert(model.to_string(), Self::new_log_buffer());
        }
    }

    fn ensure_active_counter(&self, model: &str) {
        if self.active_requests.get(model).is_none() {
            self.active_requests
                .insert(model.to_string(), Arc::new(AtomicU32::new(0)));
        }
    }

    fn stream_log_sink(&self, model: &str) -> Option<Arc<dyn Fn(StructuredLog) + Send + Sync>> {
        if self.logs.get(model).is_none() {
            return None;
        }
        let logs = self.logs.clone();
        let name = model.to_string();
        Some(Arc::new(move |entry: StructuredLog| {
            MistralModelManager::push_log_entry(&logs, &name, entry);
        }))
    }

    fn stream_instrumentation(&self, model: &str, kind: StreamKind) -> StreamInstrumentation {
        StreamInstrumentation {
            model: model.to_string(),
            metrics: self.metrics.clone(),
            log_sink: self.stream_log_sink(model),
            clock: self.clock,
            kind,
        }
    }

    async fn refresh_loaded_metadata(
        &self,
        req: &LoadModelRequest,
    ) -> Result<ModelMetadata, ModelManagerError> {
        let mut entry = self
            .models
            .get_mut(&req.model)
            .ok_or_else(|| ModelManagerError::NotFound(req.model.clone()))?;
        if let Some(keep_alive) = req.keep_alive {
            entry.metadata.keep_alive = keep_alive;
        }
        if req.pinned {
            entry.metadata.pinned = true;
            self.pinned.insert(req.model.clone());
        }
        self.keep_alive
            .insert(req.model.clone(), self.clock.now() + entry.metadata.keep_alive);
        self.metrics.track_active(&req.model, 0);
        self.scheduler.on_model_loaded(&entry.metadata).await;
        Ok(entry.metadata.clone())
    }

    fn finalize_model_load(
        &self,
        req: &LoadModelRequest,
        params: &ModelBuilderParams,
    ) -> Result<ModelMetadata, ModelManagerError> {
        let now = self.clock.now_system();
        let metadata = if let Some(mut entry) = self.models.get_mut(&req.model) {
            if let Some(keep_alive) = req.keep_alive {
                entry.metadata.keep_alive = keep_alive;
            }
            if req.pinned {
                entry.metadata.pinned = true;
            }
            entry.metadata.loaded = true;
            entry.last_accessed = now;
            entry.metadata.clone()
        } else {
            let mut metadata = Self::metadata_from_params(params, &self.manager_cfg);
            if let Some(keep_alive) = req.keep_alive {
                metadata.keep_alive = keep_alive;
            }
            if req.pinned {
                metadata.pinned = true;
            }
            metadata.loaded = true;
            self.models.insert(
                req.model.clone(),
                ManagedModelState {
                    metadata: metadata.clone(),
                    last_accessed: now,
                },
            );
            metadata
        };
        if metadata.pinned {
            self.pinned.insert(req.model.clone());
        } else {
            self.pinned.remove(&req.model);
        }
        self.ensure_log_buffer(&req.model);
        self.ensure_active_counter(&req.model);
        self.keep_alive
            .insert(req.model.clone(), self.clock.now() + metadata.keep_alive);
        Ok(metadata)
    }

    async fn apply_scheduler_advice(&self) {
        let advice = self.scheduler.advise_evict().await;
        for model in advice {
            if let Err(err) = self.unload_model_internal(&model, false, "scheduler").await {
                warn!(target: "mistralrs::scheduler", %model, error = %err, "failed to unload advised model");
            }
        }
    }

    fn has_active_requests(&self, model: &str) -> bool {
        self.active_requests
            .get(model)
            .map(|counter| counter.load(Ordering::SeqCst) > 0)
            .unwrap_or(false)
    }

    async fn unload_model_internal(
        &self,
        model: &str,
        allow_pinned: bool,
        reason: &str,
    ) -> Result<(), ModelManagerError> {
        let exists = self
            .models
            .contains_key(model)
            .then_some(())
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))?;
        let _ = exists;
        if self.pinned.contains(model) && !allow_pinned {
            return Err(ModelManagerError::Scheduler(format!(
                "model {model} is pinned"
            )));
        }
        if self.has_active_requests(model) {
            return Err(ModelManagerError::Scheduler(format!(
                "model {model} still has active requests"
            )));
        }
        let was_loaded = {
            if let Some(mut entry) = self.models.get_mut(model) {
                let loaded = entry.metadata.loaded;
                if loaded {
                    entry.metadata.loaded = false;
                }
                loaded
            } else {
                false
            }
        };
        if !was_loaded {
            return Ok(());
        }
        self.engine.remove_model(model).await?;
        self.keep_alive.remove(model);
        self.metrics.inc_unloaded();
        self.scheduler.on_model_unloaded(model).await;
        self.push_info_log(model, "unload_model", json!({"reason": reason}));
        Ok(())
    }

    async fn ensure_engine_has_model(&self, model: &str) -> Result<(), ModelManagerError> {
        match self.engine.ensure_model_loaded(model).await {
            Ok(_) => Ok(()),
            Err(ModelManagerError::NotFound(_)) => {
                self.load_model(LoadModelRequest {
                    model: model.to_string(),
                    keep_alive: None,
                    pinned: false,
                })
                .await
                .map(|_| ())
            }
            Err(err) => Err(err),
        }
    }

    fn pull_job_id(model: &str, source: &str) -> Uuid {
        let key = format!("{model}:{source}");
        Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes())
    }

    fn calculate_model_disk_size(params: &ModelBuilderParams) -> Option<u64> {
        let model_dir: PathBuf;

        if let Some(_repo_id) = params.source.strip_prefix("hf://") {
            // TODO: Implement proper HF cache path resolution
            return None;
        } else {
            // Assume params.source is a local path
            let candidate_path = PathBuf::from(&params.source);
            if candidate_path.is_absolute() {
                model_dir = candidate_path;
            } else {
                model_dir = params.model_base_dir.as_ref()?.join(&params.source);
            }
        }

        if !model_dir.exists() {
            return None;
        }

        let mut total_size = 0;
        for entry in WalkDir::new(&model_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                total_size += entry.metadata().ok()?.len();
            }
        }
        Some(total_size)
    }

    fn spawn_pull_job(&self, id: Uuid, model: String, source: String, immediate: bool) {
        if immediate {
            self.jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Succeeded,
                    progress: 1.0,
                    message: Some(format!("pulled {model} from {source}")),
                },
            );
            return;
        }
        if self.engine_state.is_none() {
            self.simulate_pull_job(id, model, source);
            return;
        }
        let params = match self.model_params.get(&model).cloned() {
            Some(p) => p,
            None => {
                self.jobs.insert(
                    id,
                    JobStatus {
                        id,
                        status: JobStatusKind::Failed,
                        progress: 1.0,
                        message: Some(format!("unknown model {model}")),
                    },
                );
                return;
            }
        };
        let token_source = self.builder_cfg.token_source.clone();
        let cache_hint = params
            .hf_cache_path
            .clone()
            .unwrap_or_else(|| format!("/var/cache/mistral/{model}"));
        let jobs = self.jobs.clone();
        self.rt.spawn_download(async move {
            jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Running,
                    progress: 0.05,
                    message: Some(format!("queued download for {model} ({source})")),
                },
            );
            let params_clone = params.clone();
            let download = tokio::task::spawn_blocking(move || {
                MistralModelManager::download_model_artifacts(&params_clone, token_source)
            })
            .await;
            match download {
                Ok(Ok(())) => {
                    jobs.insert(
                        id,
                        JobStatus {
                            id,
                            status: JobStatusKind::Succeeded,
                            progress: 1.0,
                            message: Some(format!(
                                "cached {model} from {source} at {cache_hint}"
                            )),
                        },
                    );
                }
                Ok(Err(err)) => {
                    jobs.insert(
                        id,
                        JobStatus {
                            id,
                            status: JobStatusKind::Failed,
                            progress: 1.0,
                            message: Some(err.to_string()),
                        },
                    );
                }
                Err(join_err) => {
                    jobs.insert(
                        id,
                        JobStatus {
                            id,
                            status: JobStatusKind::Failed,
                            progress: 1.0,
                            message: Some(join_err.to_string()),
                        },
                    );
                }
            }
        });
    }

    fn simulate_pull_job(&self, id: Uuid, model: String, source: String) {
        let jobs = self.jobs.clone();
        let cache_hint = self
            .model_params
            .get(&model)
            .and_then(|params| params.hf_cache_path.clone())
            .unwrap_or_else(|| format!("/var/cache/mistral/{model}"));
        self.rt.spawn_download(async move {
            jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Running,
                    progress: 0.05,
                    message: Some(format!("queued download for {model} ({source})")),
                },
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
            jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Running,
                    progress: 0.6,
                    message: Some(format!(
                        "downloading artifacts into {}",
                        cache_hint
                    )),
                },
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
            jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Succeeded,
                    progress: 1.0,
                    message: Some(format!(
                        "cached {model} from {source} at {cache_hint}"
                    )),
                },
            );
        });
    }

    fn download_model_artifacts(
        params: &ModelBuilderParams,
        token_source: Option<TokenSource>,
    ) -> Result<(), ModelManagerError> {
        let model_selected = params
            .model_selected()
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let dtype = get_model_dtype(&model_selected)
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let mapper = DeviceMapSetting::Auto(
            get_auto_device_map_params(&model_selected)
                .map_err(|err| ModelManagerError::Other(err.to_string()))?,
        );
        let token = token_source
            .as_ref()
            .map(TokenSource::to_upstream)
            .unwrap_or(UpTokenSource::CacheToken);
        let loader = LoaderBuilder::new(model_selected)
            .with_no_kv_cache(false)
            .with_chat_template(params.jinja_template.clone())
            .with_jinja_explicit(params.jinja_explicit.clone())
            .build()
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let device = Device::Cpu;
        loader
            .load_model_from_hf(
                None,
                token,
                &dtype,
                &device,
                true,
                mapper,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|err| ModelManagerError::Other(err.to_string()))
    }
}

#[async_trait]
impl ModelManager for MistralModelManager {
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, ModelManagerError> {
        let model = req.model.clone();
        self.apply_scheduler_advice().await;
        self.ensure_model_exists(&model)?;
        self.ensure_engine_has_model(&model).await?;
        self.scheduler.register_activity(&model).await;
        self.touch_model(&model);
        self.apply_keep_alive_override(&model, req.keep_alive.as_ref())?;
        let guard = self.acquire_guard(&model)?;
        let (tx, mut rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let engine_req = build_generate_request(&req, tx, self.next_request_id(), false)?;
        self.engine
            .send_request_with_model(EngineRequest::Normal(Box::new(engine_req)), Some(&model))
            .await?;
        drop(req);
        let response = rx
            .recv()
            .await
            .ok_or_else(|| ModelManagerError::Other("generation cancelled".to_string()))?;
        drop(guard);
        match response {
            EngineResponse::CompletionDone(done) => {
                let result = convert_completion_response(done);
                self.metrics.inc_request(&model, "generate");
                self.metrics.add_tokens(
                    &model,
                    result.usage.prompt_tokens,
                    result.usage.completion_tokens,
                );
                let usage = result.usage.clone();
                self.push_info_log(
                    &model,
                    "generate",
                    json!({
                        "finish_reason": finish_reason_str(&result.finish_reason),
                        "usage": usage,
                    }),
                );
                Ok(result)
            }
            EngineResponse::CompletionModelError(msg, _) => Err(ModelManagerError::Other(msg)),
            EngineResponse::InternalError(err) => Err(ModelManagerError::Other(err.to_string())),
            EngineResponse::ValidationError(err) => Err(ModelManagerError::Other(err.to_string())),
            _ => Err(ModelManagerError::Other(
                "unexpected response for completion".to_string(),
            )),
        }
    }

    async fn generate_stream(
        &self,
        req: GenerateRequest,
    ) -> Result<CompletionStreamWrapper, ModelManagerError> {
        let model = req.model.clone();
        self.apply_scheduler_advice().await;
        self.ensure_model_exists(&model)?;
        self.ensure_engine_has_model(&model).await?;
        self.scheduler.register_activity(&model).await;
        self.touch_model(&model);
        self.apply_keep_alive_override(&model, req.keep_alive.as_ref())?;
        let guard = self.acquire_guard(&model)?;
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let engine_req = build_generate_request(&req, tx, self.next_request_id(), true)?;
        self.engine
            .send_request_with_model(EngineRequest::Normal(Box::new(engine_req)), Some(&model))
            .await?;
        self.metrics.inc_request(&model, "generate");
        self.ensure_log_buffer(&model);
        let instrumentation = Some(self.stream_instrumentation(&model, StreamKind::Completion));
        Ok(CompletionStreamWrapper::new(
            Uuid::new_v4().to_string(),
            rx,
            Some(guard),
            instrumentation,
        ))
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ModelManagerError> {
        let model = req.model.clone();
        self.apply_scheduler_advice().await;
        self.ensure_model_exists(&model)?;
        self.ensure_engine_has_model(&model).await?;
        self.scheduler.register_activity(&model).await;
        self.touch_model(&model);
        self.apply_keep_alive_override(&model, req.keep_alive.as_ref())?;
        let guard = self.acquire_guard(&model)?;
        let (tx, mut rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let engine_req = build_chat_request(&req, tx, self.next_request_id(), false).await?;
        self.engine
            .send_request_with_model(EngineRequest::Normal(Box::new(engine_req)), Some(&model))
            .await?;
        drop(req);
        let response = rx
            .recv()
            .await
            .ok_or_else(|| ModelManagerError::Other("chat cancelled".to_string()))?;
        drop(guard);
        match response {
            EngineResponse::Done(done) => {
                let result = convert_chat_response(done)?;
                self.metrics.inc_request(&model, "chat");
                self.metrics.add_tokens(
                    &model,
                    result.usage.prompt_tokens,
                    result.usage.completion_tokens,
                );
                let usage = result.usage.clone();
                self.push_info_log(
                    &model,
                    "chat",
                    json!({
                        "finish_reason": result.finish_reason,
                        "usage": usage,
                    }),
                );
                Ok(result)
            }
            EngineResponse::ModelError(msg, _) => Err(ModelManagerError::Other(msg)),
            EngineResponse::InternalError(err) => Err(ModelManagerError::Other(err.to_string())),
            EngineResponse::ValidationError(err) => Err(ModelManagerError::Other(err.to_string())),
            _ => Err(ModelManagerError::Other(
                "unexpected response for chat".to_string(),
            )),
        }
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<ChatStreamWrapper, ModelManagerError> {
        let model = req.model.clone();
        self.apply_scheduler_advice().await;
        self.ensure_model_exists(&model)?;
        self.ensure_engine_has_model(&model).await?;
        self.scheduler.register_activity(&model).await;
        self.touch_model(&model);
        self.apply_keep_alive_override(&model, req.keep_alive.as_ref())?;
        let guard = self.acquire_guard(&model)?;
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let engine_req = build_chat_request(&req, tx, self.next_request_id(), true).await?;
        self.engine
            .send_request_with_model(EngineRequest::Normal(Box::new(engine_req)), Some(&model))
            .await?;
        self.metrics.inc_request(&model, "chat");
        self.ensure_log_buffer(&model);
        let instrumentation = Some(self.stream_instrumentation(&model, StreamKind::Chat));
        Ok(ChatStreamWrapper::new(
            Uuid::new_v4().to_string(),
            rx,
            Some(guard),
            instrumentation,
        ))
    }

    async fn embeddings(
        &self,
        req: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ModelManagerError> {
        let model = req.model.clone();
        self.apply_scheduler_advice().await;
        self.ensure_model_exists(&model)?;
        self.ensure_engine_has_model(&model).await?;
        self.scheduler.register_activity(&model).await;
        self.touch_model(&model);
        let guard = self.acquire_guard(&model)?;
        let (tx, mut rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let engine_req = build_embedding_request(&req, tx, self.next_request_id());
        self.engine
            .send_request_with_model(EngineRequest::Embedding(engine_req), Some(&model))
            .await?;
        self.metrics.inc_request(&model, "embeddings");
        self.ensure_log_buffer(&model);
        let response = rx
            .recv()
            .await
            .ok_or_else(|| ModelManagerError::Other("embedding channel closed".into()))?;
        drop(guard);
        match response {
            EngineResponse::Embedding(resp) => {
                let converted = convert_embedding_response(resp);
                let usage = converted.usage.clone();
                self.metrics
                    .add_tokens(&model, usage.prompt_tokens, 0);
                self.push_info_log(
                    &model,
                    "embeddings",
                    json!({
                        "count": converted.embeddings.len(),
                        "usage": usage,
                    }),
                );
                Ok(converted)
            }
            EngineResponse::ValidationError(err) => {
                Err(ModelManagerError::Other(err.to_string()))
            }
            EngineResponse::InternalError(err) => {
                Err(ModelManagerError::Other(err.to_string()))
            }
            _ => Err(ModelManagerError::Other(
                "unexpected response for embeddings".to_string(),
            )),
        }
    }

    async fn load_model(&self, req: LoadModelRequest) -> Result<ModelMetadata, ModelManagerError> {
        self.apply_scheduler_advice().await;
        let params = self
            .model_params
            .get(&req.model)
            .cloned()
            .ok_or_else(|| ModelManagerError::NotFound(req.model.clone()))?;
        
        let estimated_size = Self::calculate_model_disk_size(&params).unwrap_or(0);
        self.scheduler
            .can_load_model(&req.model, estimated_size)
            .map_err(|e| e)?;
        
        if self
            .models
            .get(&req.model)
            .map(|entry| entry.metadata.loaded)
            .unwrap_or(false)
        {
            return self.refresh_loaded_metadata(&req).await;
        }
        let builder = self
            .builder_cfg
            .to_builder()
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let model_config = params
            .to_upstream_model_config()
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let engine_state = self
            .engine_state
            .clone()
            .ok_or_else(|| ModelManagerError::Other("dynamic loading is unavailable".into()))?;
        let upstream_state: upstream_mistralrs_server_core::types::SharedMistralRsState =
            engine_state.clone().into();
        builder
            .add_model_config_to_existing(model_config, upstream_state)
            .await
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        let metadata = self.finalize_model_load(&req, &params)?;
        self.metrics.inc_loaded();
        self.metrics.track_active(&req.model, 0);
        self.scheduler.on_model_loaded(&metadata).await;
        self.push_info_log(
            &req.model,
            "load_model",
            json!({
                "pinned": metadata.pinned,
                "keep_alive_secs": metadata.keep_alive.as_secs(),
            }),
        );
        Ok(metadata)
    }

    async fn unload_model(&self, model: &str) -> Result<(), ModelManagerError> {
        self.unload_model_internal(model, false, "manual").await
    }

    async fn list_models(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ModelRecord>, ModelManagerError> {
        let models: Vec<ModelRecord> = self
            .models
            .iter()
            .map(|entry| ModelRecord {
                metadata: entry.metadata.clone(),
                last_accessed: entry.last_accessed,
            })
            .collect();

        Ok(models.into_iter().skip(offset).take(limit).collect())
    }

    async fn active_models(&self) -> Result<Vec<ActiveModel>, ModelManagerError> {
        Ok(self
            .models
            .iter()
            .map(|entry| {
                let count = self
                    .active_requests
                    .get(&entry.metadata.name)
                    .map(|counter| counter.load(Ordering::SeqCst))
                    .unwrap_or(0);
                ActiveModel {
                    metadata: entry.metadata.clone(),
                    active_requests: count,
                }
            })
            .collect())
    }

    async fn job_status(&self, id: Uuid) -> Result<JobStatus, ModelManagerError> {
        self.jobs
            .get(&id)
            .map(|entry| entry.clone())
            .ok_or_else(|| ModelManagerError::NotFound(id.to_string()))
    }

    async fn logs(
        &self,
        model: &str,
        limit: usize,
    ) -> Result<Vec<StructuredLog>, ModelManagerError> {
        let buffer = self
            .logs
            .get(model)
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))?;
        Ok(buffer.iter().rev().take(limit).cloned().collect())
    }

    async fn submit_pull_job(
        &self,
        model: &str,
        source: &str,
    ) -> Result<Uuid, ModelManagerError> {
        self.ensure_model_exists(model)?;
        let id = Self::pull_job_id(model, source);
        let mut spawn = true;
        if let Some(job) = self.jobs.get(&id) {
            if matches!(job.status, JobStatusKind::Running | JobStatusKind::Queued) {
                spawn = false;
            }
        }
        if spawn {
            self.jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Queued,
                    progress: 0.0,
                    message: Some(format!("queued pull for {model} from {source}")),
                },
            );
            let simulate = self.engine_state.is_none();
            self.spawn_pull_job(id, model.to_string(), source.to_string(), simulate);
        }
        Ok(id)
    }
}

fn convert_completion_response(response: EngineCompletionResponse) -> GenerateResponse {
    let usage = convert_usage(&response.usage);
    let mut finish_reason = FinishReason::Stop;
    let mut output = String::new();
    if let Some(choice) = response.choices.into_iter().next() {
        finish_reason = parse_finish_reason(&choice.finish_reason);
        output = choice.text;
    }
    GenerateResponse {
        output,
        finish_reason,
        usage,
        tool_calls: Vec::new(),
    }
}

fn convert_chat_response(
    response: EngineChatResponse,
) -> Result<ChatResponse, ModelManagerError> {
    let usage = convert_usage(&response.usage);
    let mut finish_reason = String::from("stop");
    let message = if let Some(choice) = response.choices.into_iter().next() {
        if !choice.finish_reason.is_empty() {
            finish_reason = choice.finish_reason.clone();
        }
        convert_response_message(choice.message)
    } else {
        return Err(ModelManagerError::Other(
            "chat response missing choices".to_string(),
        ));
    };
    Ok(ChatResponse {
        message,
        usage,
        finish_reason,
    })
}

fn convert_embedding_response(response: EngineEmbeddingResponse) -> EmbeddingResponse {
    let usage = convert_usage(&response.usage);
    let embeddings = response
        .data
        .into_iter()
        .map(|entry| entry.embedding)
        .collect();
    EmbeddingResponse { embeddings, usage }
}

fn convert_response_message(message: EngineResponseMessage) -> ChatMessage {
    let content = message
        .content
        .map(|text| vec![ChatContent::Text { text }])
        .unwrap_or_default();
    let tool_calls = convert_tool_calls(message.tool_calls);
    ChatMessage {
        role: parse_message_role(&message.role),
        content,
        tool_calls,
        name: None,
        tool_call_id: None,
    }
}

fn convert_tool_calls(calls: Option<Vec<ToolCallResponse>>) -> Vec<ToolCall> {
    calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| {
            let arguments = serde_json::from_str(&call.function.arguments)
                .unwrap_or(Value::String(call.function.arguments));
            ToolCall {
                name: call.function.name,
                arguments,
                id: call.id,
            }
        })
        .collect()
}

fn parse_message_role(role: &str) -> MessageRole {
    match role {
        "system" => MessageRole::System,
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::Assistant,
    }
}

fn convert_usage(usage: &EngineUsage) -> Usage {
    Usage {
        prompt_tokens: usage.prompt_tokens.min(u32::MAX as usize) as u32,
        completion_tokens: usage.completion_tokens.min(u32::MAX as usize) as u32,
        total_tokens: usage.total_tokens.min(u32::MAX as usize) as u32,
    }
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCall,
        _ => FinishReason::Error,
    }
}

fn finish_reason_str(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCall => "tool_calls",
        FinishReason::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineHandle;
    use async_trait::async_trait;
    use mistralrs_core::Request;
    use std::{sync::Mutex, time::Duration};

    fn sample_manager_cfg() -> ManagerConfig {
        ManagerConfig {
            keep_alive_default: Duration::from_secs(30),
            max_loaded_models: 2,
            max_parallel_requests_per_model: 4,
            max_total_concurrent_requests: 100,
        }
    }

    fn sample_params() -> ModelBuilderParams {
        ModelBuilderParams {
            model_id: "test".into(),
            source: "hf://test".into(),
            tokenizer_json: None,
            arch: None,
            dtype: None,
            hf_cache_path: None,
            matformer_config_path: None,
            matformer_slice_name: None,
            num_device_layers: None,
            in_situ_quant: None,
            paged_attn: None,
            paged_attn_block_size: None,
            paged_ctxt_len: None,
            paged_gpu_mem: None,
            paged_gpu_mem_usage: None,
            prefix_cache_n: None,
            search_bert_model: None,
            search_enabled: None,
            jinja_template: None,
            jinja_explicit: None,
            tokenizer: None,
            tokenizer_config: None,
            tokenizer_model_id: None,
            quantization: None,
            keep_alive: None,
            keep_alive_override: None,
            pinned: false,
            context_length: None,
            gpu_ids: None,
            max_parallel_requests: None,
            model_base_dir: None,
        }
    }

    fn sample_builder_cfg(
        params: Vec<ModelBuilderParams>,
        cfg: &ManagerConfig,
    ) -> MistralBuilderConfig {
        MistralBuilderConfig {
            keep_alive_default: cfg.keep_alive_default,
            max_loaded_models: cfg.max_loaded_models,
            max_parallel_requests_per_model: cfg.max_parallel_requests_per_model,
            eviction_strategy: "lru".into(),
            max_seqs: None,
            no_kv_cache: None,
            prefix_cache_n: None,
            paged_attention: None,
            token_source: None,
            enable_search: None,
            search_bert_model: None,
            mcp_client: None,
            default_model_id: params
                .first()
                .map(|p| p.model_id.clone())
                .unwrap_or_else(|| "".into()),
            models: params,
            model_base_dir: None,
        }
    }

    #[derive(Clone)]
    struct TestEngine;

    #[async_trait]
    impl EngineHandle for TestEngine {
        async fn ensure_model_loaded(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn send_request_with_model(
            &self,
            _request: Request,
            _model: Option<&str>,
        ) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn remove_model(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn list_models(&self) -> Result<Vec<String>, ModelManagerError> {
            Ok(vec![])
        }
    }

    fn manager(params: Vec<ModelBuilderParams>) -> MistralModelManager {
        let cfg = sample_manager_cfg();
        let scheduler: Arc<dyn ModelScheduler> = Arc::new(NoopScheduler);
        let builder_cfg = sample_builder_cfg(params.clone(), &cfg);
        let mut model_params = HashMap::new();
        let models = DashMap::new();
        let logs = DashMap::new();
        let jobs = DashMap::new();
        let keep_alive = DashMap::new();
        let pinned = DashSet::new();
        let active_requests = DashMap::new();
        for param in params {
            model_params.insert(param.model_id.clone(), param.clone());
            let metadata = MistralModelManager::metadata_from_params(&param, &cfg);
            models.insert(
                param.model_id.clone(),
                ManagedModelState {
                    metadata: metadata.clone(),
                    last_accessed: SystemTime::now(),
                },
            );
            logs.insert(param.model_id.clone(), MistralModelManager::new_log_buffer());
            keep_alive.insert(
                param.model_id.clone(),
                Instant::now() + metadata.keep_alive,
            );
            if metadata.pinned {
                pinned.insert(param.model_id.clone());
            }
            active_requests.insert(param.model_id.clone(), Arc::new(AtomicU32::new(0)));
        }
        let metrics = Arc::new(
            ModelMetrics::register(&prometheus::Registry::new()).expect("metrics"),
        );
        MistralModelManager {
            engine: Arc::new(TestEngine) as Arc<dyn EngineHandle>,
            engine_state: None,
            scheduler,
            manager_cfg: cfg,
            builder_cfg,
            model_params,
            models,
            logs,
            jobs,
            keep_alive,
            pinned,
            active_requests,
            metrics,
            rt: RuntimeAdapters::current(),
            clock: SystemClock,
            total_active: Arc::new(AtomicU64::new(0)),
            request_counter: AtomicU64::new(1),
        }
    }

    #[test]
    fn metadata_respects_overrides() {
        let cfg = sample_manager_cfg();
        let mut params = sample_params();
        params.context_length = Some(8192);
        params.quantization = Some("q4".into());
        params.keep_alive = Some(Duration::from_secs(120));
        params.keep_alive_override = Some(Duration::from_secs(15));
        params.pinned = true;

        let metadata = MistralModelManager::metadata_from_params(&params, &cfg);
        assert_eq!(metadata.context_length, 8192);
        assert_eq!(metadata.quantization.as_deref(), Some("q4"));
        assert_eq!(metadata.keep_alive, Duration::from_secs(15));
        assert!(metadata.pinned);
        assert!(metadata.loaded);
    }

    #[test]
    fn log_buffer_has_expected_capacity() {
        let buffer = MistralModelManager::new_log_buffer();
        assert!(buffer.capacity() >= LOG_CAPACITY);
    }

    #[test]
    fn metadata_clamps_context_length() {
        let cfg = sample_manager_cfg();
        let mut params = sample_params();
        params.context_length = Some(u32::MAX as usize + 42);
        let metadata = MistralModelManager::metadata_from_params(&params, &cfg);
        assert_eq!(metadata.context_length, u32::MAX);
    }

    #[derive(Clone)]
    struct EmbeddingTestEngine;

    #[async_trait]
    impl EngineHandle for EmbeddingTestEngine {
        async fn ensure_model_loaded(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn send_request_with_model(
            &self,
            request: Request,
            _model: Option<&str>,
        ) -> Result<(), ModelManagerError> {
            if let Request::Embedding(req) = request {
                let usage = EngineUsage {
                    prompt_tokens: req.inputs.iter().map(|s| s.len()).sum(),
                    completion_tokens: 0,
                    total_tokens: req.inputs.iter().map(|s| s.len()).sum(),
                    avg_tok_per_sec: 0.0,
                    avg_prompt_tok_per_sec: 0.0,
                    avg_compl_tok_per_sec: 0.0,
                    total_time_sec: 0.0,
                    total_prompt_time_sec: 0.0,
                    total_completion_time_sec: 0.0,
                };
                let data = req
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(idx, value)| mistralrs_core::EmbeddingData {
                        object: "embedding".to_string(),
                        embedding: vec![value.len() as f32 + idx as f32],
                        index: idx,
                    })
                    .collect();
                let response = EngineEmbeddingResponse {
                    id: format!("emb-{}", req.id),
                    object: "list".to_string(),
                    model: req.model_id.unwrap_or_default(),
                    data,
                    usage,
                };
                let _ = req.response.send(EngineResponse::Embedding(response)).await;
            }
            Ok(())
        }

        async fn remove_model(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn list_models(&self) -> Result<Vec<String>, ModelManagerError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct RecordingScheduler {
        loaded: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ModelScheduler for RecordingScheduler {
        async fn on_model_loaded(&self, metadata: &ModelMetadata) {
            let mut guard = self.loaded.lock().unwrap();
            guard.push(metadata.name.clone());
        }

        async fn on_model_unloaded(&self, _model: &str) {}
        async fn advise_evict(&self) -> Vec<String> {
            vec![]
        }
        async fn register_activity(&self, _model: &str) {}
    }

    #[tokio::test]
    async fn embeddings_return_vectors_and_log_usage() {
        let mut mgr = manager(vec![sample_params()]);
        mgr.engine = Arc::new(EmbeddingTestEngine);
        let response = mgr
            .embeddings(EmbeddingRequest {
                model: "test".into(),
                input: EmbeddingInput::Multiple(vec!["foo".into(), "bar".into()]),
                normalize: false,
            })
            .await
            .expect("embeddings");
        assert_eq!(response.embeddings.len(), 2);
        let logs = mgr.logs("test", 1).await.expect("logs");
        assert_eq!(logs[0].message, "embeddings");
    }

    #[tokio::test]
    async fn constructor_registers_existing_models() {
        let cfg = sample_manager_cfg();
        let params = vec![sample_params()];
        let builder_cfg = sample_builder_cfg(params.clone(), &cfg);
        let registry = prometheus::Registry::new();
        let scheduler = Arc::new(RecordingScheduler::default());
        let scheduler_dyn: Arc<dyn ModelScheduler> = scheduler.clone();
        let model_metrics = Arc::new(ModelMetrics::register(&registry).expect("metrics"));
        let manager = MistralModelManager::from_components(
            builder_cfg,
            cfg,
            scheduler_dyn,
            Arc::new(TestEngine),
            None,
            &registry,
            model_metrics,
            RuntimeAdapters::current(),
        )
        .expect("manager");
        assert!(manager.models.contains_key("test"));
        tokio::time::sleep(Duration::from_millis(10)).await;
        let loaded = scheduler.loaded.lock().unwrap().clone();
        assert_eq!(loaded, vec!["test".to_string()]);
    }

    #[tokio::test]
    async fn logs_return_latest_entries() {
        let mgr = manager(vec![sample_params()]);
        let mut entry = mgr.logs.get_mut("test").expect("log buffer");
        entry.push_back(StructuredLog {
            timestamp: SystemTime::now(),
            level: "info".into(),
            message: "old".into(),
            fields: serde_json::json!({"idx": 0}),
        });
        entry.push_back(StructuredLog {
            timestamp: SystemTime::now(),
            level: "info".into(),
            message: "new".into(),
            fields: serde_json::json!({"idx": 1}),
        });
        drop(entry);
        let logs = mgr.logs("test", 1).await.expect("logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "new");
    }

    #[tokio::test]
    async fn submit_pull_job_tracks_status() {
        let mgr = manager(vec![sample_params()]);
        let id = mgr
            .submit_pull_job("test", "hf://repo")
            .await
            .expect("job id");
        assert_eq!(
            id,
            Uuid::new_v5(&Uuid::NAMESPACE_OID, b"test:hf://repo")
        );
        let mut attempts = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let status = mgr.job_status(id).await.expect("job status");
            if status.status == JobStatusKind::Succeeded {
                assert_eq!(status.progress, 1.0);
                break;
            }
            attempts += 1;
            if attempts > 20 {
                panic!("job did not complete");
            }
        }
    }

    #[tokio::test]
    async fn active_models_reflect_counters() {
        let mgr = manager(vec![sample_params()]);
        if let Some(counter) = mgr.active_requests.get("test") {
            counter.store(3, Ordering::SeqCst);
        }
        let models = mgr.active_models().await.expect("active models");
        assert_eq!(models[0].active_requests, 3);
    }
}

#[cfg(feature = "mock-manager")]
#[derive(Clone)]
pub struct InMemoryModelManager<C: Clock = SystemClock> {
    state: Arc<RwLock<InnerState>>, // fine for tests
    config: ManagerConfig,
    scheduler: Arc<dyn ModelScheduler>,
    clock: C,
}

#[cfg(feature = "mock-manager")]
struct InnerState {
    models: HashMap<String, ModelEntry>,
    jobs: HashMap<Uuid, JobStatus>,
}

#[cfg(feature = "mock-manager")]
struct ModelEntry {
    metadata: ModelMetadata,
    last_accessed: SystemTime,
    keep_alive_until: Instant,
    pinned: bool,
    active: u32,
    semaphore: Arc<Semaphore>,
    logs: VecDeque<StructuredLog>,
    usage: Usage,
}

#[cfg(feature = "mock-manager")]
impl<C: Clock> InMemoryModelManager<C> {
    pub fn new(config: ManagerConfig, scheduler: Arc<dyn ModelScheduler>, clock: C) -> Self {
        Self {
            state: Arc::new(RwLock::new(InnerState {
                models: HashMap::new(),
                jobs: HashMap::new(),
            })),
            config,
            scheduler,
            clock,
        }
    }

    fn ensure_loaded(&self, model: &str) -> Result<ModelMetadata, ModelManagerError> {
        self.state
            .read()
            .models
            .get(model)
            .map(|entry| entry.metadata.clone())
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))
    }

    fn update_access(&self, model: &str) {
        if let Some(entry) = self.state.write().models.get_mut(model) {
            entry.last_accessed = self.clock.now_system();
            entry.keep_alive_until = self.clock.now() + entry.metadata.keep_alive;
        }
    }

    fn prune_expired(&self) {
        let now = self.clock.now();
        let mut state = self.state.write();
        let targets: Vec<String> = state
            .models
            .iter()
            .filter_map(|(name, entry)| {
                if !entry.pinned && entry.active == 0 && entry.keep_alive_until <= now {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        for model in targets {
            state.models.remove(&model);
        }
    }

    fn add_log(entry: &mut ModelEntry, message: &str, fields: serde_json::Value, timestamp: SystemTime) {
        if entry.logs.len() >= 200 {
            entry.logs.pop_front();
        }
        entry.logs.push_back(StructuredLog {
            timestamp,
            level: "info".to_string(),
            message: message.to_string(),
            fields,
        });
    }

    fn record_usage(entry: &mut ModelEntry, usage: &Usage) {
        entry.usage.prompt_tokens = entry.usage.prompt_tokens.saturating_add(usage.prompt_tokens);
        entry.usage.completion_tokens = entry
            .usage
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        entry.usage.total_tokens = entry.usage.total_tokens.saturating_add(usage.total_tokens);
        entry.metadata.parameters.insert(
            "usage_prompt_tokens".into(),
            json!(entry.usage.prompt_tokens),
        );
        entry.metadata.parameters.insert(
            "usage_completion_tokens".into(),
            json!(entry.usage.completion_tokens),
        );
        entry.metadata.parameters.insert(
            "usage_total_tokens".into(),
            json!(entry.usage.total_tokens),
        );
    }
}

#[cfg(feature = "mock-manager")]
impl InMemoryModelManager<SystemClock> {
    /// Convenience constructor used by the CLI when running in mock mode.
    pub fn new_mock(config: ManagerConfig) -> Self {
        Self::new(config, Arc::new(NoopScheduler), SystemClock)
    }
}

#[cfg(feature = "mock-manager")]
#[async_trait]
impl<C> ModelManager for InMemoryModelManager<C>
where
    C: Clock + Send + Sync + 'static,
{
    #[instrument(skip(self))]
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse, ModelManagerError> {
        self.ensure_loaded(&req.model)?;
        self.scheduler.register_activity(&req.model).await;
        self.update_access(&req.model);
        let _permit = {
            let mut guard = self.state.write();
            let entry = guard
                .models
                .get_mut(&req.model)
                .ok_or_else(|| ModelManagerError::NotFound(req.model.clone()))?;
            let permit = entry
                .semaphore
                .clone()
                .try_acquire_owned()
                .map_err(|_| ModelManagerError::MaxParallel(req.model.clone()))?;
            entry.active += 1;
            permit
        };

        let output = if let Some(schema) = req.structured_output.clone() {
            validate_structured_output(&req.prompt, schema)
        } else {
            req.prompt.clone()
        };
        let tool_calls = if matches!(req.tool_choice, ToolChoice::Auto) {
            vec![ToolCall {
                name: "extract_metadata".to_string(),
                arguments: json!({"prompt_length": req.prompt.len()}),
                id: Uuid::new_v4().to_string(),
            }]
        } else {
            vec![]
        };
        let prompt_tokens = (req.prompt.len() / 4) as u32 + 1;
        let completion_tokens = (output.len() / 4) as u32 + 1;
        let usage = Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        };
        let response = GenerateResponse {
            output,
            finish_reason: FinishReason::Stop,
            usage: usage.clone(),
            tool_calls,
        };

        {
            let mut guard = self.state.write();
            if let Some(entry) = guard.models.get_mut(&req.model) {
                entry.active = entry.active.saturating_sub(1);
                Self::add_log(
                    entry,
                    "generate",
                    json!({"length": response.output.len()}),
                    self.clock.now_system(),
                );
                Self::record_usage(entry, &usage);
            }
        }
        self.prune_expired();
        Ok(response)
    }

    async fn generate_stream(
        &self,
        mut req: GenerateRequest,
    ) -> Result<CompletionStreamWrapper, ModelManagerError> {
        req.stream = false;
        let model = req.model.clone();
        let response = self.generate(req).await?;
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let _ = tx
            .send(mock_completion_chunk(&model, &response.output))
            .await;
        let _ = tx
            .send(mock_completion_done(&model, &response))
            .await;
        Ok(CompletionStreamWrapper::new(
            Uuid::new_v4().to_string(),
            rx,
            None,
            None,
        ))
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ModelManagerError> {
        self.ensure_loaded(&req.model)?;
        self.scheduler.register_activity(&req.model).await;
        self.update_access(&req.model);
        let _permit = {
            let mut guard = self.state.write();
            let entry = guard
                .models
                .get_mut(&req.model)
                .ok_or_else(|| ModelManagerError::NotFound(req.model.clone()))?;
            let permit = entry
                .semaphore
                .clone()
                .try_acquire_owned()
                .map_err(|_| ModelManagerError::MaxParallel(req.model.clone()))?;
            entry.active += 1;
            permit
        };
        let text = req
            .messages
            .iter()
            .filter_map(|m| match &m.content.first() {
                Some(ChatContent::Text { text }) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" \n");
        let message = ChatMessage {
            role: MessageRole::Assistant,
            content: vec![ChatContent::Text {
                text: format!("Echo: {}", text),
            }],
            tool_calls: Vec::new(),
            name: None,
            tool_call_id: None,
        };
        let prompt_tokens = text.len() as u32;
        let completion_tokens = message
            .content
            .iter()
            .map(|c| match c {
                ChatContent::Text { text } => text.len() as u32,
                _ => 0,
            })
            .sum();
        let usage = Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        };
        {
            let mut guard = self.state.write();
            if let Some(entry) = guard.models.get_mut(&req.model) {
                entry.active = entry.active.saturating_sub(1);
                Self::add_log(
                    entry,
                    "chat",
                    json!({"messages": req.messages.len()}),
                    self.clock.now_system(),
                );
                Self::record_usage(entry, &usage);
            }
        }
        self.prune_expired();
        Ok(ChatResponse {
            message,
            usage,
            finish_reason: "stop".to_string(),
        })
    }

    async fn chat_stream(
        &self,
        mut req: ChatRequest,
    ) -> Result<ChatStreamWrapper, ModelManagerError> {
        req.stream = false;
        let model = req.model.clone();
        let response = self.chat(req).await?;
        let text = chat_text(&response.message);
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_BUFFER_SIZE);
        let _ = tx.send(mock_chat_chunk(&model, &text)).await;
        let _ = tx.send(mock_chat_done(&model, &response)).await;
        Ok(ChatStreamWrapper::new(
            Uuid::new_v4().to_string(),
            rx,
            None,
            None,
        ))
    }

    async fn embeddings(
        &self,
        req: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ModelManagerError> {
        self.ensure_loaded(&req.model)?;
        self.scheduler.register_activity(&req.model).await;
        self.update_access(&req.model);
        let texts = match req.input {
            EmbeddingInput::Single(text) => vec![text],
            EmbeddingInput::Multiple(texts) => texts,
        };
        let embeddings = texts
            .iter()
            .map(|input| fake_embedding(input, req.normalize))
            .collect::<Vec<_>>();
        let prompt_tokens = texts.iter().map(|t| t.len() as u32).sum();
        let usage = Usage {
            prompt_tokens,
            completion_tokens: 0,
            total_tokens: prompt_tokens,
        };
        let mut entry_guard = self.state.write();
        if let Some(entry) = entry_guard.models.get_mut(&req.model) {
            Self::add_log(
                entry,
                "embeddings",
                json!({"count": embeddings.len()}),
                self.clock.now_system(),
            );
            Self::record_usage(entry, &usage);
        }
        drop(entry_guard);
        self.prune_expired();
        Ok(EmbeddingResponse { embeddings, usage })
    }

    async fn load_model(&self, req: LoadModelRequest) -> Result<ModelMetadata, ModelManagerError> {
        if let Some(existing) = self
            .state
            .read()
            .models
            .get(&req.model)
            .map(|entry| entry.metadata.clone())
        {
            return Ok(existing);
        }
        loop {
            let maybe_metadata = {
                let mut state = self.state.write();
                if let Some(entry) = state.models.get(&req.model) {
                    return Ok(entry.metadata.clone());
                }
                if state.models.len() < self.config.max_loaded_models {
                    let keep_alive = req.keep_alive.unwrap_or(self.config.keep_alive_default);
                    let metadata = ModelMetadata {
                        name: req.model.clone(),
                        size_bytes: 1024 * 1024 * 4,
                        context_length: 8192,
                        quantization: Some("Q4_K_M".to_string()),
                        loaded: true,
                        keep_alive,
                        pinned: req.pinned,
                        parameters: HashMap::new(),
                    };
                    let entry = ModelEntry {
                        metadata: metadata.clone(),
                        last_accessed: self.clock.now_system(),
                        keep_alive_until: self.clock.now() + keep_alive,
                        pinned: req.pinned,
                        active: 0,
                        semaphore: Arc::new(Semaphore::new(
                            self.config.max_parallel_requests_per_model,
                        )),
                        logs: VecDeque::new(),
                        usage: Usage::default(),
                    };
                    state.models.insert(req.model.clone(), entry);
                    Some(metadata)
                } else {
                    None
                }
            };
            if let Some(metadata) = maybe_metadata {
                self.scheduler.on_model_loaded(&metadata).await;
                return Ok(metadata);
            }
            let evictions = self.scheduler.advise_evict().await;
            if evictions.is_empty() {
                return Err(ModelManagerError::Scheduler("capacity reached".into()));
            }
            let mut state = self.state.write();
            for evict in evictions {
                state.models.remove(&evict);
            }
        }
    }

    async fn unload_model(&self, model: &str) -> Result<(), ModelManagerError> {
        let removed = {
            let mut state = self.state.write();
            if let Some(entry) = state.models.get(model) {
                if entry.pinned {
                    return Err(ModelManagerError::Scheduler(format!(
                        "model {model} is pinned"
                    )));
                }
            }
            state.models.remove(model)
        };
        if removed.is_some() {
            self.scheduler.on_model_unloaded(model).await;
            Ok(())
        } else {
            Err(ModelManagerError::NotFound(model.to_string()))
        }
    }

    async fn list_models(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ModelRecord>, ModelManagerError> {
        let state = self.state.read();
        Ok(state
            .models
            .values()
            .skip(offset)
            .take(limit)
            .map(|entry| ModelRecord {
                metadata: entry.metadata.clone(),
                last_accessed: entry.last_accessed,
            })
            .collect())
    }

    async fn active_models(&self) -> Result<Vec<ActiveModel>, ModelManagerError> {
        let state = self.state.read();
        Ok(state
            .models
            .values()
            .map(|entry| ActiveModel {
                metadata: entry.metadata.clone(),
                active_requests: entry.active,
            })
            .collect())
    }

    async fn job_status(&self, id: Uuid) -> Result<JobStatus, ModelManagerError> {
        let state = self.state.read();
        state
            .jobs
            .get(&id)
            .cloned()
            .ok_or_else(|| ModelManagerError::NotFound(id.to_string()))
    }

    async fn logs(
        &self,
        model: &str,
        limit: usize,
    ) -> Result<Vec<StructuredLog>, ModelManagerError> {
        let state = self.state.read();
        let entry = state
            .models
            .get(model)
            .ok_or_else(|| ModelManagerError::NotFound(model.to_string()))?;
        Ok(entry.logs.iter().rev().take(limit).cloned().collect())
    }

    async fn submit_pull_job(&self, model: &str, source: &str) -> Result<Uuid, ModelManagerError> {
        let id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("{model}:{source}").as_bytes(),
        );
        {
            let mut state = self.state.write();
            state.jobs.insert(
                id,
                JobStatus {
                    id,
                    status: JobStatusKind::Queued,
                    progress: 0.0,
                    message: Some(format!("queued pull for {model} from {source}")),
                },
            );
        }
        let state = Arc::clone(&self.state);
        let model = model.to_string();
        let source = source.to_string();
        tokio::spawn(async move {
            {
                let mut guard = state.write();
                if let Some(job) = guard.jobs.get_mut(&id) {
                    job.status = JobStatusKind::Running;
                    job.progress = 0.4;
                    job.message = Some(format!("pulling {model} from {source}"));
                }
            }
            sleep(Duration::from_millis(25)).await;
            {
                let mut guard = state.write();
                if let Some(job) = guard.jobs.get_mut(&id) {
                    job.status = JobStatusKind::Succeeded;
                    job.progress = 1.0;
                    job.message = Some(format!("{model} pulled from {source}"));
                }
            }
        });
        Ok(id)
    }
}

#[cfg(feature = "mock-manager")]
fn usage_to_engine(usage: &Usage) -> EngineUsage {
    EngineUsage {
        prompt_tokens: usage.prompt_tokens as usize,
        completion_tokens: usage.completion_tokens as usize,
        total_tokens: usage.total_tokens as usize,
        avg_tok_per_sec: 0.0,
        avg_prompt_tok_per_sec: 0.0,
        avg_compl_tok_per_sec: 0.0,
        total_time_sec: 0.0,
        total_prompt_time_sec: 0.0,
        total_completion_time_sec: 0.0,
    }
}

#[cfg(feature = "mock-manager")]
fn mock_completion_chunk(model: &str, text: &str) -> EngineResponse {
    EngineResponse::CompletionChunk(CompletionChunkResponse {
        id: format!("mock-{model}"),
        choices: vec![CompletionChunkChoice {
            text: text.to_string(),
            index: 0,
            logprobs: None,
            finish_reason: None,
        }],
        created: 0,
        model: model.to_string(),
        system_fingerprint: "mock".into(),
        object: "text_completion".into(),
    })
}

#[cfg(feature = "mock-manager")]
fn mock_completion_done(model: &str, response: &GenerateResponse) -> EngineResponse {
    EngineResponse::CompletionDone(EngineCompletionResponse {
        id: format!("mock-{model}"),
        choices: vec![CompletionChoice {
            finish_reason: finish_reason_str(&response.finish_reason).to_string(),
            index: 0,
            text: response.output.clone(),
            logprobs: None,
        }],
        created: 0,
        model: model.to_string(),
        system_fingerprint: "mock".into(),
        object: "text_completion".into(),
        usage: usage_to_engine(&response.usage),
    })
}

#[cfg(feature = "mock-manager")]
fn chat_text(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ChatContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "mock-manager")]
fn mock_chat_chunk(model: &str, text: &str) -> EngineResponse {
    EngineResponse::Chunk(ChatCompletionChunkResponse {
        id: format!("mock-{model}"),
        choices: vec![ChunkChoice {
            finish_reason: None,
            index: 0,
            delta: Delta {
                content: Some(text.to_string()),
                role: "assistant".into(),
                tool_calls: None,
            },
            logprobs: None,
        }],
        created: 0,
        model: model.to_string(),
        system_fingerprint: "mock".into(),
        object: "chat.completion.chunk".into(),
        usage: None,
    })
}

#[cfg(feature = "mock-manager")]
fn mock_chat_done(model: &str, response: &ChatResponse) -> EngineResponse {
    EngineResponse::Done(EngineChatResponse {
        id: format!("mock-{model}"),
        choices: vec![Choice {
            finish_reason: "stop".into(),
            index: 0,
            message: EngineResponseMessage {
                content: Some(chat_text(&response.message)),
                role: "assistant".into(),
                tool_calls: None,
            },
            logprobs: None,
        }],
        created: 0,
        model: model.to_string(),
        system_fingerprint: "mock".into(),
        object: "chat.completion".into(),
        usage: usage_to_engine(&response.usage),
    })
}

#[cfg(feature = "mock-manager")]
fn fake_embedding(input: &str, normalize: bool) -> Vec<f32> {
    let mut acc = vec![0.0, 0.0, 0.0, 0.0];
    let len = acc.len();
    for (idx, byte) in input.bytes().enumerate() {
        let slot = idx % len;
        if let Some(value) = acc.get_mut(slot) {
            *value += byte as f32;
        }
    }
    if normalize {
        let norm = acc.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-8);
        acc.iter_mut().for_each(|v| *v /= norm);
    }
    acc
}

#[cfg(feature = "mock-manager")]
fn validate_structured_output(prompt: &str, schema: StructuredOutputSchema) -> String {
    let desired = json!({"prompt": prompt, "schema": schema.schema});
    serde_json::to_string(&desired).unwrap_or_else(|_| prompt.to_string())
}

const fn default_schema_retries() -> u8 {
    3
}

#[cfg(all(test, feature = "mock-manager"))]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[derive(Clone)]
    struct TestClock {
        instant: Arc<RwLock<Instant>>,
        system: Arc<RwLock<SystemTime>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                instant: Arc::new(RwLock::new(Instant::now())),
                system: Arc::new(RwLock::new(SystemTime::now())),
            }
        }

        fn advance(&self, duration: Duration) {
            *self.instant.write() += duration;
            *self.system.write() += duration;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            *self.instant.read()
        }

        fn now_system(&self) -> SystemTime {
            *self.system.read()
        }
    }

    #[derive(Clone)]
    struct TestScheduler;

    #[async_trait]
    impl ModelScheduler for TestScheduler {
        async fn on_model_loaded(&self, _metadata: &ModelMetadata) {}
        async fn on_model_unloaded(&self, _model: &str) {}
        async fn advise_evict(&self) -> Vec<String> {
            vec![]
        }
        async fn register_activity(&self, _model: &str) {}
    }

    fn manager(clock: TestClock) -> InMemoryModelManager<TestClock> {
        InMemoryModelManager::new(
            ManagerConfig {
                keep_alive_default: Duration::from_millis(10),
                max_loaded_models: 2,
                max_parallel_requests_per_model: 1,
                max_total_concurrent_requests: 100,
            },
            Arc::new(TestScheduler),
            clock,
        )
    }

    #[tokio::test]
    async fn keep_alive_expires() {
        let clock = TestClock::new();
        let manager = manager(clock.clone());
        manager
            .load_model(LoadModelRequest {
                model: "demo".into(),
                keep_alive: None,
                pinned: false,
            })
            .await
            .unwrap();
        clock.advance(Duration::from_secs(1));
        manager.prune_expired();
        assert!(manager.list_models(usize::MAX, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn structured_output_enforced() {
        let clock = TestClock::new();
        let manager = manager(clock);
        manager
            .load_model(LoadModelRequest {
                model: "demo".into(),
                keep_alive: None,
                pinned: false,
            })
            .await
            .unwrap();
        let resp = manager
            .generate(GenerateRequest {
                model: "demo".into(),
                prompt: "hello".into(),
                max_tokens: None,
                temperature: None,
                stream: false,
                keep_alive: None,
                tool_choice: ToolChoice::Auto,
                structured_output: Some(StructuredOutputSchema {
                    schema: json!({"type": "object"}),
                    max_retries: 1,
                }),
                metadata: None,
                context: None,
            })
            .await
            .unwrap();
        assert!(resp.output.contains("schema"));
    }

    #[tokio::test]
    async fn usage_recorded_in_metadata() {
        let clock = TestClock::new();
        let manager = manager(clock);
        manager
            .load_model(LoadModelRequest {
                model: "demo".into(),
                keep_alive: None,
                pinned: false,
            })
            .await
            .unwrap();
        manager
            .generate(GenerateRequest {
                model: "demo".into(),
                prompt: "hi".into(),
                max_tokens: None,
                temperature: None,
                stream: false,
                keep_alive: None,
                tool_choice: ToolChoice::Auto,
                structured_output: None,
                metadata: None,
                context: None,
            })
            .await
            .unwrap();
        let record = manager
            .list_models(usize::MAX, 0)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert!(record
            .metadata
            .parameters
            .contains_key("usage_total_tokens"));
    }

    #[tokio::test]
    async fn max_parallel_requests_respected() {
        let clock = TestClock::new();
        let manager = manager(clock);
        manager
            .load_model(LoadModelRequest {
                model: "demo".into(),
                keep_alive: None,
                pinned: false,
            })
            .await
            .unwrap();
        let permit = {
            let state = manager.state.read();
            state
                .models
                .get("demo")
                .unwrap()
                .semaphore
                .clone()
        };
        let _held = permit.try_acquire_owned().unwrap();
        let err = manager
            .generate(GenerateRequest {
                model: "demo".into(),
                prompt: "hi".into(),
                max_tokens: None,
                temperature: None,
                stream: false,
                keep_alive: None,
                tool_choice: ToolChoice::Auto,
                structured_output: None,
                metadata: None,
                context: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ModelManagerError::MaxParallel(_)));
    }

    #[tokio::test]
    async fn logs_are_capped_at_two_hundred() {
        let clock = TestClock::new();
        let manager = manager(clock);
        manager
            .load_model(LoadModelRequest {
                model: "demo".into(),
                keep_alive: None,
                pinned: false,
            })
            .await
            .unwrap();
        for _ in 0..220 {
            manager
                .generate(GenerateRequest {
                    model: "demo".into(),
                    prompt: "hi".into(),
                    max_tokens: None,
                    temperature: None,
                    stream: false,
                    keep_alive: None,
                    tool_choice: ToolChoice::Auto,
                    structured_output: None,
                    metadata: None,
                    context: None,
                })
                .await
                .unwrap();
        }
        let logs = manager.logs("demo", 500).await.unwrap();
        assert_eq!(logs.len(), 200);
    }

    #[tokio::test]
    async fn pull_job_transitions_through_states() {
        let clock = TestClock::new();
        let manager = manager(clock);
        let id = manager.submit_pull_job("demo", "s3://bucket").await.unwrap();
        assert_eq!(
            id,
            Uuid::new_v5(&Uuid::NAMESPACE_OID, b"demo:s3://bucket")
        );
        sleep(Duration::from_millis(60)).await;
        let status = manager.job_status(id).await.unwrap();
        assert_eq!(status.status, JobStatusKind::Succeeded);
        assert_eq!(status.progress, 1.0);
    }
}
