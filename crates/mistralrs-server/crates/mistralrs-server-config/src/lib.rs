use std::{
    collections::{HashMap, HashSet},
    fs,
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use config::{Config, Environment, File, FileFormat};
use mistralrs_core::{
    AutoDeviceMapParams, McpClientConfig, ModelDType, ModelSelected,
    TokenSource as UpTokenSource,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;
use upstream_mistralrs_server_core::mistralrs_for_server_builder::{
    ModelConfig as UpstreamModelConfig, MistralRsForServerBuilder,
};
use validator::Validate;

/// Settings that control the HTTP server listener (env prefix `MISTRALRS__SERVER__`).
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ServerSection {
    /// TCP port to bind. Defaults to `8080`. Environment: `MISTRALRS__SERVER__PORT`.
    #[validate(range(min = 1, max = 65535))]
    pub port: u16,
    /// Interface or hostname to bind to. Defaults to `0.0.0.0`. Environment: `MISTRALRS__SERVER__HOST`.
    #[validate(length(min = 1))]
    pub host: String,
    /// TLS configuration when HTTPS is enabled. Environment: `MISTRALRS__SERVER__TLS__*`.
    pub tls: Option<TlsSection>,
    /// Number of worker threads for the Tokio runtime. Defaults to `num_cpus::get().max(4)`. Environment: `MISTRALRS__SERVER__RUNTIME_THREADS`.
    #[serde(default = "default_runtime_threads")]
    pub runtime_threads: usize,
    /// Upper bound of concurrent requests across all models. Environment: `MISTRALRS__SERVER__MAX_TOTAL_CONCURRENT_REQUESTS`.
    #[validate(range(min = 1))]
    pub max_total_concurrent_requests: usize,
    /// Additional allowed origins for CORS. Environment: `MISTRALRS__SERVER__CORS_ALLOWED_ORIGINS`.
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
}

/// Scheduler-wide knobs for model lifecycle management (env prefix `MISTRALRS__SCHEDULER__`).
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct SchedulerSection {
    /// Maximum resident models. Defaults to `3`. Environment: `MISTRALRS__SCHEDULER__MAX_LOADED_MODELS`.
    #[serde(default = "default_max_loaded_models")]
    #[validate(range(min = 1))]
    pub max_loaded_models: usize,
    /// Default keep-alive duration applied when a model is loaded. Defaults to `300s`. Environment: `MISTRALRS__SCHEDULER__KEEP_ALIVE_DEFAULT`.
    #[serde(default = "default_keep_alive")]
    #[serde(with = "humantime_serde")]
    pub keep_alive_default: std::time::Duration,
    /// Maximum concurrent requests per model. Defaults to `4`. Environment: `MISTRALRS__SCHEDULER__MAX_PARALLEL_REQUESTS_PER_MODEL`.
    #[serde(default = "default_max_parallel_requests")]
    #[validate(range(min = 1))]
    pub max_parallel_requests_per_model: usize,
    /// Eviction strategy identifier (`lru`). Environment: `MISTRALRS__SCHEDULER__EVICTION_STRATEGY`.
    #[serde(default = "default_eviction_strategy")]
    pub eviction_strategy: String,
    /// Absolute GPU memory to reserve for paged attention. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_GPU_MEM`.
    #[serde(default)]
    pub paged_attn_gpu_mem: Option<usize>,
    /// Percentage of GPU memory for paged attention. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_GPU_MEM_USAGE`.
    #[serde(default)]
    pub paged_attn_gpu_mem_usage: Option<f32>,
    /// Block size for paged attention allocations. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_BLOCK_SIZE`.
    #[serde(default)]
    pub paged_attn_block_size: Option<usize>,
    /// Enables paged attention pipeline. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN`.
    #[serde(default)]
    pub paged_attn: Option<bool>,
    /// Enables CPU backend for paged attention. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_CPU`.
    #[serde(default)]
    pub paged_attn_cpu: Option<bool>,
    /// Enables CUDA backend for paged attention. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_CUDA`.
    #[serde(default)]
    pub paged_attn_cuda: Option<bool>,
    /// Enables Metal backend for paged attention. Environment: `MISTRALRS__SCHEDULER__PAGED_ATTN_METAL`.
    #[serde(default)]
    pub paged_attn_metal: Option<bool>,
    /// Optional override for maximum sequences per batch. Environment: `MISTRALRS__SCHEDULER__MAX_SEQS`.
    #[serde(default)]
    pub max_seqs: Option<usize>,
    /// Control flag to disable KV caching. Environment: `MISTRALRS__SCHEDULER__NO_KV_CACHE`.
    #[serde(default)]
    pub no_kv_cache: Option<bool>,
    /// Prefix cache capacity. Environment: `MISTRALRS__SCHEDULER__PREFIX_CACHE_N`.
    #[serde(default)]
    pub prefix_cache_n: Option<usize>,
    /// Enables search pipeline when true. Environment: `MISTRALRS__SCHEDULER__ENABLE_SEARCH`.
    #[serde(default)]
    pub enable_search: Option<bool>,
    /// Optional BERT model used for search. Environment: `MISTRALRS__SCHEDULER__SEARCH_BERT_MODEL`.
    #[serde(default)]
    pub search_bert_model: Option<String>,
    /// Optional MCP client identifier. Environment: `MISTRALRS__SCHEDULER__MCP_CLIENT`.
    #[serde(default)]
    pub mcp_client: Option<String>,
    /// Token source description for builder integration. Environment: `MISTRALRS__SCHEDULER__TOKEN_SOURCE`.
    #[serde(default)]
    pub token_source: Option<String>,
}

/// API authentication configuration (env prefix `MISTRALRS__AUTH__`).
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AuthSection {
    /// Enables API key enforcement when set. Environment: `MISTRALRS__AUTH__ENABLED`.
    #[serde(default)]
    pub enabled: bool,
    /// Allowed API keys. Environment: `MISTRALRS__AUTH__API_KEYS`.
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// Optional rate-limit section. Environment: `MISTRALRS__AUTH__RATE_LIMIT__*`.
    #[serde(default)]
    pub rate_limit: Option<RateLimitSection>,
}

/// Fixed-window rate limit definition.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct RateLimitSection {
    /// Allowed requests per minute. Environment: `MISTRALRS__AUTH__RATE_LIMIT__REQUESTS_PER_MINUTE`.
    #[validate(range(min = 1))]
    pub requests_per_minute: u32,
}

/// Structured logging controls (env prefix `MISTRALRS__LOGGING__`).
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoggingSection {
    /// Minimum log level (default `info`). Environment: `MISTRALRS__LOGGING__LEVEL`.
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Output format string (default `json`). Environment: `MISTRALRS__LOGGING__FORMAT`.
    #[serde(default = "default_log_format")]
    pub format: String,
}

/// Per-model configuration (env prefix `MISTRALRS__MODELS__<ID>__`).
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ModelConfig {
    /// Logical ID that must match the map key. Environment: `MISTRALRS__MODELS__<ID>__MODEL_ID`.
    #[validate(length(min = 1))]
    pub model_id: String,
    /// Source location (local path or repository). Environment: `MISTRALRS__MODELS__<ID>__SOURCE`.
    #[validate(length(min = 1))]
    pub source: String,
    /// Optional tokenizer JSON override. Environment: `MISTRALRS__MODELS__<ID>__TOKENIZER_JSON`.
    #[serde(default)]
    pub tokenizer_json: Option<String>,
    /// Explicit architecture hint. Environment: `MISTRALRS__MODELS__<ID>__ARCH`.
    #[serde(default)]
    pub arch: Option<String>,
    /// Preferred dtype override. Environment: `MISTRALRS__MODELS__<ID>__DTYPE`.
    #[serde(default)]
    pub dtype: Option<String>,
    /// HuggingFace cache path. Environment: `MISTRALRS__MODELS__<ID>__HF_CACHE_PATH`.
    #[serde(default)]
    pub hf_cache_path: Option<String>,
    /// Optional MatFormer config path. Environment: `MISTRALRS__MODELS__<ID>__MATFORMER_CONFIG_PATH`.
    #[serde(default)]
    pub matformer_config_path: Option<String>,
    /// Optional MatFormer slice name. Environment: `MISTRALRS__MODELS__<ID>__MATFORMER_SLICE_NAME`.
    #[serde(default)]
    pub matformer_slice_name: Option<String>,
    /// Per-device layer definitions. Environment: `MISTRALRS__MODELS__<ID>__NUM_DEVICE_LAYERS`.
    #[serde(default)]
    pub num_device_layers: Option<Vec<String>>,
    /// Quantization setting for in-situ quantization. Environment: `MISTRALRS__MODELS__<ID>__IN_SITU_QUANT`.
    #[serde(default)]
    pub in_situ_quant: Option<String>,
    /// Overrides paged attention enablement at the model level. Environment: `MISTRALRS__MODELS__<ID>__PAGED_ATTN`.
    #[serde(default)]
    pub paged_attn: Option<bool>,
    /// Per-model paged attention block size. Environment: `MISTRALRS__MODELS__<ID>__PAGED_ATTN_BLOCK_SIZE`.
    #[serde(default)]
    pub paged_attn_block_size: Option<usize>,
    /// Context length limit for paged attention. Environment: `MISTRALRS__MODELS__<ID>__PAGED_CTXT_LEN`.
    #[serde(default)]
    pub paged_ctxt_len: Option<usize>,
    /// Absolute GPU memory for paged attention. Environment: `MISTRALRS__MODELS__<ID>__PAGED_GPU_MEM`.
    #[serde(default)]
    pub paged_gpu_mem: Option<usize>,
    /// Percentage GPU memory for paged attention. Environment: `MISTRALRS__MODELS__<ID>__PAGED_GPU_MEM_USAGE`.
    #[serde(default)]
    pub paged_gpu_mem_usage: Option<f32>,
    /// Prefix cache size override. Environment: `MISTRALRS__MODELS__<ID>__PREFIX_CACHE_N`.
    #[serde(default)]
    pub prefix_cache_n: Option<usize>,
    /// Overrides search BERT model. Environment: `MISTRALRS__MODELS__<ID>__SEARCH_BERT_MODEL`.
    #[serde(default)]
    pub search_bert_model: Option<String>,
    /// Enables semantic search per model. Environment: `MISTRALRS__MODELS__<ID>__SEARCH_ENABLED`.
    #[serde(default)]
    pub search_enabled: Option<bool>,
    /// Optional Jinja template path. Environment: `MISTRALRS__MODELS__<ID>__JINJA_TEMPLATE`.
    #[serde(default)]
    pub jinja_template: Option<String>,
    /// Literal Jinja prompt override. Environment: `MISTRALRS__MODELS__<ID>__JINJA_EXPLICIT`.
    #[serde(default)]
    pub jinja_explicit: Option<String>,
    /// Tokenizer path override. Environment: `MISTRALRS__MODELS__<ID>__TOKENIZER`.
    #[serde(default)]
    pub tokenizer: Option<String>,
    /// Tokenizer config path override. Environment: `MISTRALRS__MODELS__<ID>__TOKENIZER_CONFIG`.
    #[serde(default)]
    pub tokenizer_config: Option<String>,
    /// Tokenizer model ID override. Environment: `MISTRALRS__MODELS__<ID>__TOKENIZER_MODEL_ID`.
    #[serde(default)]
    pub tokenizer_model_id: Option<String>,
    /// Marks the default model returned by the API. Environment: `MISTRALRS__MODELS__<ID>__DEFAULT`.
    #[serde(default)]
    pub default: bool,
    /// Per-model keep-alive duration. Environment: `MISTRALRS__MODELS__<ID>__KEEP_ALIVE`.
    #[serde(default)]
    #[serde(with = "humantime_serde::option")]
    pub keep_alive: Option<Duration>,
    /// Prevents eviction when true. Environment: `MISTRALRS__MODELS__<ID>__PINNED`.
    #[serde(default)]
    pub pinned: bool,
    /// Quantization identifier. Environment: `MISTRALRS__MODELS__<ID>__QUANTIZATION`.
    #[serde(default)]
    pub quantization: Option<String>,
    /// Context window override. Environment: `MISTRALRS__MODELS__<ID>__CONTEXT_LENGTH`.
    #[serde(default)]
    pub context_length: Option<usize>,
    /// GPU placement block. Environment: `MISTRALRS__MODELS__<ID>__GPU__*`.
    #[serde(default)]
    pub gpu: Option<GpuPlacement>,
    /// Explicit GPU id list. Environment: `MISTRALRS__MODELS__<ID>__GPU_IDS`.
    #[serde(default)]
    pub gpu_ids: Option<Vec<i32>>,
    /// Per-model parallel request limit. Environment: `MISTRALRS__MODELS__<ID>__MAX_PARALLEL_REQUESTS`.
    #[serde(default)]
    pub max_parallel_requests: Option<usize>,
    /// Overrides scheduler keep-alive for the model. Environment: `MISTRALRS__MODELS__<ID>__KEEP_ALIVE_OVERRIDE`.
    #[serde(default)]
    #[serde(with = "humantime_serde::option")]
    pub keep_alive_override: Option<Duration>,
}

/// GPU placement helper reused by the builder.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct GpuPlacement {
    /// GPU identifiers the model may use.
    pub ids: Vec<u32>,
}

/// TLS certificate configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct TlsSection {
    /// Path to the PEM certificate file.
    pub cert_path: String,
    /// Path to the PEM private key.
    pub key_path: String,
}

/// Full configuration tree loaded from disk/environment.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ServerConfig {
    /// HTTP server section.
    #[serde(default = "ServerSection::default")]
    pub server: ServerSection,
    /// Scheduler section.
    #[serde(default = "SchedulerSection::default")]
    pub scheduler: SchedulerSection,
    /// Authentication section.
    #[serde(default = "AuthSection::default")]
    pub auth: AuthSection,
    /// Logging section.
    #[serde(default = "LoggingSection::default")]
    pub logging: LoggingSection,
    /// Map of configured models keyed by logical name.
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
}

impl ServerSection {
    fn default_host() -> String {
        "0.0.0.0".to_string()
    }
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            port: 8080,
            host: Self::default_host(),
            tls: None,
            runtime_threads: default_runtime_threads(),
            max_total_concurrent_requests: 64,
            cors_allowed_origins: vec![],
        }
    }
}

impl Default for SchedulerSection {
    fn default() -> Self {
        Self {
            max_loaded_models: default_max_loaded_models(),
            keep_alive_default: default_keep_alive(),
            max_parallel_requests_per_model: default_max_parallel_requests(),
            eviction_strategy: default_eviction_strategy(),
            paged_attn_gpu_mem: None,
            paged_attn_gpu_mem_usage: None,
            paged_attn_block_size: None,
            paged_attn: None,
            paged_attn_cpu: None,
            paged_attn_cuda: None,
            paged_attn_metal: None,
            max_seqs: None,
            no_kv_cache: None,
            prefix_cache_n: None,
            enable_search: None,
            search_bert_model: None,
            mcp_client: None,
            token_source: None,
        }
    }
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            enabled: false,
            api_keys: vec![],
            rate_limit: None,
        }
    }
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSection::default(),
            scheduler: SchedulerSection::default(),
            auth: AuthSection::default(),
            logging: LoggingSection::default(),
            models: HashMap::new(),
        }
    }
}

fn default_runtime_threads() -> usize {
    num_cpus::get().max(4)
}

fn default_max_loaded_models() -> usize {
    3
}

fn default_keep_alive() -> std::time::Duration {
    std::time::Duration::from_secs(300)
}

fn default_eviction_strategy() -> String {
    "lru".to_string()
}

fn default_max_parallel_requests() -> usize {
    4
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

/// Async configuration handle that supports reloads and environment overrides.
#[derive(Clone)]
pub struct ConfigManager {
    inner: Arc<RwLock<ServerConfig>>,
    source: ConfigSource,
}

/// Identifies where the configuration originated from.
#[derive(Clone)]
pub enum ConfigSource {
    /// Load from a TOML file on disk.
    Path(std::path::PathBuf),
    /// Provide the configuration inline (still subject to env overrides).
    Inline(ServerConfig),
}

impl ConfigManager {
    /// Loads the configuration and applies `MISTRALRS__*` environment overrides.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use tokio::runtime::Runtime;
    /// use mistralrs_server_config::{ConfigManager, ConfigSource, ModelConfig, ServerConfig};
    ///
    /// let rt = Runtime::new().unwrap();
    /// rt.block_on(async {
    ///     let mut cfg = ServerConfig::default();
    ///     let mut model = ModelConfig::default();
    ///     model.model_id = "demo".into();
    ///     model.source = "hf://demo".into();
    ///     cfg.models.insert("demo".into(), model);
    ///
    ///     std::env::set_var("MISTRALRS__SCHEDULER__KEEP_ALIVE_DEFAULT", "5s");
    ///     std::env::set_var("MISTRALRS__MODELS__demo__PINNED", "true");
    ///
    ///     let manager = ConfigManager::load(ConfigSource::Inline(cfg)).await.unwrap();
    ///     let loaded = manager.get().await;
    ///     assert_eq!(loaded.scheduler.keep_alive_default, Duration::from_secs(5));
    ///     assert!(loaded.models["demo"].pinned);
    ///
    ///     std::env::remove_var("MISTRALRS__SCHEDULER__KEEP_ALIVE_DEFAULT");
    ///     std::env::remove_var("MISTRALRS__MODELS__demo__PINNED");
    /// });
    /// ```
    pub async fn load(source: ConfigSource) -> Result<Self> {
        let cfg = match &source {
            ConfigSource::Path(path) => load_from_path(path)?,
            ConfigSource::Inline(cfg) => merge_inline_with_env(cfg)?,
        };
        cfg.validate()?;
        Ok(Self {
            inner: Arc::new(RwLock::new(cfg)),
            source,
        })
    }

    /// Returns the last loaded configuration snapshot.
    pub async fn get(&self) -> ServerConfig {
        self.inner.read().await.clone()
    }

    /// Re-reads configuration from the configured source, re-applying overrides.
    pub async fn reload(&self) -> Result<ServerConfig> {
        let new_cfg = match &self.source {
            ConfigSource::Path(path) => load_from_path(path)?,
            ConfigSource::Inline(cfg) => merge_inline_with_env(cfg)?,
        };
        new_cfg.validate()?;
        {
            let mut guard = self.inner.write().await;
            *guard = new_cfg.clone();
        }
        info!("config.reloaded" = true);
        Ok(new_cfg)
    }
}

impl ServerConfig {
    pub fn validate(&self) -> Result<()> {
        self.server.validate().context("invalid server section")?;
        self.scheduler
            .validate()
            .context("invalid scheduler section")?;
        self.auth.validate().context("invalid auth section")?;
        self.logging.validate().context("invalid logging section")?;
        let mut seen_ids = HashSet::new();
        let mut defaults = 0usize;
        for (name, model) in &self.models {
            model
                .validate()
                .with_context(|| format!("invalid model config {name}"))?;
            if !seen_ids.insert(model.model_id.clone()) {
                bail!("duplicate model_id {}", model.model_id);
            }
            if model.default {
                defaults += 1;
            }
            if let Some(gpu_ids) = model.effective_gpu_ids() {
                validate_gpu_ids(name, &gpu_ids)?;
            }
        }
        if defaults > 1 {
            bail!("multiple models marked as default");
        }
        let total_parallel = self
            .scheduler
            .max_loaded_models
            .saturating_mul(self.scheduler.max_parallel_requests_per_model);
        if total_parallel > self.server.max_total_concurrent_requests {
            bail!(
                "scheduler parallelism ({total_parallel}) exceeds server limit ({})",
                self.server.max_total_concurrent_requests
            );
        }
        Ok(())
    }

    /// Returns the configured default model id after validating invariants.
    pub fn default_model_id(&self) -> Result<&str> {
        let mut default_id: Option<&str> = None;
        for (key, model) in &self.models {
            if model.default {
                if key != &model.model_id {
                    bail!(
                        "default model key {key} must match model_id {}",
                        model.model_id
                    );
                }
                if default_id.replace(&model.model_id).is_some() {
                    bail!("multiple default models configured");
                }
            }
        }
        default_id.ok_or_else(|| anyhow::anyhow!("no default model configured"))
    }
}

impl ModelConfig {
    pub fn effective_gpu_ids(&self) -> Option<Vec<i32>> {
        if let Some(ids) = &self.gpu_ids {
            return Some(ids.clone());
        }
        self.gpu
            .as_ref()
            .map(|placement| placement.ids.iter().map(|id| *id as i32).collect())
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_id: String::new(),
            source: String::new(),
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
            default: false,
            keep_alive: None,
            pinned: false,
            quantization: None,
            context_length: None,
            gpu: None,
            gpu_ids: None,
            max_parallel_requests: None,
            keep_alive_override: None,
        }
    }
}

fn validate_gpu_ids(model: &str, ids: &[i32]) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if *id < 0 {
            bail!("model {model} has negative gpu id {id}");
        }
        if !seen.insert(*id) {
            bail!("model {model} has duplicate gpu id {id}");
        }
    }
    Ok(())
}

/// Loads configuration from a TOML file and applies environment overrides.
pub fn load_from_path(path: impl AsRef<Path>) -> Result<ServerConfig> {
    let path = path.as_ref();
    let builder = Config::builder()
        .add_source(File::from(path).required(true))
        .add_source(Environment::with_prefix("MISTRALRS").separator("__"));
    let cfg = builder.build().map_err(anyhow::Error::from)?;
    let cfg: ServerConfig = cfg.try_deserialize()?;
    cfg.validate()?;
    Ok(cfg)
}

fn merge_inline_with_env(cfg: &ServerConfig) -> Result<ServerConfig> {
    let inline = toml::to_string(cfg)?;
    let builder = Config::builder()
        .add_source(File::from_str(&inline, FileFormat::Toml))
        .add_source(Environment::with_prefix("MISTRALRS").separator("__"));
    let cfg = builder.build().map_err(anyhow::Error::from)?;
    let cfg: ServerConfig = cfg.try_deserialize()?;
    cfg.validate()?;
    Ok(cfg)
}

/// Source location for the tokenizer and token plumbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Load from environment variable.
    Environment(String),
    /// Read from a file on disk.
    File(PathBuf),
    /// Inline literal JSON/YAML blob.
    Literal(String),
}

impl TokenSource {
    fn parse(raw: &str) -> Result<Self> {
        if let Some(rest) = raw.strip_prefix("env:") {
            return Ok(Self::Environment(rest.to_string()));
        }
        if let Some(rest) = raw.strip_prefix("file:") {
            return Ok(Self::File(PathBuf::from(rest)));
        }
        if let Some(rest) = raw.strip_prefix("literal:") {
            return Ok(Self::Literal(rest.to_string()));
        }
        bail!("unknown token source '{raw}'")
    }

    pub fn to_upstream(&self) -> UpTokenSource {
        match self {
            TokenSource::Environment(key) => UpTokenSource::EnvVar(key.clone()),
            TokenSource::File(path) => UpTokenSource::Path(path.to_string_lossy().into_owned()),
            TokenSource::Literal(body) => UpTokenSource::Literal(body.clone()),
        }
    }
}

/// Available paged-attention backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagedAttentionBackend {
    Cpu,
    Cuda,
    Metal,
}

/// Scheduler paged-attention configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct PagedAttentionConfig {
    pub enabled: bool,
    pub backend: Option<PagedAttentionBackend>,
    pub gpu_mem: Option<usize>,
    pub gpu_mem_usage: Option<f32>,
    pub block_size: Option<usize>,
}

/// Fully realized configuration sent to the builder.
#[derive(Debug, Clone, PartialEq)]
pub struct MistralBuilderConfig {
    pub keep_alive_default: Duration,
    pub max_loaded_models: usize,
    pub max_parallel_requests_per_model: usize,
    pub eviction_strategy: String,
    pub max_seqs: Option<usize>,
    pub no_kv_cache: Option<bool>,
    pub prefix_cache_n: Option<usize>,
    pub paged_attention: Option<PagedAttentionConfig>,
    pub token_source: Option<TokenSource>,
    pub enable_search: Option<bool>,
    pub search_bert_model: Option<String>,
    pub mcp_client: Option<String>,
    pub default_model_id: String,
    pub models: Vec<ModelBuilderParams>,
}

/// Derived configuration for a single model builder invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBuilderParams {
    pub model_id: String,
    pub source: String,
    pub tokenizer_json: Option<String>,
    pub arch: Option<String>,
    pub dtype: Option<String>,
    pub hf_cache_path: Option<String>,
    pub matformer_config_path: Option<String>,
    pub matformer_slice_name: Option<String>,
    pub num_device_layers: Option<Vec<String>>,
    pub in_situ_quant: Option<String>,
    pub paged_attn: Option<bool>,
    pub paged_attn_block_size: Option<usize>,
    pub paged_ctxt_len: Option<usize>,
    pub paged_gpu_mem: Option<usize>,
    pub paged_gpu_mem_usage: Option<f32>,
    pub prefix_cache_n: Option<usize>,
    pub search_bert_model: Option<String>,
    pub search_enabled: Option<bool>,
    pub jinja_template: Option<String>,
    pub jinja_explicit: Option<String>,
    pub tokenizer: Option<String>,
    pub tokenizer_config: Option<String>,
    pub tokenizer_model_id: Option<String>,
    pub quantization: Option<String>,
    pub keep_alive: Option<Duration>,
    pub keep_alive_override: Option<Duration>,
    pub pinned: bool,
    pub context_length: Option<usize>,
    pub gpu_ids: Option<Vec<i32>>,
    pub max_parallel_requests: Option<usize>,
}

impl ModelBuilderParams {
    pub fn model_selected(&self) -> Result<ModelSelected> {
        self.build_model_selected()
    }

    pub fn to_upstream_model_config(&self) -> Result<UpstreamModelConfig> {
        let mut config = UpstreamModelConfig::new(self.model_id.clone(), self.build_model_selected()?);
        if let Some(template) = self.jinja_template.clone() {
            config = config.with_chat_template(template);
        }
        if let Some(explicit) = self.jinja_explicit.clone() {
            config = config.with_jinja_explicit(explicit);
        }
        if let Some(layers) = self.num_device_layers.clone() {
            config = config.with_num_device_layers(layers);
        }
        if let Some(isq) = self.in_situ_quant.clone() {
            config = config.with_in_situ_quant(isq);
        }
        Ok(config)
    }

    fn build_model_selected(&self) -> Result<ModelSelected> {
        match parse_model_source(&self.source)? {
            ModelSource::Toml(path) => Ok(ModelSelected::Toml { file: path }),
            ModelSource::Run(model_id) => Ok(ModelSelected::Run {
                model_id,
                tokenizer_json: self.tokenizer_json.clone(),
                dtype: parse_dtype(self.dtype.as_deref())?,
                topology: None,
                organization: None,
                write_uqff: None,
                from_uqff: None,
                imatrix: None,
                calibration_file: None,
                max_edge: None,
                max_seq_len: self
                    .context_length
                    .unwrap_or(AutoDeviceMapParams::DEFAULT_MAX_SEQ_LEN),
                max_batch_size: AutoDeviceMapParams::DEFAULT_MAX_BATCH_SIZE,
                max_num_images: None,
                max_image_length: None,
                hf_cache_path: self.hf_cache_path.as_ref().map(|value| PathBuf::from(value)),
                matformer_config_path: self
                    .matformer_config_path
                    .as_ref()
                    .map(|value| PathBuf::from(value)),
                matformer_slice_name: self.matformer_slice_name.clone(),
            }),
        }
    }
}

/// Placeholder builder artifact used in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockBuilder;

/// Factory trait that produces builders for a specific topology.
pub trait ServerBuilderFactory: Send + Sync {
    fn build_single(&self, cfg: &ModelBuilderParams) -> Result<MockBuilder>;
    fn build_multi(&self, cfgs: &[ModelBuilderParams]) -> Result<MockBuilder>;
}

pub trait BuilderShim {
    type Output;

    fn set_max_seqs(&mut self, max_seqs: Option<usize>);
    fn set_no_kv_cache(&mut self, disabled: Option<bool>);
    fn set_prefix_cache_n(&mut self, size: Option<usize>);
    fn set_token_source(&mut self, source: Option<UpTokenSource>);
    fn set_search_options(&mut self, enabled: Option<bool>, bert_model: Option<&str>);
    fn set_mcp_client(&mut self, config: Option<&McpClientConfig>);
    fn configure_paged_attention(&mut self, cfg: Option<&PagedAttentionConfig>);
    fn configure_models(&mut self, default_model_id: &str, models: &[ModelBuilderParams]) -> Result<()>;
    fn build(self) -> Result<Self::Output>;
}

#[derive(Default)]
struct RealBuilderShim {
    builder: MistralRsForServerBuilder,
}

impl BuilderShim for RealBuilderShim {
    type Output = MistralRsForServerBuilder;

    fn set_max_seqs(&mut self, max_seqs: Option<usize>) {
        if let Some(value) = max_seqs {
            self.update_builder(|builder| builder.with_max_seqs(value));
        }
    }

    fn set_no_kv_cache(&mut self, disabled: Option<bool>) {
        if let Some(flag) = disabled {
            self.update_builder(|builder| builder.with_no_kv_cache(flag));
        }
    }

    fn set_prefix_cache_n(&mut self, size: Option<usize>) {
        if let Some(value) = size {
            self.update_builder(|builder| builder.with_prefix_cache_n(value));
        }
    }

    fn set_token_source(&mut self, source: Option<UpTokenSource>) {
        if let Some(src) = source {
            self.update_builder(|builder| builder.with_token_source(src));
        }
    }

    fn set_search_options(&mut self, enabled: Option<bool>, bert_model: Option<&str>) {
        if let Some(flag) = enabled {
            self.update_builder(|builder| builder.with_enable_search(flag));
        }
        if let Some(model) = bert_model {
            let model = model.to_string();
            self.update_builder(|builder| builder.with_search_bert_model(model));
        }
    }

    fn set_mcp_client(&mut self, config: Option<&McpClientConfig>) {
        if let Some(cfg) = config {
            let cloned = cfg.clone();
            self.update_builder(|builder| builder.with_mcp_config(cloned));
        }
    }

    fn configure_paged_attention(&mut self, cfg: Option<&PagedAttentionConfig>) {
        if let Some(cfg) = cfg {
            self.update_builder(|builder| builder.set_paged_attn(Some(cfg.enabled)));
            self.update_builder(|builder| builder.with_paged_attn_gpu_mem_optional(cfg.gpu_mem));
            self.update_builder(|builder| builder.with_paged_attn_gpu_mem_usage_optional(cfg.gpu_mem_usage));
            self.update_builder(|builder| builder.with_paged_attn_block_size_optional(cfg.block_size));
        }
    }

    fn configure_models(&mut self, default_model_id: &str, models: &[ModelBuilderParams]) -> Result<()> {
        if models.len() == 1 {
            self.configure_single_model(&models[0])
        } else {
            self.configure_multi_model(default_model_id, models)
        }
    }

    fn build(self) -> Result<Self::Output> {
        Ok(self.builder)
    }
}

impl RealBuilderShim {
    fn update_builder<F>(&mut self, func: F)
    where
        F: FnOnce(MistralRsForServerBuilder) -> MistralRsForServerBuilder,
    {
        let builder = mem::take(&mut self.builder);
        self.builder = func(builder);
    }

    fn configure_single_model(&mut self, params: &ModelBuilderParams) -> Result<()> {
        let model = params.build_model_selected()?;
        self.update_builder(|builder| builder.with_model(model));
        self.apply_model_overrides(params);
        Ok(())
    }

    fn configure_multi_model(
        &mut self,
        default_model_id: &str,
        models: &[ModelBuilderParams],
    ) -> Result<()> {
        let default_id = default_model_id.to_string();
        self.update_builder(|builder| builder.with_default_model_id(default_id));
        let configs = models
            .iter()
            .map(ModelBuilderParams::to_upstream_model_config)
            .collect::<Result<Vec<_>>>()?;
        self.update_builder(|builder| builder.with_model_configs(configs));
        Ok(())
    }

    fn apply_model_overrides(&mut self, params: &ModelBuilderParams) {
        self.update_builder(|builder| builder.with_chat_template_optional(params.jinja_template.clone()));
        self.update_builder(|builder| builder.with_jinja_explicit_optional(params.jinja_explicit.clone()));
        self.update_builder(|builder| builder.with_num_device_layers_optional(params.num_device_layers.clone()));
        self.update_builder(|builder| builder.with_in_situ_quant_optional(params.in_situ_quant.clone()));
        if let Some(size) = params.prefix_cache_n {
            self.update_builder(|builder| builder.with_prefix_cache_n(size));
        }
        if let Some(flag) = params.paged_attn {
            self.update_builder(|builder| builder.set_paged_attn(Some(flag)));
        }
        self.update_builder(|builder| builder.with_paged_attn_gpu_mem_optional(params.paged_gpu_mem));
        self.update_builder(|builder| builder.with_paged_attn_gpu_mem_usage_optional(params.paged_gpu_mem_usage));
        self.update_builder(|builder| builder.with_paged_attn_block_size_optional(params.paged_attn_block_size));
        self.update_builder(|builder| builder.with_paged_ctxt_len_optional(params.paged_ctxt_len));
    }
}

enum ModelSource {
    Toml(String),
    Run(String),
}

fn parse_model_source(raw: &str) -> Result<ModelSource> {
    if let Some(rest) = raw.strip_prefix("toml://") {
        return Ok(ModelSource::Toml(rest.to_string()));
    }
    if raw.ends_with(".toml") {
        return Ok(ModelSource::Toml(raw.to_string()));
    }
    let trimmed = raw.strip_prefix("hf://").unwrap_or(raw).trim();
    if trimmed.is_empty() {
        bail!("model source '{raw}' produced empty identifier");
    }
    Ok(ModelSource::Run(trimmed.to_string()))
}

fn parse_dtype(value: Option<&str>) -> Result<ModelDType> {
    match value {
        Some(raw) => raw
            .parse()
            .map_err(|err| anyhow!("invalid dtype '{raw}': {err}")),
        None => Ok(ModelDType::Auto),
    }
}

impl MistralBuilderConfig {
    /// Dispatches to the provided factory without performing any engine work.
    pub fn to_mock_builder(&self, factory: &dyn ServerBuilderFactory) -> Result<()> {
        if self.models.len() == 1 {
            factory.build_single(&self.models[0])?;
        } else {
            factory.build_multi(&self.models)?;
        }
        Ok(())
    }

    pub fn to_builder(&self) -> Result<MistralRsForServerBuilder> {
        let mut shim = RealBuilderShim::default();
        self.apply_to_shim(&mut shim)?;
        shim.build()
    }

    fn apply_to_shim<S: BuilderShim>(&self, shim: &mut S) -> Result<()> {
        shim.set_max_seqs(self.max_seqs);
        shim.set_no_kv_cache(self.no_kv_cache);
        shim.set_prefix_cache_n(self.prefix_cache_n);
        shim.set_token_source(self.token_source.as_ref().map(TokenSource::to_upstream));
        shim.set_search_options(self.enable_search, self.search_bert_model.as_deref());

        let mcp_config = self
            .mcp_client
            .as_ref()
            .map(|spec| parse_mcp_client_config(spec))
            .transpose()?;
        shim.set_mcp_client(mcp_config.as_ref());

        shim.configure_paged_attention(self.paged_attention.as_ref());
        shim.configure_models(&self.default_model_id, &self.models)
    }
}

impl TryFrom<&ServerConfig> for MistralBuilderConfig {
    type Error = anyhow::Error;

    fn try_from(value: &ServerConfig) -> Result<Self> {
        value.validate()?;
        if value.models.is_empty() {
            bail!("at least one model must be configured");
        }
        let default_model_id = value.default_model_id()?.to_string();
        let token_source = match &value.scheduler.token_source {
            Some(raw) => Some(TokenSource::parse(raw)?),
            None => None,
        };
        let backend = detect_paged_backend(&value.scheduler)?;
        let paged_attention = value.scheduler.paged_attn.map(|enabled| PagedAttentionConfig {
            enabled,
            backend: backend.clone(),
            gpu_mem: value.scheduler.paged_attn_gpu_mem,
            gpu_mem_usage: value.scheduler.paged_attn_gpu_mem_usage,
            block_size: value.scheduler.paged_attn_block_size,
        });
        let mut models = Vec::with_capacity(value.models.len());
        let mut seen = HashSet::new();
        for (name, cfg) in value.models.iter() {
            if name != &cfg.model_id {
                bail!("model entry key {name} must match model_id {}", cfg.model_id);
            }
            if !seen.insert(cfg.model_id.clone()) {
                bail!("duplicate model id {}", cfg.model_id);
            }
            let params = ModelBuilderParams {
                model_id: cfg.model_id.clone(),
                source: cfg.source.clone(),
                tokenizer_json: cfg.tokenizer_json.clone(),
                arch: cfg.arch.clone(),
                dtype: cfg.dtype.clone(),
                hf_cache_path: cfg.hf_cache_path.clone(),
                matformer_config_path: cfg.matformer_config_path.clone(),
                matformer_slice_name: cfg.matformer_slice_name.clone(),
                num_device_layers: cfg.num_device_layers.clone(),
                in_situ_quant: cfg.in_situ_quant.clone(),
                paged_attn: cfg.paged_attn,
                paged_attn_block_size: cfg.paged_attn_block_size,
                paged_ctxt_len: cfg.paged_ctxt_len,
                paged_gpu_mem: cfg.paged_gpu_mem,
                paged_gpu_mem_usage: cfg.paged_gpu_mem_usage,
                prefix_cache_n: cfg.prefix_cache_n,
                search_bert_model: cfg.search_bert_model.clone(),
                search_enabled: cfg.search_enabled,
                jinja_template: cfg.jinja_template.clone(),
                jinja_explicit: cfg.jinja_explicit.clone(),
                tokenizer: cfg.tokenizer.clone(),
                tokenizer_config: cfg.tokenizer_config.clone(),
                tokenizer_model_id: cfg.tokenizer_model_id.clone(),
                quantization: cfg.quantization.clone(),
                keep_alive: cfg.keep_alive,
                keep_alive_override: cfg.keep_alive_override,
                pinned: cfg.pinned,
                context_length: cfg.context_length,
                gpu_ids: cfg.effective_gpu_ids(),
                max_parallel_requests: cfg.max_parallel_requests,
            };
            if let Some(ids) = params.gpu_ids.as_ref() {
                validate_gpu_ids(name, ids)?;
            }
            models.push(params);
        }
        Ok(Self {
            keep_alive_default: value.scheduler.keep_alive_default,
            max_loaded_models: value.scheduler.max_loaded_models,
            max_parallel_requests_per_model: value.scheduler.max_parallel_requests_per_model,
            eviction_strategy: value.scheduler.eviction_strategy.clone(),
            max_seqs: value.scheduler.max_seqs,
            no_kv_cache: value.scheduler.no_kv_cache,
            prefix_cache_n: value.scheduler.prefix_cache_n,
            paged_attention,
            token_source,
            enable_search: value.scheduler.enable_search,
            search_bert_model: value.scheduler.search_bert_model.clone(),
            mcp_client: value.scheduler.mcp_client.clone(),
            default_model_id,
            models,
        })
    }
}

fn detect_paged_backend(section: &SchedulerSection) -> Result<Option<PagedAttentionBackend>> {
    let mut backend: Option<PagedAttentionBackend> = None;
    let mut consider = |flag: Option<bool>, variant: PagedAttentionBackend| -> Result<()> {
        if flag.unwrap_or(false) {
            if backend.is_some() {
                bail!("multiple paged attention backends enabled simultaneously");
            }
            backend = Some(variant);
        }
        Ok(())
    };
    consider(section.paged_attn_cpu, PagedAttentionBackend::Cpu)?;
    consider(section.paged_attn_cuda, PagedAttentionBackend::Cuda)?;
    consider(section.paged_attn_metal, PagedAttentionBackend::Metal)?;
    Ok(backend)
}

fn parse_mcp_client_config(spec: &str) -> Result<McpClientConfig> {
    if let Some(path) = spec.strip_prefix("file:") {
        let body = fs::read_to_string(path)
            .with_context(|| format!("failed to read MCP config from {path}"))?;
        return parse_mcp_payload(&body);
    }
    if let Some(body) = spec.strip_prefix("literal:") {
        return parse_mcp_payload(body);
    }
    parse_mcp_payload(spec)
}

fn parse_mcp_payload(body: &str) -> Result<McpClientConfig> {
    serde_json::from_str(body).or_else(|json_err| {
        toml::from_str(body).map_err(|toml_err| {
            anyhow::anyhow!(
                "failed to parse MCP config as JSON ({json_err}) or TOML ({toml_err})"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct RecordingShim {
        max_seqs: Option<usize>,
        no_kv_cache: Option<bool>,
        prefix_cache_n: Option<usize>,
        token_source: Option<String>,
        search_enabled: Option<bool>,
        search_model: Option<String>,
        mcp: bool,
        paged_attn_called: bool,
        models_called: Option<usize>,
        default_model_id: Option<String>,
    }

    impl BuilderShim for RecordingShim {
        type Output = ();

        fn set_max_seqs(&mut self, max_seqs: Option<usize>) {
            self.max_seqs = max_seqs;
        }

        fn set_no_kv_cache(&mut self, disabled: Option<bool>) {
            self.no_kv_cache = disabled;
        }

        fn set_prefix_cache_n(&mut self, size: Option<usize>) {
            self.prefix_cache_n = size;
        }

        fn set_token_source(&mut self, source: Option<UpTokenSource>) {
            self.token_source = source.map(|value| value.to_string());
        }

        fn set_search_options(&mut self, enabled: Option<bool>, bert_model: Option<&str>) {
            self.search_enabled = enabled;
            self.search_model = bert_model.map(|value| value.to_string());
        }

        fn set_mcp_client(&mut self, config: Option<&McpClientConfig>) {
            self.mcp = config.is_some();
        }

        fn configure_paged_attention(&mut self, cfg: Option<&PagedAttentionConfig>) {
            self.paged_attn_called = cfg.is_some();
        }

        fn configure_models(
            &mut self,
            default_model_id: &str,
            models: &[ModelBuilderParams],
        ) -> Result<()> {
            self.models_called = Some(models.len());
            self.default_model_id = Some(default_model_id.to_string());
            Ok(())
        }

        fn build(self) -> Result<Self::Output> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn config_roundtrip() {
        let toml = r#"
            [server]
            port = 9000
            host = "127.0.0.1"
            max_total_concurrent_requests = 32

            [scheduler]
            max_loaded_models = 2

            [models."mistral-7b"]
            model_id = "mistral-7b"
            source = "hf://mistral-ai/mistral-7b"
            default = true
            pinned = true
            keep_alive = "60s"
            gpu_ids = [0]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        cfg.validate().expect("valid config");
        let manager = ConfigManager::load(ConfigSource::Inline(cfg.clone()))
            .await
            .expect("manager");
        let fetched = manager.get().await;
        assert_eq!(fetched.server.port, 9000);
    }

    #[test]
    fn builder_shim_applies_single_model_settings() {
        let toml = r#"
            [scheduler]
            token_source = "env:HF_TOKEN"
            max_seqs = 8
            no_kv_cache = true
            prefix_cache_n = 3
            paged_attn = true
            paged_attn_gpu_mem = 1024
            enable_search = true
            search_bert_model = "bge-small"
            mcp_client = "literal:{\"servers\":[],\"auto_register_tools\":true}"

            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid config");
        let builder = MistralBuilderConfig::try_from(&cfg).expect("builder config");
        let mut shim = RecordingShim::default();
        builder.apply_to_shim(&mut shim).expect("apply to shim");
        assert_eq!(shim.max_seqs, Some(8));
        assert_eq!(shim.no_kv_cache, Some(true));
        assert_eq!(shim.prefix_cache_n, Some(3));
        assert_eq!(shim.token_source.as_deref(), Some("env:HF_TOKEN"));
        assert_eq!(shim.search_enabled, Some(true));
        assert_eq!(shim.search_model.as_deref(), Some("bge-small"));
        assert!(shim.mcp);
        assert!(shim.paged_attn_called);
        assert_eq!(shim.models_called, Some(1));
    }

    #[test]
    fn builder_shim_tracks_multi_model_mode() {
        let toml = r#"
            [scheduler]
            token_source = "env:TOK"

            [models.alpha]
            model_id = "alpha"
            source = "hf://alpha"
            default = true

            [models.beta]
            model_id = "beta"
            source = "hf://beta"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid config");
        let builder = MistralBuilderConfig::try_from(&cfg).expect("builder config");
        let mut shim = RecordingShim::default();
        builder.apply_to_shim(&mut shim).expect("apply to shim");
        assert_eq!(shim.models_called, Some(2));
        assert_eq!(shim.default_model_id.as_deref(), Some("alpha"));
    }

    #[test]
    fn duplicate_model_ids_rejected() {
        let toml = r#"
            [models.a]
            model_id = "duplicate"
            source = "hf://demo/a"

            [models.b]
            model_id = "duplicate"
            source = "hf://demo/b"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn builder_config_uses_default_flag() {
        let toml = r#"
            [models.alpha]
            model_id = "alpha"
            source = "hf://alpha"

            [models.beta]
            model_id = "beta"
            source = "hf://beta"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        let builder = MistralBuilderConfig::try_from(&cfg).expect("builder config");
        assert_eq!(builder.default_model_id, "beta");
        let ids: Vec<_> = builder.models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"alpha"));
    }

    #[test]
    fn scheduler_limits_checked() {
        let toml = r#"
            [server]
            port = 8080
            host = "0.0.0.0"
            max_total_concurrent_requests = 1

            [scheduler]
            max_loaded_models = 2
            max_parallel_requests_per_model = 2

            [models.demo]
            model_id = "demo"
            source = "hf://demo"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn default_model_id_helper_returns_id() {
        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert_eq!(cfg.default_model_id().unwrap(), "demo");
    }

    #[test]
    fn default_model_id_helper_errors_without_default() {
        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert!(cfg.default_model_id().is_err());
    }

    #[test]
    fn default_model_id_helper_errors_with_multiple_defaults() {
        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true

            [models.other]
            model_id = "other"
            source = "hf://other"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert!(cfg.default_model_id().is_err());
    }

    #[test]
    fn default_model_id_helper_checks_key_matches() {
        let toml = r#"
            [models.fake]
            model_id = "real"
            source = "hf://demo"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("valid");
        assert!(cfg.default_model_id().is_err());
    }

    #[test]
    fn builder_config_preserves_single_model_defaults() {
        let toml = r#"
            [scheduler]
            keep_alive_default = "30s"

            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let builder = MistralBuilderConfig::try_from(&cfg).unwrap();
        assert_eq!(builder.models.len(), 1);
        assert_eq!(builder.default_model_id, "demo");
        assert_eq!(builder.keep_alive_default, Duration::from_secs(30));
    }

    #[test]
    fn builder_config_handles_multi_model_flags() {
        let toml = r#"
            [scheduler]
            token_source = "env:TOKENS"

            [models.primary]
            model_id = "primary"
            source = "hf://primary"
            default = true
            pinned = true

            [models.secondary]
            model_id = "secondary"
            source = "hf://secondary"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let builder = MistralBuilderConfig::try_from(&cfg).unwrap();
        assert!(matches!(builder.token_source, Some(TokenSource::Environment(key)) if key == "TOKENS"));
        assert_eq!(builder.models.len(), 2);
        assert!(builder.models.iter().any(|m| m.pinned && m.model_id == "primary"));
    }

    #[test]
    fn builder_config_rejects_negative_gpu_ids() {
        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true
            gpu_ids = [-1]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        assert!(MistralBuilderConfig::try_from(&cfg).is_err());
    }

    #[test]
    fn token_source_parsing_variants() {
        assert!(matches!(TokenSource::parse("env:KEY").unwrap(), TokenSource::Environment(key) if key == "KEY"));
        assert!(matches!(TokenSource::parse("file:/tmp/a").unwrap(), TokenSource::File(path) if path == PathBuf::from("/tmp/a")));
        assert!(matches!(TokenSource::parse("literal:{}").unwrap(), TokenSource::Literal(body) if body == "{}"));
    }

    #[test]
    fn builder_factory_dispatches_correctly() {
        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let builder = MistralBuilderConfig::try_from(&cfg).unwrap();
        let recorder = RecordingFactory::default();
        builder.to_mock_builder(&recorder).unwrap();
        assert_eq!(recorder.single_count(), 1);

        let toml = r#"
            [models.demo]
            model_id = "demo"
            source = "hf://demo"
            default = true

            [models.other]
            model_id = "other"
            source = "hf://other"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let builder = MistralBuilderConfig::try_from(&cfg).unwrap();
        let recorder = RecordingFactory::default();
        builder.to_mock_builder(&recorder).unwrap();
        assert_eq!(recorder.multi_count(), 1);
    }

    #[derive(Default, Clone)]
    struct RecordingFactory {
        single: Arc<Mutex<u32>>,
        multi: Arc<Mutex<u32>>,
    }

    impl RecordingFactory {
        fn single_count(&self) -> u32 {
            *self.single.lock().unwrap()
        }

        fn multi_count(&self) -> u32 {
            *self.multi.lock().unwrap()
        }
    }

    impl ServerBuilderFactory for RecordingFactory {
        fn build_single(&self, cfg: &ModelBuilderParams) -> Result<MockBuilder> {
            assert!(!cfg.model_id.is_empty());
            let mut guard = self.single.lock().unwrap();
            *guard += 1;
            Ok(MockBuilder)
        }

        fn build_multi(&self, cfgs: &[ModelBuilderParams]) -> Result<MockBuilder> {
            assert!(cfgs.len() >= 2);
            let mut guard = self.multi.lock().unwrap();
            *guard += 1;
            Ok(MockBuilder)
        }
    }
}
