use anyhow::Result;
use mistralrs::{
    ChatCompletionChunkResponse, Delta, GgufModelBuilder, RequestBuilder, Response,
    TextMessageRole, Tool, ToolCallResponse, ToolChoice, Model,
};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use serde_json::{json, Value};
use serde::Serialize;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::types::ExitStatus;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use once_cell::sync::OnceCell;

pub mod config;
pub mod tools;
pub mod web_search;

// Global state for persistent shell session
struct GlobalState {
    manager: Arc<colossal_linux_sandbox::manager::SessionManager>,
    shell_session_id: tokio::sync::Mutex<Option<colossal_linux_sandbox::types::SessionId>>,
    shell: colossal_linux_sandbox::shell::Shell,
    sandbox_policy: SandboxPolicy,
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
        });
    }
}

async fn get_or_create_shell_session() -> Result<(Arc<colossal_linux_sandbox::manager::SessionManager>, colossal_linux_sandbox::types::SessionId)> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE.get().unwrap();
    let mut session_id_lock = state.shell_session_id.lock().await;

    if session_id_lock.is_none() {
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
            let (manager, session_id) = get_or_create_shell_session().await?;
            let command = arguments["command"].as_str().unwrap_or("");

            let result = manager.exec_command_in_shell_session(
                session_id,
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
    /// Agent's thinking process (internal reasoning)
    ThinkingContent(String),
    /// Agent is processing a tool call
    ToolCallStarted(String),
    /// Tool call completed with result
    ToolCallCompleted(String, String), // (tool_name, result)
    /// Agent encountered an error
    Error(String),
    /// Agent has finished processing
    Done,
    /// Model has finished loading
    ModelLoaded,
}

/// Agent instance that can be used from the TUI
pub struct Agent {
    model: Arc<Mutex<Option<Arc<Model>>>>,
    model_path: String,
    model_files: Vec<String>,
    system_prompt: String,
    tools: Vec<Tool>,
}

impl Agent {
    /// Create a new agent instance
    pub fn new(
        model_path: String,
        model_files: Vec<String>,
        system_prompt: String,
        tools: Vec<Tool>,
    ) -> Self {
        Self {
            model: Arc::new(Mutex::new(None)),
            model_path,
            model_files,
            system_prompt,
            tools,
        }
    }

    /// Create a new agent with default configuration
    pub async fn new_with_defaults() -> Result<Self> {
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
        let model_files = vec!["Qwen_Qwen3-4B-Thinking-2507-Q8_0.gguf".to_string()];

        Ok(Self::new(model_path, model_files, system_prompt, tools))
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
                    let model = GgufModelBuilder::new(
                        self.model_path.clone(),
                        self.model_files.clone(),
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
                    let model = GgufModelBuilder::new(
                        self.model_path.clone(),
                        self.model_files.clone(),
                    )
                    .build()
                    .await?;

                    *model_guard = Some(Arc::new(model));
                }
            }

            #[cfg(not(unix))]
            {
                // On non-Unix systems, just load normally
                let model = GgufModelBuilder::new(
                    self.model_path.clone(),
                    self.model_files.clone(),
                )
                .build()
                .await?;

                *model_guard = Some(Arc::new(model));
            }
        }

        Ok(model_guard.as_ref().unwrap().clone())
    }

    /// Process a user message and stream responses back through the channel
    pub async fn process_message(
        &self,
        user_message: String,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> Result<()> {
        // Create request
        let request_builder = RequestBuilder::new()
            .enable_thinking(true)
            .add_message(
                TextMessageRole::System,
                "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand."
            )
            .add_message(
                TextMessageRole::User,
                &format!("{}\n\n{}", self.system_prompt, user_message),
            )
            .set_tools(self.tools.clone())
            .set_tool_choice(ToolChoice::Auto);

        self.run_generation(request_builder, tx).await
    }

    /// Internal method to handle the generation loop
    async fn run_generation(
        &self,
        request_builder: RequestBuilder,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> Result<()> {
        // Get the model (will be loaded only once on first call)
        let model = self.get_model().await?;

        let mut current_request_builder = request_builder;
        let mut has_more_tool_calls = true;

        while has_more_tool_calls {
            let mut stream = model.stream_chat_request(current_request_builder.clone()).await?;
            let mut accumulated_tool_calls: Vec<ToolCallResponse> = Vec::new();
            let mut accumulated_content = String::new();
            has_more_tool_calls = false;

            // Track thinking state (Qwen3-Thinking auto-includes <think> in chat template)
            let mut in_thinking = true;  // Start in thinking mode since <think> is auto-included
            let mut thinking_buffer = String::new();

            while let Some(response) = stream.next().await {
                match response {
                    Response::Chunk(ChatCompletionChunkResponse { choices, .. }) => {
                        if let Some(choice) = choices.first() {
                            match &choice.delta {
                                Delta {
                                    content: Some(content),
                                    tool_calls: None,
                                    ..
                                } => {
                                    accumulated_content.push_str(content);

                                    // Parse thinking tags for Qwen3-Thinking models
                                    // Note: <think> is auto-included by chat template, so we only see </think>
                                    if in_thinking {
                                        thinking_buffer.push_str(content);

                                        // Check if we've hit the end of thinking section
                                        if let Some(end_idx) = thinking_buffer.find("</think>") {
                                            // Found end of thinking - switch to normal output mode
                                            in_thinking = false;

                                            // Send the remaining thinking content before the tag (streaming)
                                            let final_thinking = &thinking_buffer[..end_idx];
                                            if !final_thinking.is_empty() {
                                                let _ = tx.send(AgentMessage::ThinkingContent(final_thinking.to_string()));
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
                                            // Use character count instead of byte count to avoid UTF-8 boundary panics
                                            let char_count = thinking_buffer.chars().count();
                                            if char_count > 8 {
                                                // Find byte position at the character boundary
                                                // We want to keep last 8 chars, so send (char_count - 8) chars
                                                let send_char_count = char_count - 8;

                                                if let Some((byte_idx, _)) = thinking_buffer.char_indices().nth(send_char_count) {
                                                    let to_send = &thinking_buffer[..byte_idx];
                                                    let _ = tx.send(AgentMessage::ThinkingContent(to_send.to_string()));
                                                    thinking_buffer = thinking_buffer[byte_idx..].to_string();
                                                }
                                            }
                                        }
                                    } else {
                                        // Not in thinking section - send content directly to UI
                                        let _ = tx.send(AgentMessage::AgentResponse(content.clone()));
                                    }
                                }
                                Delta {
                                    tool_calls: Some(tool_calls),
                                    ..
                                } => {
                                    accumulated_tool_calls.extend(tool_calls.clone());
                                    for tool_call in tool_calls {
                                        let _ = tx.send(AgentMessage::ToolCallStarted(
                                            tool_call.function.name.clone()
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Response::Done(_) => {
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
