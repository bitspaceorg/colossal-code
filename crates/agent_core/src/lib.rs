use anyhow::Result;
use mistralrs::{
    ChatCompletionChunkResponse, Delta, GgufModelBuilder, RequestBuilder, Response,
    TextMessageRole, Tool, ToolCallResponse, ToolChoice, Model,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, Mutex};
use serde_json::{json, Value};
use serde::Serialize;
use tokenizers::Tokenizer;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::types::{ExitStatus, SessionId};
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use once_cell::sync::OnceCell;

pub mod config;
pub mod tools;
pub mod web_search;
pub mod thinking_summarizer;

// Global state for persistent shell session
struct GlobalState {
    manager: Arc<colossal_linux_sandbox::manager::SessionManager>,
    shell_session_id: tokio::sync::Mutex<Option<colossal_linux_sandbox::types::SessionId>>,
    shell: colossal_linux_sandbox::shell::Shell,
    sandbox_policy: SandboxPolicy,
    // Track if current session has a background process running
    session_has_background_process: tokio::sync::Mutex<bool>,
}

static GLOBAL_STATE: OnceCell<GlobalState> = OnceCell::new();

#[derive(Serialize)]
struct ExecCommandResult {
    command: String,
    status: String,
    cmd_out: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Serialize)]
struct WebSearchResult {
    status: String,
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    results: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct HtmlToTextResult {
    status: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn execute_tool_binary(args: Vec<String>, sandbox_policy: &SandboxPolicy) -> Result<String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let output = execute_tools_with_sandbox(args, sandbox_policy, cwd).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Tool execution failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn ensure_global_state_initialized() {
    if GLOBAL_STATE.get().is_none() {
        let shell = colossal_linux_sandbox::shell::default_user_shell().await;
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));

        let mut writable_roots = vec![
            colossal_linux_sandbox::protocol::WritableRoot {
                root: workspace_path.clone(),
                recursive: true,
                read_only_subpaths: vec![],
            },
        ];

        if let Some(parent) = workspace_path.parent() {
            writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                root: parent.to_path_buf(),
                recursive: true,
                read_only_subpaths: vec![],
            });
        }

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_parent) = exe_path.parent().and_then(|p| p.parent()) {
                writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                    root: exe_parent.to_path_buf(),
                    recursive: true,
                    read_only_subpaths: vec![],
                });
            }
        }

        if let Ok(extra_roots) = std::env::var("SANDBOX_EXTRA_ROOTS") {
            for root_path in extra_roots.split(':') {
                if !root_path.is_empty() {
                    writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                        root: std::path::PathBuf::from(root_path),
                        recursive: true,
                        read_only_subpaths: vec![],
                    });
                }
            }
        }

        let sandbox_policy = if std::env::var("SAFE_MODE").is_ok() {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access: colossal_linux_sandbox::protocol::NetworkAccess::Enabled,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }
        } else {
            SandboxPolicy::DangerFullAccess
        };

        let _ = GLOBAL_STATE.set(GlobalState {
            manager: Arc::new(colossal_linux_sandbox::manager::SessionManager::default()),
            shell_session_id: tokio::sync::Mutex::new(None),
            shell,
            sandbox_policy,
            session_has_background_process: tokio::sync::Mutex::new(false),
        });
    }
}

async fn get_or_create_shell_session() -> Result<(Arc<colossal_linux_sandbox::manager::SessionManager>, colossal_linux_sandbox::types::SessionId)> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE.get().unwrap();
    let mut session_id_lock = state.shell_session_id.lock().await;
    let has_background = state.session_has_background_process.lock().await;

    // Create new session if:
    // 1. No session exists yet, OR
    // 2. Current session has a background process running
    if session_id_lock.is_none() || *has_background {
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));

        let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
            workspace_path.clone()
        ));

        let session_id = state.manager.create_persistent_shell_session(
            state.shell.path().to_string_lossy().to_string(),
            false,
            state.sandbox_policy.clone(),
            shared_state,
            None,
        ).await?;

        let _ = state.manager.exec_command_in_shell_session(
            session_id.clone(),
            format!("cd {}", workspace_path.display()),
            Some(5000),
            1000,
        ).await;

        *session_id_lock = Some(session_id.clone());
    }

    Ok((state.manager.clone(), session_id_lock.clone().unwrap()))
}

async fn execute_tool_call(tool_call: &ToolCallResponse) -> Result<String> {
    ensure_global_state_initialized().await;

    let name = &tool_call.function.name;
    let arguments: Value = serde_json::from_str(&tool_call.function.arguments)?;

    match name.as_str() {
        "exec_command" => {
            let state = GLOBAL_STATE.get().unwrap();
            let command = arguments["command"].as_str().unwrap_or("");
            let is_background = arguments.get("is_background").and_then(|v| v.as_bool()).unwrap_or(false);

            // Get or create shell session (will create new one if current has background process)
            let (manager, session_id) = get_or_create_shell_session().await?;

            if is_background {
                // Mark session as busy BEFORE starting the command
                let mut has_background = state.session_has_background_process.lock().await;
                *has_background = true;
                drop(has_background); // Release lock

                // Create log file for background output
                let log_file_path = format!("/tmp/shell_{}.log", session_id.as_str());

                // Run command in background with output redirected to log file
                // Strip trailing & if present since we'll add it with redirection
                let command_clean = command.trim_end().trim_end_matches('&').trim_end();
                let bg_command = format!("{} > {} 2>&1 &", command_clean, log_file_path);
                manager.send_input_to_shell_session(
                    session_id.clone(),
                    bg_command,
                ).await?;

                let exec_result = serde_json::json!({
                    "command": command,
                    "status": "Background",
                    "session_id": session_id.as_str(),
                    "log_file": log_file_path,
                    "message": format!("Command started in background. Session ID: {}. Log file: {}", session_id.as_str(), log_file_path)
                });
                Ok(serde_yaml::to_string(&exec_result)?)
            } else {
                // Foreground command - wait for completion
                let result = manager.exec_command_in_shell_session(
                    session_id.clone(),
                    command.to_string(),
                    Some(5000),
                    1000,
                ).await?;

                let is_success = matches!(result.exit_status, ExitStatus::Completed { code } if code == 0);
                let exec_result = ExecCommandResult {
                    command: command.to_string(),
                    status: if is_success { "Success".to_string() } else { "Failure".to_string() },
                    cmd_out: result.aggregated_output,
                    message: if is_success { None } else { Some(format!("{:?}", result.exit_status)) },
                };
                Ok(serde_yaml::to_string(&exec_result)?)
            }
        },
        "read_output" => {
            let state = GLOBAL_STATE.get().unwrap();
            let session_id_str = arguments["session_id"].as_str().unwrap_or("");
            let session_id = SessionId::new(session_id_str.to_string());

            match state.manager.read_background_output(session_id).await {
                Ok(output) => {
                    let result = serde_json::json!({
                        "status": "Success",
                        "session_id": session_id_str,
                        "output": output
                    });
                    Ok(serde_yaml::to_string(&result)?)
                },
                Err(e) => {
                    let result = serde_json::json!({
                        "status": "Failure",
                        "session_id": session_id_str,
                        "error": format!("Failed to read output: {}", e)
                    });
                    Ok(serde_yaml::to_string(&result)?)
                }
            }
        },
        "read_file" | "delete_path" | "delete_many" | "get_files" | "get_files_recursive" | "search_files_with_regex" | "edit_file" | "semantic_search" => {
            let state = GLOBAL_STATE.get().unwrap();
            let mut args = vec![name.to_string()];

            // Build args based on tool type
            match name.as_str() {
                "read_file" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    let should_read_entire = arguments["should_read_entire_file"].as_bool().unwrap_or(true);
                    args.push(path.to_string());
                    if should_read_entire {
                        args.push("entire".to_string());
                    }
                },
                "delete_path" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    args.push(path.to_string());
                },
                "get_files" => {
                    let path = arguments["path"].as_str().unwrap_or(".");
                    let limit = arguments["limit"].as_u64().map(|l| l.to_string()).unwrap_or_else(|| "100".to_string());
                    args.push(path.to_string());
                    args.push(limit);
                },
                "get_files_recursive" => {
                    let path = arguments["path"].as_str().unwrap_or(".");
                    args.push(path.to_string());

                    // Add limit if provided
                    if let Some(limit) = arguments.get("limit").and_then(|v| v.as_u64()) {
                        args.push(limit.to_string());
                    }

                    // Add include patterns if provided
                    if let Some(patterns) = arguments.get("include_patterns").and_then(|v| v.as_array()) {
                        for pattern in patterns {
                            if let Some(p) = pattern.as_str() {
                                args.push(p.to_string());
                            }
                        }
                    }

                    // Add exclude patterns with --exclude flag if provided
                    if let Some(patterns) = arguments.get("exclude_patterns").and_then(|v| v.as_array()) {
                        if !patterns.is_empty() {
                            args.push("--exclude".to_string());
                            for pattern in patterns {
                                if let Some(p) = pattern.as_str() {
                                    args.push(p.to_string());
                                }
                            }
                        }
                    }
                },
                "edit_file" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    let old_string = arguments["old_string"].as_str().unwrap_or("");
                    let new_string = arguments["new_string"].as_str().unwrap_or("");
                    args.push(path.to_string());
                    args.push(old_string.to_string());
                    args.push(new_string.to_string());
                },
                "semantic_search" => {
                    let query = arguments["query"].as_str().unwrap_or("");
                    args.push(query.to_string());
                },
                _ => {}
            }

            let output = execute_tool_binary(args, &state.sandbox_policy).await?;
            
            // Parse the JSON output and convert to YAML format for consistency
            let json_value: Value = serde_json::from_str(&output).unwrap_or_else(|_| {
                // If parsing fails, create a default error response
                json!({"error": format!("Failed to parse tool output: {}", output)})
            });
            
            Ok(serde_yaml::to_string(&json_value)?)
        },
        "web_search" => {
            let query = arguments["query"].as_str().unwrap_or("");
            let limit = arguments.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
            let site = arguments.get("site").and_then(|v| {
                if v.is_array() {
                    v.as_array().map(|arr| arr.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                } else {
                    v.as_str().map(|s| vec![s.to_string()])
                }
            });

            let params = web_search::SearchFunctionParameters {
                query: query.to_string(),
                limit,
                site,
            };

            match web_search::web_search(&params) {
                Ok(results) => {
                    let results_json = serde_json::to_value(&results)?;
                    let search_result = WebSearchResult {
                        status: "Success".to_string(),
                        query: query.to_string(),
                        results: Some(results_json),
                        error: None,
                    };
                    Ok(serde_yaml::to_string(&search_result)?)
                },
                Err(e) => {
                    let search_result = WebSearchResult {
                        status: "Failure".to_string(),
                        query: query.to_string(),
                        results: None,
                        error: Some(format!("Web search failed: {}", e)),
                    };
                    Ok(serde_yaml::to_string(&search_result)?)
                },
            }
        },
        "html_to_text" => {
            let url = arguments["url"].as_str().unwrap_or("");
            let max_content_length = arguments.get("max_content_length")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            let params = web_search::ExtractUrlParameters {
                url: url.to_string(),
                max_content_length,
            };

            match web_search::html_to_text(&params) {
                Ok(result) => {
                    let result_json = serde_json::to_value(&result)?;
                    let html_result = HtmlToTextResult {
                        status: "Success".to_string(),
                        url: url.to_string(),
                        result: Some(result_json),
                        error: None,
                    };
                    Ok(serde_yaml::to_string(&html_result)?)
                },
                Err(e) => {
                    let html_result = HtmlToTextResult {
                        status: "Failure".to_string(),
                        url: url.to_string(),
                        result: None,
                        error: Some(format!("HTML extraction failed: {}", e)),
                    };
                    Ok(serde_yaml::to_string(&html_result)?)
                },
            }
        },
        "todo_write" => {
            // Return the todos array as JSON for the main app to save
            let todos = &arguments["todos"];
            let result = serde_json::json!({
                "status": "Success",
                "todos": todos
            });
            Ok(serde_json::to_string(&result)?)
        },
        _ => Ok(format!("Tool '{}' executed (not fully implemented)", name))
    }
}

// Re-export commonly used types
pub use config::{initialize_config, read_system_prompt, get_default_niterules};
pub use tools::{get_all_tools, generate_tools_section};

/// Message type for communication between TUI and agent
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// User input to send to the agent
    UserInput(String),
    /// Agent's text response
    AgentResponse(String),
    /// Agent's thinking process (internal reasoning) with token count
    ThinkingContent(String, usize), // (content, token_count)
    /// Thinking summary line (from summarizer model)
    ThinkingSummary(String),
    /// Thinking has completed with residual token count
    ThinkingComplete(usize), // residual_token_count
    /// Agent is processing a tool call
    ToolCallStarted(String, String), // (tool_name, arguments_json)
    /// Tool call completed with result
    ToolCallCompleted(String, String), // (tool_name, result)
    /// Agent encountered an error
    Error(String),
    /// Cancel the current generation and finalize the response
    Cancel,
    /// Clear the conversation context/history
    ClearContext,
    /// Background task started
    BackgroundTaskStarted(String, String, String), // (session_id, command, log_file)
    /// Agent has finished processing
    Done,
    /// Model has finished loading
    ModelLoaded,
    /// Generation statistics (tokens/sec, token_count, time_to_first_token, stop_reason)
    GenerationStats(f32, usize, f32, String), // (tok_per_sec, token_count, time_to_first_token_sec, stop_reason)
    /// Reload the model with a new model file
    ReloadModel(String), // (model_filename)
}

/// Agent instance that can be used from the TUI
#[derive(Clone)]
pub struct Agent {
    model: Arc<Mutex<Option<Arc<Model>>>>,
    model_path: String,
    model_files: Arc<Mutex<Vec<String>>>,
    system_prompt: String,
    tools: Vec<Tool>,
    thinking_summarizer: Arc<Mutex<thinking_summarizer::ThinkingSummarizer>>,
    /// Flag to cancel current generation
    cancel_requested: Arc<AtomicBool>,
    /// Tokenizer for accurate token counting
    tokenizer: Arc<Tokenizer>,
    /// Conversation history (RequestBuilder maintains all messages)
    conversation: Arc<Mutex<Option<RequestBuilder>>>,
}

impl Agent {
    /// Create a new agent instance
    pub fn new(
        model_path: String,
        model_files: Vec<String>,
        system_prompt: String,
        tools: Vec<Tool>,
        tokenizer: Tokenizer,
    ) -> Self {
        Self {
            model: Arc::new(Mutex::new(None)),
            model_path,
            model_files: Arc::new(Mutex::new(model_files)),
            system_prompt,
            tools,
            thinking_summarizer: Arc::new(Mutex::new(thinking_summarizer::ThinkingSummarizer::new())),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            tokenizer: Arc::new(tokenizer),
            conversation: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new agent with default configuration
    pub async fn new_with_defaults() -> Result<Self> {
        Self::new_with_model(None).await
    }

    /// Create a new agent with a specific model (or default if None)
    pub async fn new_with_model(model_filename: Option<String>) -> Result<Self> {
        // Initialize config
        if let Err(e) = initialize_config() {
            eprintln!("Warning: Failed to initialize config: {}", e);
        }

        // Get runtime user info
        let os_info = std::env::consts::OS;
        let os_version = if os_info == "linux" {
            std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|content| {
                    content.lines()
                        .find(|line| line.starts_with("PRETTY_NAME="))
                        .map(|line| line.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
                })
                .unwrap_or_else(|| "Linux".to_string())
        } else {
            os_info.to_string()
        };
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .display()
            .to_string();

        // Get tools
        let tools = get_all_tools();
        let tools_section = generate_tools_section(&tools);

        // Read system prompt
        let system_prompt_template = read_system_prompt()
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to read .niterules, using default: {}", e);
                get_default_niterules()
            });

        // Replace placeholders
        let system_prompt = system_prompt_template
            .replace("{tools_section}", &tools_section)
            .replace("{os_version}", &os_version)
            .replace("{workspace_path}", &workspace_path);

        let model_path = "/home/wise/.config/.nite/models".to_string();
        let model_files = vec![model_filename.unwrap_or_else(|| "Qwen_Qwen3-4B-Thinking-2507-Q8_0.gguf".to_string())];

        // Load tokenizer from HuggingFace (Qwen2.5 tokenizer)
        // Suppress output during loading
        #[cfg(unix)]
        let tokenizer = {
            use std::os::unix::io::AsRawFd;
            use std::fs::OpenOptions;

            // Save original stdout and stderr
            let stdout_fd = std::io::stdout().as_raw_fd();
            let stderr_fd = std::io::stderr().as_raw_fd();
            let saved_stdout = unsafe { libc::dup(stdout_fd) };
            let saved_stderr = unsafe { libc::dup(stderr_fd) };

            // Open /dev/null
            let devnull = OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .ok();

            let result = if let Some(devnull) = devnull {
                let devnull_fd = devnull.as_raw_fd();

                // Redirect stdout and stderr to /dev/null
                unsafe {
                    libc::dup2(devnull_fd, stdout_fd);
                    libc::dup2(devnull_fd, stderr_fd);
                }

                // Load tokenizer
                let tokenizer = Tokenizer::from_pretrained("Qwen/Qwen2.5-0.5B", None);

                // Restore stdout and stderr
                unsafe {
                    libc::dup2(saved_stdout, stdout_fd);
                    libc::dup2(saved_stderr, stderr_fd);
                    libc::close(saved_stdout);
                    libc::close(saved_stderr);
                }

                tokenizer
            } else {
                // Fallback if /dev/null can't be opened
                Tokenizer::from_pretrained("Qwen/Qwen2.5-0.5B", None)
            };

            result.map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?
        };

        #[cfg(not(unix))]
        let tokenizer = Tokenizer::from_pretrained("Qwen/Qwen2.5-0.5B", None)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        Ok(Self::new(model_path, model_files, system_prompt, tools, tokenizer))
    }

    /// Get or initialize the model (lazy loading)
    pub async fn get_model(&self) -> Result<Arc<Model>> {
        let mut model_guard = self.model.lock().await;

        if model_guard.is_none() {
            // Redirect stdout/stderr to /dev/null during model loading to suppress progress bars
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                use std::fs::OpenOptions;

                // Save original stdout and stderr
                let stdout_fd = std::io::stdout().as_raw_fd();
                let stderr_fd = std::io::stderr().as_raw_fd();
                let saved_stdout = unsafe { libc::dup(stdout_fd) };
                let saved_stderr = unsafe { libc::dup(stderr_fd) };

                // Open /dev/null
                let devnull = OpenOptions::new()
                    .write(true)
                    .open("/dev/null")
                    .ok();

                if let Some(devnull) = devnull {
                    let devnull_fd = devnull.as_raw_fd();

                    // Redirect stdout and stderr to /dev/null
                    unsafe {
                        libc::dup2(devnull_fd, stdout_fd);
                        libc::dup2(devnull_fd, stderr_fd);
                    }

                    // Load the model
                    let model_files = self.model_files.lock().await.clone();
                    let model = GgufModelBuilder::new(
                        self.model_path.clone(),
                        model_files,
                    )
                    .build()
                    .await?;

                    // Restore stdout and stderr
                    unsafe {
                        libc::dup2(saved_stdout, stdout_fd);
                        libc::dup2(saved_stderr, stderr_fd);
                        libc::close(saved_stdout);
                        libc::close(saved_stderr);
                    }

                    *model_guard = Some(Arc::new(model));
                } else {
                    // Fallback if /dev/null can't be opened
                    let model_files = self.model_files.lock().await.clone();
                    let model = GgufModelBuilder::new(
                        self.model_path.clone(),
                        model_files,
                    )
                    .build()
                    .await?;

                    *model_guard = Some(Arc::new(model));
                }
            }

            #[cfg(not(unix))]
            {
                // On non-Unix systems, just load normally
                let model_files = self.model_files.lock().await.clone();
                let model = GgufModelBuilder::new(
                    self.model_path.clone(),
                    model_files,
                )
                .build()
                .await?;

                *model_guard = Some(Arc::new(model));
            }
        }

        Ok(model_guard.as_ref().unwrap().clone())
    }

    /// Reload the model with a new model file
    /// This clears the cached model and updates the model_files to use the new file
    /// The model will be lazy-loaded on the next get_model() call
    pub async fn reload_model(&self, new_model_filename: String) -> Result<()> {
        // Clear the cached model
        let mut model_guard = self.model.lock().await;
        *model_guard = None;
        drop(model_guard);

        // Update the model files
        let mut model_files_guard = self.model_files.lock().await;
        *model_files_guard = vec![new_model_filename];

        Ok(())
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

    /// Clear the conversation history
    pub async fn clear_conversation(&self) {
        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = None;
    }

    /// Restore conversation from JSON string
    /// Takes a JSON array of messages with "role" and "content" fields
    pub async fn restore_conversation(&self, messages_json: &str) -> Result<()> {
        let messages: Vec<Value> = serde_json::from_str(messages_json)?;

        // Create a new RequestBuilder
        let mut request_builder = RequestBuilder::new()
            .enable_thinking(true)
            .set_tools(self.tools.clone())
            .set_tool_choice(ToolChoice::Auto);

        // Add each message to the RequestBuilder
        for message in messages {
            if let (Some(role_str), Some(content_str)) = (
                message.get("role").and_then(|r| r.as_str()),
                message.get("content").and_then(|c| c.as_str())
            ) {
                let role = match role_str {
                    "system" => TextMessageRole::System,
                    "user" => TextMessageRole::User,
                    "assistant" => TextMessageRole::Assistant,
                    _ => continue, // Skip unknown roles
                };
                request_builder = request_builder.add_message(role, content_str);
            }
        }

        // Store the restored conversation
        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(request_builder);

        Ok(())
    }

    /// Export the conversation directly from RequestBuilder
    /// This gives you the ACTUAL conversation that gets sent to the model
    /// Returns None if there's no conversation history
    pub async fn export_conversation(&self) -> Option<String> {
        let conversation_guard = self.conversation.lock().await;
        if let Some(request_builder) = conversation_guard.as_ref() {
            // Get messages from RequestBuilder and serialize to JSON
            let messages = request_builder.messages();
            if messages.is_empty() {
                None
            } else {
                serde_json::to_string_pretty(&messages).ok()
            }
        } else {
            None
        }
    }

    /// Count tokens in text using the tokenizer
    fn count_tokens(&self, text: &str) -> usize {
        match self.tokenizer.encode(text, false) {
            Ok(encoding) => encoding.len(),
            Err(_) => 0, // Fallback to 0 if encoding fails
        }
    }

    /// Process a user message and stream responses back through the channel
    pub async fn process_message(
        &self,
        user_message: String,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> Result<()> {
        // Reset cancel flag for new message
        self.reset_cancel();

        // Get or create conversation
        let mut conversation_guard = self.conversation.lock().await;

        let request_builder = if let Some(existing_conversation) = conversation_guard.take() {
            // Continue existing conversation
            existing_conversation.add_message(
                TextMessageRole::User,
                &user_message,
            )
        } else {
            // Start new conversation with system prompt
            let system_msg = "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand.";
            let full_user_msg = format!("{}\n\n{}", self.system_prompt, user_message);

            RequestBuilder::new()
                .enable_thinking(true)
                .add_message(TextMessageRole::System, system_msg)
                .add_message(TextMessageRole::User, &full_user_msg)
                .set_tools(self.tools.clone())
                .set_tool_choice(ToolChoice::Auto)
        };
        drop(conversation_guard);

        self.run_generation(request_builder, tx).await
    }

    async fn run_generation(
        &self,
        request_builder: RequestBuilder,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> Result<()> {
        // Get the model (will be loaded only once on first call)
        let model = self.get_model().await?;

        let mut current_request_builder = request_builder;
        let mut has_more_tool_calls = true;
        let mut final_accumulated_content = String::new();

        while has_more_tool_calls {
            let mut stream = model.stream_chat_request(current_request_builder.clone()).await?;
            let mut accumulated_tool_calls: Vec<ToolCallResponse> = Vec::new();
            let mut accumulated_content = String::new();
            has_more_tool_calls = false;

            // Track thinking state (Qwen3-Thinking auto-includes <think> in chat template)
            let mut in_thinking = true;  // Start in thinking mode since <think> is auto-included
            let mut thinking_buffer = String::new();

            loop {
                // Helper macro to check cancellation frequently
                macro_rules! check_cancel {
                    () => {
                        if self.is_cancel_requested() {
                            // Finalize thinking buffer if needed
                            if !thinking_buffer.is_empty() && in_thinking {
                                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                                summarizer_guard.flush().await;
                                for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                                    let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                                }
                            }

                            // Send any accumulated content as partial response
                            if !accumulated_content.is_empty() && !in_thinking {
                                let _ = tx.send(AgentMessage::AgentResponse(accumulated_content.clone()));
                            }

                            // Send Done to finalize
                            let _ = tx.send(AgentMessage::Done);
                            return Ok(());
                        }
                    };
                }

                // Check at start of loop iteration
                check_cancel!();

                // Poll for next response with timeout to allow cancellation checks
                let response = tokio::select! {
                    res = stream.next() => {
                        match res {
                            Some(r) => r,
                            None => break, // Stream ended
                        }
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                        // Timeout - check cancellation and continue (10ms for responsive interruption)
                        check_cancel!();
                        continue;
                    }
                };

                match response {
                    Response::Chunk(ChatCompletionChunkResponse { choices, usage, .. }) => {
                        // Check if this is the final chunk with usage stats
                        // Only send stats if there are NO tool calls (final text response only)
                        if let Some(usage_stats) = usage {
                            if accumulated_tool_calls.is_empty() {
                                let tok_per_sec = usage_stats.avg_compl_tok_per_sec;
                                let token_count = usage_stats.completion_tokens;
                                let time_to_first_token = usage_stats.total_prompt_time_sec;
                                // Try to get finish_reason from the choice
                                let stop_reason = choices.first()
                                    .and_then(|c| c.finish_reason.as_ref())
                                    .map(|s| s.clone())
                                    .unwrap_or_else(|| "unknown".to_string());

                                let _ = tx.send(AgentMessage::GenerationStats(
                                    tok_per_sec,
                                    token_count,
                                    time_to_first_token,
                                    stop_reason
                                ));
                            }
                        }

                        if let Some(choice) = choices.first() {
                            match &choice.delta {
                                Delta {
                                    content: Some(content),
                                    tool_calls: None,
                                    ..
                                } => {
                                    accumulated_content.push_str(content);

                                    if in_thinking {
                                        thinking_buffer.push_str(content);

                                        // Check if we've hit the end of thinking section
                                        if let Some(end_idx) = thinking_buffer.find("</think>") {
                                            // Found end of thinking - switch to normal output mode
                                            in_thinking = false;

                                            // Send the remaining thinking content before the tag (streaming)
                                            let final_thinking = &thinking_buffer[..end_idx];
                                            if !final_thinking.is_empty() {
                                                let token_count = self.count_tokens(final_thinking);
                                                let _ = tx.send(AgentMessage::ThinkingContent(final_thinking.to_string(), token_count));
                                                check_cancel!();

                                                // Add to summarizer and check for new summaries
                                                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                                summarizer_guard.add_thinking_chunk(final_thinking).await;
                                                // Send only new summaries with token count and chunk count embedded
                                                for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                                                    let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                                                    check_cancel!();
                                                }
                                            }

                                            // At end, force flush to handle any residual <50 tokens
                                            let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                            summarizer_guard.flush().await;
                                            // Send new summaries from flush
                                            for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                                                let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                                                check_cancel!();
                                            }
                                            // Send completion signal with residual token count (should be 0 after flush)
                                            let residual_tokens = summarizer_guard.get_residual_token_count();
                                            if residual_tokens > 0 {
                                                let _ = tx.send(AgentMessage::ThinkingComplete(residual_tokens));
                                            }

                                            // Send any content after </think> tag as normal response
                                            let after_think = &thinking_buffer[end_idx + 8..]; // 8 = len("</think>")
                                            if !after_think.is_empty() {
                                                let _ = tx.send(AgentMessage::AgentResponse(after_think.to_string()));
                                            }
                                            thinking_buffer.clear();
                                        } else {
                                            // Still in thinking - stream the chunk immediately
                                            // But keep last 8 chars in buffer in case "</think>" spans chunks
                                            let char_count = thinking_buffer.chars().count();
                                            if char_count > 8 {
                                                // Send (char_count - 8) chars, but check cancellation every 100 chars
                                                let send_char_count = char_count - 8;
                                                if let Some((byte_idx, _)) = thinking_buffer.char_indices().nth(send_char_count) {
                                                    let to_send = &thinking_buffer[..byte_idx];

                                                    // Break into 100-char chunks for frequent cancellation checks
                                                    let mut remaining = to_send;
                                                    while !remaining.is_empty() {
                                                        // Check cancellation before processing each chunk
                                                        check_cancel!();

                                                        // Take up to 100 chars
                                                        let chunk_chars = remaining.chars().take(100).count();
                                                        if let Some((chunk_byte_end, _)) = remaining.char_indices().nth(chunk_chars) {
                                                            let chunk = &remaining[..chunk_byte_end];
                                                            let token_count = self.count_tokens(chunk);
                                                            let _ = tx.send(AgentMessage::ThinkingContent(chunk.to_string(), token_count));
                                                            remaining = &remaining[chunk_byte_end..];
                                                        } else {
                                                            // Last chunk
                                                            let token_count = self.count_tokens(remaining);
                                                            let _ = tx.send(AgentMessage::ThinkingContent(remaining.to_string(), token_count));
                                                            break;
                                                        }

                                                        // Check cancellation after sending each chunk
                                                        check_cancel!();
                                                    }

                                                    // Add to summarizer as we stream
                                                    let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                                    summarizer_guard.add_thinking_chunk(to_send).await;
                                                    // Send only new summaries with token count and chunk count embedded
                                                    for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                                                        let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                                                        // Check cancellation after each summary
                                                        check_cancel!();
                                                    }

                                                    thinking_buffer = thinking_buffer[byte_idx..].to_string();
                                                }
                                            }
                                        }
                                    } else {
                                        // Not in thinking section - send content directly to UI
                                        let _ = tx.send(AgentMessage::AgentResponse(content.clone()));
                                        // Check cancellation after sending response content
                                        check_cancel!();
                                    }
                                }
                                Delta {
                                    tool_calls: Some(tool_calls),
                                    ..
                                } => {
                                    // Before processing tool calls, flush any remaining thinking content
                                    if in_thinking && !thinking_buffer.is_empty() {
                                        // Send remaining thinking content
                                        let token_count = self.count_tokens(&thinking_buffer);
                                        let _ = tx.send(AgentMessage::ThinkingContent(thinking_buffer.clone(), token_count));

                                        // Add to summarizer
                                        let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                        summarizer_guard.add_thinking_chunk(&thinking_buffer).await;

                                        // Force flush for residual
                                        summarizer_guard.flush().await;

                                        // Send new summaries
                                        for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                                            let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                                        }

                                        // Send completion if residual >0 (though flush should handle)
                                        let residual_tokens = summarizer_guard.get_residual_token_count();
                                        if residual_tokens > 0 {
                                            let _ = tx.send(AgentMessage::ThinkingComplete(residual_tokens));
                                        }

                                        thinking_buffer.clear();
                                        in_thinking = false;
                                    }

                                    accumulated_tool_calls.extend(tool_calls.clone());
                                    for tool_call in tool_calls {
                                        let _ = tx.send(AgentMessage::ToolCallStarted(
                                            tool_call.function.name.clone(),
                                            tool_call.function.arguments.clone()
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Response::Done(response) => {
                        // Extract generation statistics only if there are no tool calls
                        if accumulated_tool_calls.is_empty() {
                            let tok_per_sec = response.usage.avg_compl_tok_per_sec;
                            let token_count = response.usage.completion_tokens;
                            let time_to_first_token = response.usage.total_prompt_time_sec;
                            let stop_reason = response.choices.first()
                                .map(|c| c.finish_reason.clone())
                                .unwrap_or_else(|| "unknown".to_string());

                            // Send stats before Done
                            let _ = tx.send(AgentMessage::GenerationStats(
                                tok_per_sec,
                                token_count,
                                time_to_first_token,
                                stop_reason
                            ));
                        }
                        break;
                    }
                    Response::InternalError(e) => {
                        let _ = tx.send(AgentMessage::Error(format!("Internal error: {:?}", e)));
                        break;
                    }
                    Response::ValidationError(e) => {
                        let _ = tx.send(AgentMessage::Error(format!("Validation error: {:?}", e)));
                        break;
                    }
                    Response::ModelError(msg, _) => {
                        let _ = tx.send(AgentMessage::Error(format!("Model error: {}", msg)));
                        break;
                    }
                    _ => {}
                }
            }

            // After stream ends, if still in thinking (no </think> found), flush residual
            if in_thinking && !thinking_buffer.is_empty() {
                let token_count = self.count_tokens(&thinking_buffer);
                let _ = tx.send(AgentMessage::ThinkingContent(thinking_buffer.clone(), token_count));

                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                summarizer_guard.flush().await;
                for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                    let _ = tx.send(AgentMessage::ThinkingSummary(format!("{}|{}|{}", summary, token_count, chunk_count)));
                }
                let residual_tokens = summarizer_guard.get_residual_token_count();
                if residual_tokens > 0 {
                    let _ = tx.send(AgentMessage::ThinkingComplete(residual_tokens));
                }
            }

            // Store the accumulated content for final logging
            if accumulated_tool_calls.is_empty() {
                final_accumulated_content = accumulated_content.clone();
            }

            if !accumulated_tool_calls.is_empty() {
                has_more_tool_calls = true;
                for tool_call in accumulated_tool_calls {
                    // Execute tool call and handle errors gracefully
                    let tool_result = match execute_tool_call(&tool_call).await {
                        Ok(result) => {
                            let _ = tx.send(AgentMessage::ToolCallCompleted(
                                tool_call.function.name.clone(),
                                result.clone()
                            ));

                            // Check if this was a background exec_command and extract session info
                            if tool_call.function.name == "exec_command" {
                                if let Ok(parsed) = serde_yaml::from_str::<serde_json::Value>(&result) {
                                    if let Some(status) = parsed.get("status").and_then(|v| v.as_str()) {
                                        if status == "Background" {
                                            let session_id = parsed.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let command = parsed.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let log_file = parsed.get("log_file").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let _ = tx.send(AgentMessage::BackgroundTaskStarted(session_id, command, log_file));
                                        }
                                    }
                                }
                            }

                            result
                        },
                        Err(e) => {
                            #[derive(Serialize)]
                            struct ToolError {
                                error: String,
                                tool: String,
                                status: String,
                            }
                            let error_obj = ToolError {
                                error: e.to_string(),
                                tool: tool_call.function.name.clone(),
                                status: "failed".to_string(),
                            };
                            let error_msg = serde_yaml::to_string(&error_obj).unwrap_or_else(|_| "error: Failed to serialize error".to_string());
                            let _ = tx.send(AgentMessage::Error(format!("Tool call failed: {}", e)));
                            error_msg
                        }
                    };

                    current_request_builder = current_request_builder
                        .add_message_with_tool_call(
                            TextMessageRole::Assistant,
                            accumulated_content.clone(),
                            vec![tool_call.clone()],
                        )
                        .add_tool_message(&tool_result, &tool_call.id);
                }
            }
        }

        // Store the updated conversation for next turn
        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(current_request_builder);
        drop(conversation_guard);

        let _ = tx.send(AgentMessage::Done);
        Ok(())
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
    ensure_global_state_initialized().await;
    let state = GLOBAL_STATE.get().unwrap();

    // Create session ID
    let session_id_obj = colossal_linux_sandbox::types::SessionId::new(session_id.clone());

    // First, try to kill all background jobs in the shell
    // This will kill the background processes before terminating the shell
    let kill_jobs_cmd = "kill $(jobs -p) 2>/dev/null || true";
    let _ = state.manager.send_input_to_shell_session(
        session_id_obj.clone(),
        kill_jobs_cmd.to_string(),
    ).await;

    // Give it a moment to kill the jobs
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Terminate the session
    state.manager.terminate_session(session_id_obj.clone()).await?;

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
