pub use agent_protocol::types::spec::{
    SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus,
};
use agent_protocol::types::task::Task;
use anyhow::Result;
use mistralrs::{Model, RequestBuilder, Tool};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};

pub mod agent;
pub mod agent_bootstrap;
pub mod agent_state;
pub mod config;
pub mod conversation;
pub mod exec_command;
pub(crate) mod execution_env;
mod llm_backend;
pub mod message_helpers;
pub mod message_types;
pub mod summary;
pub use llm_backend::{HttpBackend, LLMBackend, LocalBackend, NoneBackend};
pub mod a2a;
pub mod model_config;
pub mod orchestrator;
pub mod safety_config;
pub(crate) mod shell_session;
mod step_execution;
mod tool_dispatch;
pub use shell_session::add_writable_root;
pub mod thinking_helpers;
pub mod thinking_summarizer;
pub mod tools;
pub mod web_search;

pub use config::{get_default_niterules, initialize_config, read_system_prompt};
pub use message_helpers::{rebuild_request_builder, role_from_str};
pub use message_types::{AgentMessage, BackendConfig, BackendKind, GenerationStats};
pub use summary::{build_spec_from_goal, build_split_spec, build_split_summary, build_summary};

pub use thinking_helpers::{
    ThinkingStartDecision, analyze_thinking_start, has_visible_text, preview_thinking,
    thinking_debug_enabled, thinking_debug_log,
};

pub use exec_command::{exec_command_output_to_yaml, execute_tool_binary, execute_tool_call};
pub use execution_env::{ApplyConflict, ApplyResult, ExecutionReviewEntry};

pub(crate) fn resolve_workspace_root() -> PathBuf {
    agent_state::resolve_workspace_root()
}

pub fn resolve_tools_binary_path_for_runtime() -> Result<PathBuf> {
    agent_state::resolve_tools_binary_path_for_runtime()
}

pub use agent_state::sandbox_policy_from_config;
pub use agent_state::sandbox_policy_from_config_with_workspace;
pub use agent_state::set_workspace_root_override;
pub use shell_session::execution_mode_badge;

/// Agent instance that can be used from the TUI
#[derive(Clone)]
pub struct Agent {
    backend: Arc<Box<dyn LLMBackend>>,
    backend_kind: BackendKind,
    system_prompt: Arc<Mutex<String>>,
    tools: Arc<Mutex<Vec<Tool>>>,
    thinking_summarizer: Arc<Mutex<thinking_summarizer::ThinkingSummarizer>>,
    /// Flag to cancel current generation
    cancel_requested: Arc<AtomicBool>,
    /// Conversation history (RequestBuilder maintains all messages)
    conversation: Arc<Mutex<Option<RequestBuilder>>>,
    /// Thinking tags configuration (opening/closing tags and summary interval)
    thinking_tags: Arc<Mutex<model_config::ThinkingTags>>,
    /// Safety configuration for tool access
    safety_config: Arc<Mutex<safety_config::SafetyConfig>>,
    /// Optional working directory override for orchestration (worktree support)
    working_directory: Option<PathBuf>,
    execution_environment: Arc<Mutex<Option<execution_env::ExecutionEnvironment>>>,
    /// Display label for the currently loaded model
    model_name: Arc<Mutex<String>>,
}

impl Agent {
    fn prompt_context() -> (String, String) {
        let os_info = std::env::consts::OS;
        let os_version = if os_info == "linux" {
            std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|content| {
                    content
                        .lines()
                        .find(|line| line.starts_with("PRETTY_NAME="))
                        .map(|line| {
                            line.trim_start_matches("PRETTY_NAME=")
                                .trim_matches('"')
                                .to_string()
                        })
                })
                .unwrap_or_else(|| "Linux".to_string())
        } else {
            os_info.to_string()
        };
        let workspace_path = resolve_workspace_root().display().to_string();
        (os_version, workspace_path)
    }

    fn render_system_prompt(
        template: &str,
        os_version: &str,
        workspace_path: &str,
        model_label: &str,
        safety_mode: Option<safety_config::SafetyMode>,
    ) -> String {
        let mut result = template
            .replace("{os_version}", os_version)
            .replace("{workspace_path}", workspace_path)
            .replace("{model_name}", model_label);

        if let Some(mode) = safety_mode {
            if mode == safety_config::SafetyMode::ReadOnly {
                result = Self::filter_readonly_sections(&result);
            }
        }

        result
    }

    fn filter_readonly_sections(template: &str) -> String {
        let mut result = template.to_string();

        // Best-effort filtering: remove sections that reference tools not available in ReadOnly
        // If tags aren't present (user edited them out), silently skip

        // Remove <making_code_changes> section entirely - it references edit_file and shell commands
        if let Some(start) = result.find("<making_code_changes>") {
            if let Some(end) = result.find("</making_code_changes>") {
                // Also capture the trailing newline if present
                let end = result[end..]
                    .find('\n')
                    .map(|i| end + i + 1)
                    .unwrap_or(end + "</making_code_changes>".len());
                result.drain(start..end);
            }
        }

        result.trim().to_string()
    }

    fn label_from_filename(model_filename: &str) -> String {
        std::path::Path::new(model_filename)
            .file_stem()
            .and_then(OsStr::to_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| model_filename.to_string())
    }

    fn model_label_from_backend(backend_config: &BackendConfig) -> String {
        match backend_config {
            BackendConfig::None => String::new(),
            BackendConfig::Local {
                model_path,
                model_files,
            } => model_files
                .first()
                .map(|filename| Self::label_from_filename(filename))
                .or_else(|| {
                    std::path::Path::new(model_path)
                        .file_stem()
                        .and_then(OsStr::to_str)
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "local model".to_string()),
            BackendConfig::Http { model, .. } => model.clone(),
        }
    }

    async fn regenerate_system_prompt(&self, suffix: Option<String>) -> Result<()> {
        let (os_version, workspace_path) = Self::prompt_context();
        let system_prompt_template =
            read_system_prompt().unwrap_or_else(|_e| get_default_niterules());
        let model_label = { self.model_name.lock().await.clone() };
        let safety_mode = {
            let safety_guard = self.safety_config.lock().await;
            safety_guard.mode
        };
        let mut prompt = Self::render_system_prompt(
            &system_prompt_template,
            &os_version,
            &workspace_path,
            &model_label,
            Some(safety_mode),
        );
        if let Some(s) = suffix {
            prompt.push_str(&s);
        }
        let mut system_prompt_guard = self.system_prompt.lock().await;
        *system_prompt_guard = prompt;
        Ok(())
    }
    /// Load thinking tag configuration from disk if available.
    /// Falls back to default `<think>`/`</think>` tags when no config is present.
    fn load_thinking_tags(_model_path: &str, model_filename: &str) -> model_config::ThinkingTags {
        let filename_stem = std::path::Path::new(model_filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(model_filename);

        let name_variants = vec![
            filename_stem.to_lowercase(),
            filename_stem.to_string(),
            filename_stem
                .to_lowercase()
                .split('-')
                .take_while(|s| !s.starts_with('q') || s.len() > 2)
                .collect::<Vec<_>>()
                .join("-"),
        ];

        for variant in &name_variants {
            let (has_thinking, tags) =
                model_config::ModelConfig::load_or_detect(variant, model_filename);
            if has_thinking {
                return tags;
            }
        }

        let (_, tags) = model_config::ModelConfig::load_or_detect("", model_filename);
        tags
    }

    /// Create a new agent instance with a specific backend configuration
    pub fn new_with_backend(
        backend_config: BackendConfig,
        system_prompt: String,
        tools: Vec<Tool>,
        safety_config: safety_config::SafetyConfig,
        model_label: String,
    ) -> Self {
        // Create backend based on config
        let (backend, backend_kind, thinking_tags): (
            Arc<Box<dyn LLMBackend>>,
            BackendKind,
            model_config::ThinkingTags,
        ) = match backend_config {
            BackendConfig::None => {
                let backend: Arc<Box<dyn LLMBackend>> =
                    Arc::new(Box::new(NoneBackend) as Box<dyn LLMBackend>);
                (
                    backend,
                    BackendKind::None,
                    model_config::ThinkingTags::default(),
                )
            }
            BackendConfig::Local {
                model_path,
                model_files,
            } => {
                let backend: Arc<Box<dyn LLMBackend>> = Arc::new(Box::new(LocalBackend::new(
                    model_path.clone(),
                    model_files.clone(),
                ))
                    as Box<dyn LLMBackend>);
                let thinking_tags = if !model_files.is_empty() {
                    Self::load_thinking_tags(&model_path, &model_files[0])
                } else {
                    model_config::ThinkingTags::default()
                };
                (backend, BackendKind::Local, thinking_tags)
            }
            BackendConfig::Http {
                base_url,
                api_key,
                model,
                completions_path,
                requires_model_load,
            } => {
                let thinking_tags = Self::load_thinking_tags("", &model);
                let backend: Arc<Box<dyn LLMBackend>> = Arc::new(Box::new(HttpBackend::new(
                    base_url,
                    api_key,
                    model,
                    completions_path,
                    requires_model_load,
                ))
                    as Box<dyn LLMBackend>);
                (
                    backend,
                    if requires_model_load {
                        BackendKind::Http
                    } else {
                        BackendKind::ExternalHttp
                    },
                    thinking_tags,
                )
            }
        };

        // Create thinking summarizer with configured summary interval
        let summarizer =
            thinking_summarizer::ThinkingSummarizer::with_threshold(thinking_tags.summary_interval);

        Self {
            backend,
            backend_kind,
            system_prompt: Arc::new(Mutex::new(system_prompt)),
            tools: Arc::new(Mutex::new(tools)),
            thinking_summarizer: Arc::new(Mutex::new(summarizer)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            conversation: Arc::new(Mutex::new(None)),
            thinking_tags: Arc::new(Mutex::new(thinking_tags)),
            safety_config: Arc::new(Mutex::new(safety_config)),
            working_directory: None,
            execution_environment: Arc::new(Mutex::new(None)),
            model_name: Arc::new(Mutex::new(model_label)),
        }
    }

    /// Create a clone of this agent with a different working directory.
    /// Used by the orchestrator to run steps in isolated worktrees.
    pub fn with_working_directory(&self, cwd: PathBuf) -> Self {
        Self {
            backend: self.backend.clone(),
            backend_kind: self.backend_kind,
            system_prompt: self.system_prompt.clone(),
            tools: self.tools.clone(),
            thinking_summarizer: self.thinking_summarizer.clone(),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            conversation: Arc::new(Mutex::new(None)), // Fresh conversation for worktree agent
            thinking_tags: self.thinking_tags.clone(),
            safety_config: self.safety_config.clone(),
            working_directory: Some(cwd),
            execution_environment: Arc::new(Mutex::new(None)),
            model_name: self.model_name.clone(),
        }
    }

    /// Create a new agent with a custom tool list and prompt.
    pub async fn with_tools(&self, tools: Vec<Tool>) -> Result<Self> {
        let agent = self.clone();

        {
            let mut tools_guard = agent.tools.lock().await;
            *tools_guard = tools.clone();
        }

        let safety_config = self.safety_config.lock().await.clone();
        let suffix = safety_config.get_system_prompt_suffix();
        agent.regenerate_system_prompt(suffix).await?;

        {
            let mut conversation_guard = agent.conversation.lock().await;
            *conversation_guard = None;
        }

        Ok(agent)
    }

    /// Get the effective working directory for this agent.
    /// Returns the configured working_directory if set, otherwise uses current_dir.
    pub fn effective_cwd(&self) -> PathBuf {
        self.working_directory
            .clone()
            .or_else(|| agent_state::workspace_root_override())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    pub(crate) async fn execution_cwd(&self) -> Result<PathBuf> {
        if let Some(env) = self.ensure_execution_environment().await? {
            Ok(env.private_workspace().to_path_buf())
        } else {
            Ok(self.effective_cwd())
        }
    }

    pub(crate) async fn execution_env_overrides(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .ensure_execution_environment()
            .await?
            .map(|env| env.env_overrides())
            .unwrap_or_default())
    }

    pub(crate) async fn remap_tool_arguments_for_execution(
        &self,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let Some(env) = self.ensure_execution_environment().await? else {
            return Ok(arguments.clone());
        };

        let mut remapped = arguments.clone();
        remap_tool_argument_paths(&mut remapped, &env);
        Ok(remapped)
    }

    pub(crate) async fn checkpoint_execution_after_tool(
        &self,
    ) -> Result<Option<execution_env::FsCheckpoint>> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(None);
        }
        let mut guard = self.execution_environment.lock().await;
        if let Some(env) = guard.as_mut() {
            Ok(Some(env.checkpoint_agent_fs()?))
        } else {
            Ok(None)
        }
    }

    pub async fn apply_execution_changes(&self) -> Result<Option<ApplyResult>> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(None);
        }
        let mut guard = self.execution_environment.lock().await;
        if let Some(env) = guard.as_mut() {
            Ok(Some(env.apply_to_real_workspace()?))
        } else {
            Ok(None)
        }
    }

    pub async fn discard_execution_changes(&self) -> Result<bool> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(false);
        }
        let mut guard = self.execution_environment.lock().await;
        if let Some(env) = guard.as_mut() {
            env.discard_changes()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn pending_execution_change_count(&self) -> Result<usize> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(0);
        }
        let mut guard = self.execution_environment.lock().await;
        if guard.is_none() {
            return Ok(0);
        }
        Ok(guard
            .as_mut()
            .expect("execution environment checked")
            .pending_change_count()?)
    }

    pub async fn execution_review_entries(&self) -> Result<Vec<ExecutionReviewEntry>> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(Vec::new());
        }
        let mut guard = self.execution_environment.lock().await;
        if guard.is_none() {
            return Ok(Vec::new());
        }
        Ok(guard
            .as_mut()
            .expect("execution environment checked")
            .review_entries()?)
    }

    async fn ensure_execution_environment(
        &self,
    ) -> Result<Option<execution_env::ExecutionEnvironment>> {
        if !execution_env::isolated_execution_enabled() {
            return Ok(None);
        }
        let mut guard = self.execution_environment.lock().await;
        if guard.is_none() {
            *guard = Some(execution_env::ExecutionEnvironment::initialize(
                self.effective_cwd(),
            )?);
        }
        Ok(guard.clone())
    }

    /// Create a new agent instance (legacy method, uses Local backend)
    pub fn new(
        model_path: String,
        model_files: Vec<String>,
        system_prompt: String,
        tools: Vec<Tool>,
        safety_config: safety_config::SafetyConfig,
    ) -> Self {
        let backend_config = BackendConfig::Local {
            model_path,
            model_files,
        };
        let model_label = Self::model_label_from_backend(&backend_config);

        Self::new_with_backend(
            backend_config,
            system_prompt,
            tools,
            safety_config,
            model_label,
        )
    }

    /// Create a new agent with default configuration
    pub async fn new_with_defaults() -> Result<Self> {
        Self::new_with_model(None).await
    }

    /// Create a new agent with a specific model (or default if None).
    /// When no model is configured and no backend environment is set,
    /// creates an agent with a `NoneBackend` that allows the TUI to
    /// start up and prompt the user to configure a provider.
    pub async fn new_with_model(model_filename: Option<String>) -> Result<Self> {
        // Initialize config
        if let Err(_e) = initialize_config() {
            // eprintln!("Warning: Failed to initialize config: {}", e);
        }

        // Get runtime user info
        let os_info = std::env::consts::OS;
        let os_version = if os_info == "linux" {
            std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|content| {
                    content
                        .lines()
                        .find(|line| line.starts_with("PRETTY_NAME="))
                        .map(|line| {
                            line.trim_start_matches("PRETTY_NAME=")
                                .trim_matches('"')
                                .to_string()
                        })
                })
                .unwrap_or_else(|| "Linux".to_string())
        } else {
            os_info.to_string()
        };
        let workspace_path = resolve_workspace_root().display().to_string();

        // Load safety configuration
        let safety_config = safety_config::SafetyConfig::load().unwrap_or_default();
        // Get tools based on safety mode
        let tools = if safety_config.mode == safety_config::SafetyMode::ReadOnly {
            tools::get_readonly_tools()
        } else {
            tools::get_all_tools()
        };

        let system_prompt_template = read_system_prompt().unwrap_or_else(|_e| {
            // eprintln!("Warning: Failed to read .niterules, using default: {}", e);
            get_default_niterules()
        });

        let backend_mode =
            std::env::var("NITE_BACKEND_MODE").unwrap_or_else(|_| "http".to_string());
        let backend_mode = backend_mode.to_lowercase();

        // Determine whether we actually have a usable backend configured.
        // If backend_mode is "none" or is "http" with no API key and default localhost,
        // and no model file was specified, use NoneBackend so the TUI can start.
        let has_api_key = std::env::var("NITE_HTTP_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .is_some();

        let backend_config = if backend_mode == "none"
            || (model_filename.is_none() && !has_api_key && backend_mode != "local")
        {
            BackendConfig::None
        } else {
            let model_path = "/home/wise/.config/.nite/models".to_string();
            let selected_model = model_filename
                .unwrap_or_else(|| "Qwen_Qwen3-4B-Thinking-2507-Q8_0.gguf".to_string());

            match backend_mode.as_str() {
                "local" => BackendConfig::Local {
                    model_path,
                    model_files: vec![selected_model],
                },
                "external" => {
                    let base_url = std::env::var("NITE_HTTP_BASE_URL")
                        .unwrap_or_else(|_| "https://api.openai.com".to_string());
                    let api_key = std::env::var("NITE_HTTP_API_KEY").unwrap_or_default();
                    let completions_path = std::env::var("NITE_HTTP_COMPLETIONS_PATH")
                        .unwrap_or_else(|_| "/v1/chat/completions".to_string());
                    BackendConfig::Http {
                        base_url,
                        api_key,
                        model: selected_model,
                        completions_path,
                        requires_model_load: false,
                    }
                }
                _ => {
                    let base_url = std::env::var("NITE_HTTP_BASE_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
                    let api_key = std::env::var("NITE_HTTP_API_KEY").unwrap_or_default();
                    let completions_path = std::env::var("NITE_HTTP_COMPLETIONS_PATH")
                        .unwrap_or_else(|_| "/v1/chat/completions".to_string());
                    BackendConfig::Http {
                        base_url,
                        api_key,
                        model: selected_model,
                        completions_path,
                        requires_model_load: true,
                    }
                }
            }
        };

        let model_label = Self::model_label_from_backend(&backend_config);
        let mut system_prompt = Self::render_system_prompt(
            &system_prompt_template,
            &os_version,
            &workspace_path,
            &model_label,
            Some(safety_config.mode),
        );

        if let Some(suffix) = safety_config.get_system_prompt_suffix() {
            system_prompt.push_str(&suffix);
        }

        Ok(Self::new_with_backend(
            backend_config,
            system_prompt,
            tools,
            safety_config,
            model_label,
        ))
    }

    /// Ensure backend has loaded the active model
    pub async fn initialize_backend(&self) -> Result<()> {
        self.backend.load_model().await
    }

    /// Get the model from backend (if supported)
    pub async fn get_model(&self) -> Result<Arc<Model>> {
        self.backend.get_model().await
    }

    /// Reload the model with a new model file
    /// This clears the cached model and updates the model_files to use the new file
    /// The model will be lazy-loaded on the next get_model() call
    pub async fn reload_model(&self, new_model_filename: String) -> Result<()> {
        self.backend
            .reload_model(new_model_filename.clone())
            .await?;

        if self.backend_kind == BackendKind::Local {
            let tags =
                Self::load_thinking_tags("/home/wise/.config/.nite/models", &new_model_filename);

            let mut tags_guard = self.thinking_tags.lock().await;
            *tags_guard = tags.clone();
            drop(tags_guard);

            let mut summarizer_guard = self.thinking_summarizer.lock().await;
            *summarizer_guard =
                thinking_summarizer::ThinkingSummarizer::with_threshold(tags.summary_interval);
            drop(summarizer_guard);
        } else {
            let mut tags_guard = self.thinking_tags.lock().await;
            *tags_guard = model_config::ThinkingTags::default();
            drop(tags_guard);

            let summary_interval = model_config::ThinkingTags::default().summary_interval;
            let mut summarizer_guard = self.thinking_summarizer.lock().await;
            *summarizer_guard =
                thinking_summarizer::ThinkingSummarizer::with_threshold(summary_interval);
            drop(summarizer_guard);
        }

        let model_label = Self::label_from_filename(&new_model_filename);
        {
            let mut model_name_guard = self.model_name.lock().await;
            *model_name_guard = model_label.clone();
        }
        let suffix = {
            let safety_guard = self.safety_config.lock().await;
            safety_guard.get_system_prompt_suffix()
        };
        self.regenerate_system_prompt(suffix).await?;

        Ok(())
    }

    /// Handle approval response from UI
    pub async fn handle_approval_response(&self, approved: bool) {
        if let Some(state) = shell_session::global_state() {
            let mut guard = state.pending_approval.lock().await;
            if let Some(tx) = guard.take() {
                let _ = tx.send(approved);
            }
        }
    }

    /// Request cancellation of the current generation
    pub fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
    }

    /// Reset the cancellation flag (called at the start of a new message)
    pub fn reset_cancel(&self) {
        self.cancel_requested.store(false, Ordering::SeqCst);
    }

    /// Check if cancellation was requested
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }

    /// Get the thinking tags configuration
    /// Returns the kind of backend this agent is using.
    pub fn backend_kind(&self) -> BackendKind {
        self.backend_kind
    }

    pub async fn get_thinking_tags(&self) -> model_config::ThinkingTags {
        self.thinking_tags.lock().await.clone()
    }

    /// Update the safety configuration and refresh tools based on the new mode
    pub async fn update_safety_config(
        &self,
        new_safety_config: safety_config::SafetyConfig,
    ) -> Result<()> {
        // Clone the safety config to use in multiple places
        let safety_config_for_update = new_safety_config.clone();
        // Update the tools based on the new safety mode
        let new_tools = if new_safety_config.mode == safety_config::SafetyMode::ReadOnly {
            tools::get_readonly_tools()
        } else {
            tools::get_all_tools()
        };

        // Update the safety configuration
        {
            let mut config_guard = self.safety_config.lock().await;
            *config_guard = safety_config_for_update;
        }

        if let Some(state) = shell_session::global_state() {
            let mut policy_guard = state.pending_sandbox_policy.lock().await;
            *policy_guard = sandbox_policy_from_config_with_workspace(
                &new_safety_config,
                self.execution_cwd().await?,
            );
        }

        // Update tools in the agent
        {
            let mut tools_guard = self.tools.lock().await;
            *tools_guard = new_tools.clone();
        }

        let suffix = new_safety_config.get_system_prompt_suffix();
        self.regenerate_system_prompt(suffix).await?;

        let system_prompt_content = {
            let system_prompt_guard = self.system_prompt.lock().await;
            system_prompt_guard.clone()
        };

        // Preserve conversation history while rebuilding it with the fresh system prompt.
        // This removes stale read-only/build instructions without making the model forget
        // the current task context on mode changes.
        {
            let mut conversation_guard = self.conversation.lock().await;
            if let Some(ref conversation) = *conversation_guard {
                *conversation_guard = Some(rebuild_request_builder(
                    conversation,
                    &system_prompt_content,
                    new_tools.clone(),
                    None,
                )?);
            }
        }

        Ok(())
    }

    /// Best-effort synchronous safety update for UI mode toggles.
    ///
    /// This updates the in-memory safety config and pending sandbox policy
    /// immediately so the next tool call observes the new mode even if the
    /// full async refresh has not completed yet.
    pub fn apply_safety_config_immediately(&self, new_safety_config: safety_config::SafetyConfig) {
        if let Ok(mut config_guard) = self.safety_config.try_lock() {
            *config_guard = new_safety_config.clone();
        }

        let new_tools = if new_safety_config.mode == safety_config::SafetyMode::ReadOnly {
            tools::get_readonly_tools()
        } else {
            tools::get_all_tools()
        };
        if let Ok(mut tools_guard) = self.tools.try_lock() {
            *tools_guard = new_tools;
        }

        if let Some(state) = shell_session::global_state()
            && let Ok(mut policy_guard) = state.pending_sandbox_policy.try_lock()
        {
            *policy_guard =
                sandbox_policy_from_config_with_workspace(&new_safety_config, self.effective_cwd());
        }
    }

    pub fn collect_summary(&self, task: &Task) -> TaskSummary {
        build_summary(task, None)
    }

    pub fn collect_summary_with_artifacts(
        &self,
        task: &Task,
        artifacts: Option<&[String]>,
    ) -> TaskSummary {
        build_summary(task, artifacts)
    }

    pub async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet> {
        build_split_spec(step)
    }

    pub fn synthesize_split_summary(
        task: &Task,
        step: &SpecStep,
        child_spec: &SpecSheet,
    ) -> TaskSummary {
        build_split_summary(task, step, child_spec)
    }

    /// Execute a spec step.
    ///
    /// NOTE: Full LLM integration requires refactoring sessionizer to use tokio::sync::Mutex.
    /// For now, this creates the task structure and marks it as completed to allow
    /// orchestration flow to proceed.
    ///
    /// TODO: Integrate with process_message once sessionizer Send issue is resolved.
    pub async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
        step_execution::execute_step_simple(step, spec).await
    }

    /// Execute a spec step with real LLM execution, emitting tool events.
    /// If `cancel_rx` is provided and receives a message, execution will be interrupted.
    pub async fn execute_step_with_events(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<crate::orchestrator::OrchestratorEvent>>,
    ) -> Result<Task> {
        step_execution::execute_step_with_events(self, step, spec, prefix, event_tx).await
    }

    /// Execute a spec step with real LLM execution, emitting tool events.
    /// If `cancel_rx` is provided and receives a message, execution will be interrupted.
    pub async fn execute_step_with_events_and_cancel(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<crate::orchestrator::OrchestratorEvent>>,
        cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Result<Task> {
        step_execution::execute_step_with_events_and_cancel(
            self, step, spec, prefix, event_tx, cancel_rx,
        )
        .await
    }

    /// Create a new SpecSheet from either a path to a JSON file or a goal string.
    ///
    /// If `path_or_goal` points to an existing file, the SpecSheet is loaded from that file.
    /// Otherwise, it is treated as a goal description and a new SpecSheet is generated
    /// with the goal parsed into steps (each line becomes a step).
    ///
    /// # Arguments
    /// * `path_or_goal` - Either a path to a JSON file containing a SpecSheet, or a goal description
    ///
    /// # Returns
    /// A Result containing the loaded or generated SpecSheet
    pub fn create_spec_sheet(&self, path_or_goal: &str) -> Result<SpecSheet> {
        let path = Path::new(path_or_goal);

        // If it's an existing file, try to load it
        if path.exists() && path.is_file() {
            let content = std::fs::read_to_string(path)?;
            let spec: SpecSheet = serde_json::from_str(&content)?;
            spec.validate().map_err(|e| anyhow::anyhow!("{}", e))?;
            return Ok(spec);
        }

        // Otherwise, treat it as a goal and generate a SpecSheet
        build_spec_from_goal(path_or_goal)
    }

    /// Create a new SpecSheet from a goal string asynchronously.
    /// This variant allows for more complex goal parsing in the future.
    pub async fn create_spec_sheet_async(&self, path_or_goal: &str) -> Result<SpecSheet> {
        self.create_spec_sheet(path_or_goal)
    }

    /// Validate a step index against a SpecSheet.
    /// Returns Ok(()) if the index is valid, or an error describing the issue.
    pub fn validate_step_index(&self, spec: &SpecSheet, index: &str) -> Result<()> {
        // Check if index exists in the spec steps
        let valid = spec.steps.iter().any(|step| step.index == index);
        if !valid {
            let valid_indices: Vec<&str> = spec.steps.iter().map(|s| s.index.as_str()).collect();
            return Err(anyhow::anyhow!(
                "Invalid step index '{}'. Valid indices are: {:?}",
                index,
                valid_indices
            ));
        }
        Ok(())
    }

    pub fn get_spec_status(&self, spec: &SpecSheet) -> Result<String> {
        serde_json::to_string_pretty(spec)
            .map_err(|e| anyhow::anyhow!("Failed to serialize spec: {}", e))
    }

    pub async fn update_spec_status(
        &self,
        _spec: &SpecSheet,
        _step: &SpecStep,
        _prefix: &str,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn update_task_summary(&self, _summary: &TaskSummary) -> Result<()> {
        Ok(())
    }

    pub async fn send_task_message(&self, _task_id: &str, _message: &str) -> Result<()> {
        Ok(())
    }

    pub async fn notify_step_success(&self, _summary: &TaskSummary) -> Result<()> {
        Ok(())
    }

    pub async fn close_task_channel(&self, _task_id: &str) -> Result<()> {
        Ok(())
    }
}

fn remap_tool_argument_paths(
    value: &mut serde_json::Value,
    env: &execution_env::ExecutionEnvironment,
) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    for key in ["path", "file_path"] {
        if let Some(raw) = object
            .get_mut(key)
            .and_then(|v| v.as_str().map(str::to_string))
        {
            let remapped = env.remap_workspace_path(std::path::Path::new(&raw));
            object.insert(
                key.to_string(),
                serde_json::Value::String(remapped.to_string_lossy().to_string()),
            );
        }
    }

    if let Some(paths) = object.get_mut("paths").and_then(|v| v.as_array_mut()) {
        for path in paths {
            if let Some(raw) = path.as_str() {
                let remapped = env.remap_workspace_path(std::path::Path::new(raw));
                *path = serde_json::Value::String(remapped.to_string_lossy().to_string());
            }
        }
    }
}

/// Helper function to create a simple chat session
pub async fn create_chat_session() -> Result<(Agent, mpsc::UnboundedReceiver<AgentMessage>)> {
    let agent = Agent::new_with_defaults().await?;
    let (_tx, rx) = mpsc::unbounded_channel();
    Ok((agent, rx))
}

/// Kill a background shell session by session ID
/// This is a standalone function that can be called directly from the TUI
pub async fn kill_shell_session(session_id: String) -> Result<()> {
    shell_session::ensure_global_state_initialized().await;
    let state = shell_session::global_state().unwrap();

    // Create session ID
    let session_id_obj = colossal_linux_sandbox::types::SessionId::new(session_id.clone());

    // First, try to kill all background jobs in the shell
    // This will kill the background processes before terminating the shell
    let kill_jobs_cmd = "kill $(jobs -p) 2>/dev/null || true";
    let _ = state
        .manager
        .send_input_to_shell_session(session_id_obj.clone(), kill_jobs_cmd.to_string(), None)
        .await;

    // Give it a moment to kill the jobs
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Terminate the session
    state
        .manager
        .terminate_session(session_id_obj.clone())
        .await?;

    // If this was the current shell session, clear it
    let mut session_id_lock = state.shell_session_id.lock().await;
    if let Some(current_id) = session_id_lock.as_ref() {
        if current_id.as_str() == session_id_obj.as_str() {
            *session_id_lock = None;
            let mut has_background = state.session_has_background_process.lock().await;
            *has_background = false;
        }
    }

    Ok(())
}
