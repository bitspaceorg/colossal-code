use anyhow::Result;
use agent_protocol::types::{
    message::Role,
    spec::{SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus},
    task::Task,
};
use chrono::Utc;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use colossal_linux_sandbox::types::{ExitStatus, SessionId};
use either::Either;
use futures::StreamExt;
use mistralrs::{
    ChatCompletionChunkResponse, Delta, Model, RequestBuilder, Response, TextMessageRole, Tool,
    ToolCallResponse, ToolChoice,
};
use mistralrs_core::MessageContent;
use once_cell::sync::OnceCell;
use serde::Serialize;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokenizers::Tokenizer;
use tokio::sync::{Mutex, mpsc};

pub mod config;
mod llm_backend;
pub use llm_backend::{HttpBackend, LLMBackend, LocalBackend};
pub mod model_config;
pub mod safety_config;
pub mod thinking_summarizer;
pub mod tools;
pub mod web_search;
pub mod a2a;
pub mod orchestrator;

// Global state for persistent shell session
struct GlobalState {
    manager: Arc<colossal_linux_sandbox::manager::SessionManager>,
    shell_session_id: tokio::sync::Mutex<Option<colossal_linux_sandbox::types::SessionId>>,
    shell: colossal_linux_sandbox::shell::Shell,
    sandbox_policy: tokio::sync::Mutex<SandboxPolicy>,
    // Track if current session has a background process running
    session_has_background_process: tokio::sync::Mutex<bool>,
    // Pending approval channel
    pending_approval: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
}

static GLOBAL_STATE: OnceCell<GlobalState> = OnceCell::new();

fn thinking_debug_enabled() -> bool {
    static FLAG: OnceCell<bool> = OnceCell::new();
    *FLAG.get_or_init(|| {
        std::env::var("NITE_DEBUG_THINKING")
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
    })
}

fn thinking_debug_log(message: impl AsRef<str>) {
    if thinking_debug_enabled() {
        eprintln!("[NITE THINK] {}", message.as_ref());
    }
}

fn preview_thinking(text: &str) -> String {
    let mut preview: String = text.chars().take(80).collect();
    if text.chars().count() > 80 {
        preview.push_str("…");
    }
    preview
}

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
        let workspace_path =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        let mut writable_roots = vec![colossal_linux_sandbox::protocol::WritableRoot {
            root: workspace_path.clone(),
            recursive: true,
            read_only_subpaths: vec![],
        }];

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

        // Determine sandbox policy based on safety configuration
        let safety_config = safety_config::SafetyConfig::load().unwrap_or_default();
        let sandbox_policy = match safety_config.mode {
            safety_config::SafetyMode::ReadOnly => {
                // Read-only mode: NO write access anywhere
                SandboxPolicy::ReadOnly
            }
            safety_config::SafetyMode::Regular => {
                // Regular mode: sandbox enabled with write access to workspace
                if safety_config.sandbox_enabled || std::env::var("SAFE_MODE").is_ok() {
                    SandboxPolicy::WorkspaceWrite {
                        writable_roots,
                        network_access: colossal_linux_sandbox::protocol::NetworkAccess::Enabled,
                        exclude_tmpdir_env_var: false,
                        exclude_slash_tmp: false,
                    }
                } else {
                    SandboxPolicy::DangerFullAccess
                }
            }
            safety_config::SafetyMode::Yolo => {
                // Yolo mode: full access (no sandbox)
                SandboxPolicy::DangerFullAccess
            }
        };

        let _ = GLOBAL_STATE.set(GlobalState {
            manager: Arc::new(colossal_linux_sandbox::manager::SessionManager::default()),
            shell_session_id: tokio::sync::Mutex::new(None),
            shell,
            sandbox_policy: tokio::sync::Mutex::new(sandbox_policy),
            session_has_background_process: tokio::sync::Mutex::new(false),
            pending_approval: tokio::sync::Mutex::new(None),
        });
    }
}

/// Add a writable root to the sandbox policy dynamically
pub async fn add_writable_root(path: std::path::PathBuf) -> Result<()> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Global state not initialized"))?;

    let mut policy_lock = state.sandbox_policy.lock().await;

    match &mut *policy_lock {
        SandboxPolicy::WorkspaceWrite { writable_roots, .. } => {
            // Check if path already exists
            if writable_roots.iter().any(|root| root.root == path) {
                return Ok(()); // Already added
            }

            // Add new writable root
            writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                root: path,
                recursive: true,
                read_only_subpaths: vec![],
            });

            Ok(())
        }
        SandboxPolicy::DangerFullAccess => {
            // Already has full access, no need to add
            Ok(())
        }
        SandboxPolicy::ReadOnly => {
            // Read-only mode: cannot add writable roots
            Err(anyhow::anyhow!(
                "Cannot add writable root in read-only mode"
            ))
        }
    }
}

async fn get_or_create_shell_session() -> Result<(
    Arc<colossal_linux_sandbox::manager::SessionManager>,
    colossal_linux_sandbox::types::SessionId,
)> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE.get().unwrap();
    let mut session_id_lock = state.shell_session_id.lock().await;
    let has_background = state.session_has_background_process.lock().await;

    // Create new session if:
    // 1. No session exists yet, OR
    // 2. Current session has a background process running
    if session_id_lock.is_none() || *has_background {
        let workspace_path =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
            workspace_path.clone(),
        ));

        let sandbox_policy = state.sandbox_policy.lock().await.clone();
        let session_id = state
            .manager
            .create_persistent_shell_session(
                state.shell.path().to_string_lossy().to_string(),
                false,
                sandbox_policy,
                shared_state,
                None,
            )
            .await?;

        let _ = state
            .manager
            .exec_command_in_shell_session(
                session_id.clone(),
                format!("cd {}", workspace_path.display()),
                Some(5000),
                1000,
                None, // No approval needed for initial cd
            )
            .await;

        *session_id_lock = Some(session_id.clone());
    }

    Ok((state.manager.clone(), session_id_lock.clone().unwrap()))
}

async fn execute_tool_call(
    tool_call: &ToolCallResponse,
    tx: mpsc::UnboundedSender<AgentMessage>,
) -> Result<String> {
    ensure_global_state_initialized().await;

    let name = &tool_call.function.name;
    let arguments: Value = serde_json::from_str(&tool_call.function.arguments)?;

    match name.as_str() {
        "exec_command" => {
            let state = GLOBAL_STATE.get().unwrap();
            let command = arguments["command"].as_str().unwrap_or("");
            let is_background = arguments
                .get("is_background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Get or create shell session (will create new one if current has background process)
            let (manager, session_id) = get_or_create_shell_session().await?;

            // Determine safety strategy
            let safety_mode = if std::env::var("SAFE_MODE").is_ok() {
                colossal_linux_sandbox::safety::AskForApproval::OnRequest
            } else {
                colossal_linux_sandbox::safety::AskForApproval::Never
            };

            let mut current_approval = Some(safety_mode);

            loop {
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

                    match manager
                        .send_input_to_shell_session(
                            session_id.clone(),
                            bg_command,
                            current_approval,
                        )
                        .await
                    {
                        Ok(_) => {
                            let exec_result = serde_json::json!({
                                "command": command,
                                "status": "Background",
                                "session_id": session_id.as_str(),
                                "log_file": log_file_path,
                                "message": format!("Command started in background. Session ID: {}. Log file: {}", session_id.as_str(), log_file_path)
                            });
                            return Ok(serde_yaml::to_string(&exec_result)?);
                        }
                        Err(e) => {
                            if let colossal_linux_sandbox::error::ColossalErr::Sandbox(
                                colossal_linux_sandbox::error::SandboxErr::Denied(_, reason, _),
                            ) = &e
                            {
                                if reason == "User approval required" {
                                    // Send request to UI
                                    let (approval_tx, approval_rx) =
                                        tokio::sync::oneshot::channel();
                                    {
                                        let state = GLOBAL_STATE.get().unwrap();
                                        let mut guard = state.pending_approval.lock().await;
                                        *guard = Some(approval_tx);
                                    }

                                    let request_msg =
                                        format!("Allow background command: {}", command);
                                    let _ = tx.send(AgentMessage::RequestApproval(request_msg));

                                    // Wait for response
                                    match approval_rx.await {
                                        Ok(true) => {
                                            // Approved, retry with Never (force approve)
                                            current_approval = Some(colossal_linux_sandbox::safety::AskForApproval::Never);

                                            // Reset background flag since we failed to start
                                            let mut has_background =
                                                state.session_has_background_process.lock().await;
                                            *has_background = false;
                                            continue;
                                        }
                                        Ok(false) => {
                                            // Reset background flag since we failed to start
                                            let mut has_background =
                                                state.session_has_background_process.lock().await;
                                            *has_background = false;

                                            return Ok(serde_yaml::to_string(&json!({
                                                "status": "Failure",
                                                "command": command,
                                                "message": "Command denied by user"
                                            }))?);
                                        }
                                        Err(_) => {
                                            return Err(anyhow::anyhow!("Approval channel closed"));
                                        }
                                    }
                                }
                            }

                            // Reset background flag on other errors
                            let mut has_background =
                                state.session_has_background_process.lock().await;
                            *has_background = false;
                            return Err(e.into());
                        }
                    }
                } else {
                    // Foreground command - wait for completion
                    match manager
                        .exec_command_in_shell_session(
                            session_id.clone(),
                            command.to_string(),
                            Some(5000),
                            1000,
                            current_approval,
                        )
                        .await
                    {
                        Ok(result) => {
                            let is_success = matches!(result.exit_status, ExitStatus::Completed { code } if code == 0);
                            let exec_result = ExecCommandResult {
                                command: command.to_string(),
                                status: if is_success {
                                    "Success".to_string()
                                } else {
                                    "Failure".to_string()
                                },
                                cmd_out: result.aggregated_output,
                                message: if is_success {
                                    None
                                } else {
                                    Some(format!("{:?}", result.exit_status))
                                },
                            };
                            return Ok(serde_yaml::to_string(&exec_result)?);
                        }
                        Err(e) => {
                            if let colossal_linux_sandbox::error::ColossalErr::Sandbox(
                                colossal_linux_sandbox::error::SandboxErr::Denied(_, reason, _),
                            ) = &e
                            {
                                if reason == "User approval required" {
                                    // Send request to UI
                                    let (approval_tx, approval_rx) =
                                        tokio::sync::oneshot::channel();
                                    {
                                        let state = GLOBAL_STATE.get().unwrap();
                                        let mut guard = state.pending_approval.lock().await;
                                        *guard = Some(approval_tx);
                                    }

                                    let request_msg = format!("Allow command: {}", command);
                                    let _ = tx.send(AgentMessage::RequestApproval(request_msg));

                                    // Wait for response
                                    match approval_rx.await {
                                        Ok(true) => {
                                            // Approved, retry with Never (force approve)
                                            current_approval = Some(colossal_linux_sandbox::safety::AskForApproval::Never);
                                            continue;
                                        }
                                        Ok(false) => {
                                            return Ok(serde_yaml::to_string(&json!({
                                                "status": "Failure",
                                                "command": command,
                                                "message": "Command denied by user"
                                            }))?);
                                        }
                                        Err(_) => {
                                            return Err(anyhow::anyhow!("Approval channel closed"));
                                        }
                                    }
                                }
                            }
                            return Err(e.into());
                        }
                    }
                }
            }
        }
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
                }
                Err(e) => {
                    let result = serde_json::json!({
                        "status": "Failure",
                        "session_id": session_id_str,
                        "error": format!("Failed to read output: {}", e)
                    });
                    Ok(serde_yaml::to_string(&result)?)
                }
            }
        }
        "read_file"
        | "delete_path"
        | "delete_many"
        | "get_files"
        | "get_files_recursive"
        | "search_files_with_regex"
        | "edit_file"
        | "semantic_search" => {
            let state = GLOBAL_STATE.get().unwrap();
            let mut args = vec![name.to_string()];

            // Build args based on tool type
            match name.as_str() {
                "read_file" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    let should_read_entire = arguments["should_read_entire_file"]
                        .as_bool()
                        .unwrap_or(true);
                    args.push(path.to_string());
                    if should_read_entire {
                        args.push("entire".to_string());
                    }
                }
                "delete_path" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    args.push(path.to_string());
                }
                "get_files" => {
                    let path = arguments["path"].as_str().unwrap_or(".");
                    let limit = arguments["limit"]
                        .as_u64()
                        .map(|l| l.to_string())
                        .unwrap_or_else(|| "100".to_string());
                    args.push(path.to_string());
                    args.push(limit);
                }
                "get_files_recursive" => {
                    let path = arguments["path"].as_str().unwrap_or(".");
                    args.push(path.to_string());

                    // Add limit if provided
                    if let Some(limit) = arguments.get("limit").and_then(|v| v.as_u64()) {
                        args.push(limit.to_string());
                    }

                    // Add include patterns if provided
                    if let Some(patterns) =
                        arguments.get("include_patterns").and_then(|v| v.as_array())
                    {
                        for pattern in patterns {
                            if let Some(p) = pattern.as_str() {
                                args.push(p.to_string());
                            }
                        }
                    }

                    // Add exclude patterns with --exclude flag if provided
                    if let Some(patterns) =
                        arguments.get("exclude_patterns").and_then(|v| v.as_array())
                    {
                        if !patterns.is_empty() {
                            args.push("--exclude".to_string());
                            for pattern in patterns {
                                if let Some(p) = pattern.as_str() {
                                    args.push(p.to_string());
                                }
                            }
                        }
                    }
                }
                "edit_file" => {
                    let path = arguments["path"].as_str().unwrap_or("");
                    let old_string = arguments["old_string"].as_str().unwrap_or("");
                    let new_string = arguments["new_string"].as_str().unwrap_or("");
                    args.push(path.to_string());
                    args.push(old_string.to_string());
                    args.push(new_string.to_string());
                }
                "semantic_search" => {
                    let query = arguments["query"].as_str().unwrap_or("");
                    args.push(query.to_string());
                }
                _ => {}
            }

            let sandbox_policy = state.sandbox_policy.lock().await.clone();
            let output = execute_tool_binary(args, &sandbox_policy).await?;

            // Parse the JSON output and convert to YAML format for consistency
            let json_value: Value = serde_json::from_str(&output).unwrap_or_else(|_| {
                // If parsing fails, create a default error response
                json!({"error": format!("Failed to parse tool output: {}", output)})
            });

            Ok(serde_yaml::to_string(&json_value)?)
        }
        "web_search" => {
            let query = arguments["query"].as_str().unwrap_or("");
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let site = arguments.get("site").and_then(|v| {
                if v.is_array() {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                            .collect()
                    })
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
                }
                Err(e) => {
                    let search_result = WebSearchResult {
                        status: "Failure".to_string(),
                        query: query.to_string(),
                        results: None,
                        error: Some(format!("Web search failed: {}", e)),
                    };
                    Ok(serde_yaml::to_string(&search_result)?)
                }
            }
        }
        "html_to_text" => {
            let url = arguments["url"].as_str().unwrap_or("");
            let max_content_length = arguments
                .get("max_content_length")
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
                }
                Err(e) => {
                    let html_result = HtmlToTextResult {
                        status: "Failure".to_string(),
                        url: url.to_string(),
                        result: None,
                        error: Some(format!("HTML extraction failed: {}", e)),
                    };
                    Ok(serde_yaml::to_string(&html_result)?)
                }
            }
        }
        "todo_write" => {
            // Return the todos array as JSON for the main app to save
            let todos = &arguments["todos"];
            let result = serde_json::json!({
                "status": "Success",
                "todos": todos
            });
            Ok(serde_json::to_string(&result)?)
        }
        "request_split" => {
            let reason = arguments
                .get("reason")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let payload = serde_json::json!({
                "status": "split_requested",
                "reason": reason
            });
            Ok(serde_yaml::to_string(&payload)?)
        }
        _ => Ok(format!("Tool '{}' executed (not fully implemented)", name)),
    }
}

// Re-export commonly used types
pub use config::{get_default_niterules, initialize_config, read_system_prompt};
pub use tools::{generate_tools_section, get_all_tools};

/// Configuration for selecting which LLM backend to use
#[derive(Debug, Clone)]
pub enum BackendConfig {
    /// Use a local GGUF model
    Local {
        model_path: String,
        model_files: Vec<String>,
    },
    /// Use a remote HTTP backend (OpenAI-compatible API)
    Http {
        base_url: String,
        api_key: String,
        model: String,
        completions_path: String,
        requires_model_load: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Local,
    Http,
    ExternalHttp,
}

/// Aggregated generation statistics for a completed response turn
#[derive(Debug, Clone)]
pub struct GenerationStats {
    pub avg_completion_tok_per_sec: f32,
    pub completion_tokens: usize,
    pub prompt_tokens: usize,
    pub time_to_first_token_sec: f32,
    pub stop_reason: String,
}

/// Message type for communication between TUI and agent
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// User input to send to the agent
    UserInput(String),
    /// Agent's text response with token count for real-time context tracking
    AgentResponse(String, usize), // (content, token_count)
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
    /// Inject a summary as the new conversation context (used after compaction)
    InjectContext(String),
    /// Context has been cleared (acknowledgment)
    ContextCleared,
    /// Context has been injected (acknowledgment)
    ContextInjected,
    /// Background task started
    BackgroundTaskStarted(String, String, String), // (session_id, command, log_file)
    /// Agent has finished processing
    Done,
    /// Model has finished loading
    ModelLoaded,
    /// Generation statistics (prompt/completion tokens, token/sec, latency, stop reason)
    GenerationStats(GenerationStats),
    /// Reload the model with a new model file
    ReloadModel(String), // (model_filename)
    /// Request user approval for an action
    RequestApproval(String), // (reason)
    /// User response to approval request
    ApprovalResponse(bool), // (approved)
}

enum ThinkingStartDecision {
    NeedMoreData,
    Detected { content_start_idx: usize },
    NotThinking,
}

fn analyze_thinking_start(buffer: &str, open_tag: &str) -> ThinkingStartDecision {
    if buffer.trim().is_empty() {
        return ThinkingStartDecision::NeedMoreData;
    }

    let first_non_ws_idx = buffer
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx);

    let Some(start_idx) = first_non_ws_idx else {
        return ThinkingStartDecision::NeedMoreData;
    };

    let trimmed = &buffer[start_idx..];
    if trimmed.starts_with(open_tag) {
        return ThinkingStartDecision::Detected {
            content_start_idx: start_idx + open_tag.len(),
        };
    }

    if open_tag.starts_with(trimmed) {
        ThinkingStartDecision::NeedMoreData
    } else {
        ThinkingStartDecision::NotThinking
    }
}

fn has_visible_text(value: &str) -> bool {
    value.chars().any(|c| !c.is_whitespace())
}

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
    /// Tokenizer for accurate token counting
    tokenizer: Arc<Tokenizer>,
    /// Conversation history (RequestBuilder maintains all messages)
    conversation: Arc<Mutex<Option<RequestBuilder>>>,
    /// Thinking tags configuration (opening/closing tags and summary interval)
    thinking_tags: Arc<Mutex<model_config::ThinkingTags>>,
    /// Safety configuration for tool access
    safety_config: Arc<Mutex<safety_config::SafetyConfig>>,
}

impl Agent {
    /// Load thinking tag configuration from disk if available.
    /// Falls back to default `<think>`/`</think>` tags when no config is present.
    fn load_thinking_tags(
        _model_path: &str,
        model_filename: &str,
    ) -> model_config::ThinkingTags {
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
        tokenizer: Tokenizer,
        safety_config: safety_config::SafetyConfig,
    ) -> Self {
        // Create backend based on config
        let (backend, backend_kind, thinking_tags): (
            Arc<Box<dyn LLMBackend>>,
            BackendKind,
            model_config::ThinkingTags,
        ) = match backend_config {
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
                (
                    backend,
                    BackendKind::Local,
                    thinking_tags,
                )
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
            tokenizer: Arc::new(tokenizer),
            conversation: Arc::new(Mutex::new(None)),
            thinking_tags: Arc::new(Mutex::new(thinking_tags)),
            safety_config: Arc::new(Mutex::new(safety_config)),
        }
    }

    /// Create a new agent instance (legacy method, uses Local backend)
    pub fn new(
        model_path: String,
        model_files: Vec<String>,
        system_prompt: String,
        tools: Vec<Tool>,
        tokenizer: Tokenizer,
        safety_config: safety_config::SafetyConfig,
    ) -> Self {
        Self::new_with_backend(
            BackendConfig::Local {
                model_path,
                model_files,
            },
            system_prompt,
            tools,
            tokenizer,
            safety_config,
        )
    }

    /// Create a new agent with default configuration
    pub async fn new_with_defaults() -> Result<Self> {
        Self::new_with_model(None).await
    }

    /// Create a new agent with a specific model (or default if None)
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
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .display()
            .to_string();

        // Load safety configuration
        let safety_config = safety_config::SafetyConfig::load().unwrap_or_default();
        // Get tools based on safety mode
        let tools = if safety_config.mode == safety_config::SafetyMode::ReadOnly {
            tools::get_readonly_tools()
        } else {
            tools::get_all_tools()
        };
        let tools_section = generate_tools_section(&tools);

        // Read system prompt
        let system_prompt_template = read_system_prompt().unwrap_or_else(|_e| {
            // eprintln!("Warning: Failed to read .niterules, using default: {}", e);
            get_default_niterules()
        });

        // Replace placeholders
        let mut system_prompt = system_prompt_template
            .replace("{tools_section}", &tools_section)
            .replace("{os_version}", &os_version)
            .replace("{workspace_path}", &workspace_path);

        // Add safety mode suffix if needed
        if let Some(suffix) = safety_config.get_system_prompt_suffix() {
            system_prompt.push_str(&suffix);
        }

        let model_path = "/home/wise/.config/.nite/models".to_string();
        let selected_model =
            model_filename.unwrap_or_else(|| "Qwen_Qwen3-4B-Thinking-2507-Q8_0.gguf".to_string());

        // Detect tokenizer from model filename
        let tokenizer_name = Self::detect_tokenizer_from_model(&selected_model);

        // Load tokenizer from HuggingFace
        // Suppress output during loading
        #[cfg(unix)]
        let tokenizer = {
            use std::fs::OpenOptions;
            use std::os::unix::io::AsRawFd;

            // Save original stdout and stderr
            let stdout_fd = std::io::stdout().as_raw_fd();
            let stderr_fd = std::io::stderr().as_raw_fd();
            let saved_stdout = unsafe { libc::dup(stdout_fd) };
            let saved_stderr = unsafe { libc::dup(stderr_fd) };

            // Open /dev/null
            let devnull = OpenOptions::new().write(true).open("/dev/null").ok();

            let result = if let Some(devnull) = devnull {
                let devnull_fd = devnull.as_raw_fd();

                // Redirect stdout and stderr to /dev/null
                unsafe {
                    libc::dup2(devnull_fd, stdout_fd);
                    libc::dup2(devnull_fd, stderr_fd);
                }

                // Load tokenizer
                let tokenizer = Tokenizer::from_pretrained(&tokenizer_name, None);

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
                Tokenizer::from_pretrained(&tokenizer_name, None)
            };

            result.map_err(|e| anyhow::anyhow!("Failed to load tokenizer '{}': {}", tokenizer_name, e))?
        };

        #[cfg(not(unix))]
        let tokenizer = Tokenizer::from_pretrained(&tokenizer_name, None).map_err(|e| {
            anyhow::anyhow!("Failed to load tokenizer '{}': {}", tokenizer_name, e)
        })?;

        let backend_mode =
            std::env::var("NITE_BACKEND_MODE").unwrap_or_else(|_| "http".to_string());
        let backend_mode = backend_mode.to_lowercase();

        let backend_config = match backend_mode.as_str() {
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
        };

        Ok(Self::new_with_backend(
            backend_config,
            system_prompt,
            tools,
            tokenizer,
            safety_config,
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

        Ok(())
    }

    /// Handle approval response from UI
    pub async fn handle_approval_response(&self, approved: bool) {
        if let Some(state) = GLOBAL_STATE.get() {
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

    /// Detect the appropriate HuggingFace tokenizer from a model filename
    fn detect_tokenizer_from_model(model_filename: &str) -> String {
        let filename_lower = model_filename.to_lowercase();

        // Qwen models
        if filename_lower.contains("qwen3") || filename_lower.contains("qwen-3") {
            // Qwen 3 models
            if filename_lower.contains("32b") {
                "Qwen/Qwen3-32B".to_string()
            } else if filename_lower.contains("14b") {
                "Qwen/Qwen3-14B".to_string()
            } else if filename_lower.contains("8b") {
                "Qwen/Qwen3-8B".to_string()
            } else if filename_lower.contains("4b") {
                "Qwen/Qwen3-4B".to_string()
            } else if filename_lower.contains("1.7b") || filename_lower.contains("1_7b") {
                "Qwen/Qwen3-1.7B".to_string()
            } else if filename_lower.contains("0.6b") || filename_lower.contains("0_6b") {
                "Qwen/Qwen3-0.6B".to_string()
            } else {
                "Qwen/Qwen3-4B".to_string() // Default Qwen3
            }
        } else if filename_lower.contains("qwen2.5") || filename_lower.contains("qwen2_5") {
            // Qwen 2.5 models
            if filename_lower.contains("72b") {
                "Qwen/Qwen2.5-72B".to_string()
            } else if filename_lower.contains("32b") {
                "Qwen/Qwen2.5-32B".to_string()
            } else if filename_lower.contains("14b") {
                "Qwen/Qwen2.5-14B".to_string()
            } else if filename_lower.contains("7b") {
                "Qwen/Qwen2.5-7B".to_string()
            } else if filename_lower.contains("3b") {
                "Qwen/Qwen2.5-3B".to_string()
            } else if filename_lower.contains("1.5b") || filename_lower.contains("1_5b") {
                "Qwen/Qwen2.5-1.5B".to_string()
            } else if filename_lower.contains("0.5b") || filename_lower.contains("0_5b") {
                "Qwen/Qwen2.5-0.5B".to_string()
            } else {
                "Qwen/Qwen2.5-7B".to_string() // Default Qwen2.5
            }
        } else if filename_lower.contains("qwen2") || filename_lower.contains("qwen-2") {
            // Qwen 2 models
            "Qwen/Qwen2-7B".to_string()
        } else if filename_lower.contains("qwen") {
            // Generic Qwen - assume Qwen 2.5
            "Qwen/Qwen2.5-7B".to_string()
        }
        // Llama models
        else if filename_lower.contains("llama-3.3") || filename_lower.contains("llama3.3") {
            "meta-llama/Llama-3.3-70B-Instruct".to_string()
        } else if filename_lower.contains("llama-3.2") || filename_lower.contains("llama3.2") {
            if filename_lower.contains("3b") {
                "meta-llama/Llama-3.2-3B".to_string()
            } else if filename_lower.contains("1b") {
                "meta-llama/Llama-3.2-1B".to_string()
            } else {
                "meta-llama/Llama-3.2-3B".to_string()
            }
        } else if filename_lower.contains("llama-3.1") || filename_lower.contains("llama3.1") {
            if filename_lower.contains("405b") {
                "meta-llama/Llama-3.1-405B".to_string()
            } else if filename_lower.contains("70b") {
                "meta-llama/Llama-3.1-70B".to_string()
            } else {
                "meta-llama/Llama-3.1-8B".to_string()
            }
        } else if filename_lower.contains("llama-3") || filename_lower.contains("llama3") {
            if filename_lower.contains("70b") {
                "meta-llama/Meta-Llama-3-70B".to_string()
            } else {
                "meta-llama/Meta-Llama-3-8B".to_string()
            }
        } else if filename_lower.contains("llama") {
            "meta-llama/Llama-3.1-8B".to_string()
        }
        // Mistral models
        else if filename_lower.contains("mistral") {
            if filename_lower.contains("nemo") {
                "mistralai/Mistral-Nemo-Base-2407".to_string()
            } else if filename_lower.contains("large") {
                "mistralai/Mistral-Large-Instruct-2407".to_string()
            } else {
                "mistralai/Mistral-7B-v0.3".to_string()
            }
        }
        // Gemma models
        else if filename_lower.contains("gemma-2") || filename_lower.contains("gemma2") {
            if filename_lower.contains("27b") {
                "google/gemma-2-27b".to_string()
            } else if filename_lower.contains("9b") {
                "google/gemma-2-9b".to_string()
            } else {
                "google/gemma-2-2b".to_string()
            }
        } else if filename_lower.contains("gemma") {
            "google/gemma-7b".to_string()
        }
        // Phi models
        else if filename_lower.contains("phi-4") || filename_lower.contains("phi4") {
            "microsoft/phi-4".to_string()
        } else if filename_lower.contains("phi-3") || filename_lower.contains("phi3") {
            "microsoft/Phi-3-mini-4k-instruct".to_string()
        } else if filename_lower.contains("phi") {
            "microsoft/phi-2".to_string()
        }
        // DeepSeek models
        else if filename_lower.contains("deepseek") {
            if filename_lower.contains("r1") {
                "deepseek-ai/DeepSeek-R1".to_string()
            } else if filename_lower.contains("v3") {
                "deepseek-ai/DeepSeek-V3".to_string()
            } else if filename_lower.contains("coder") {
                "deepseek-ai/deepseek-coder-7b-base-v1.5".to_string()
            } else {
                "deepseek-ai/deepseek-llm-7b-base".to_string()
            }
        }
        // Default fallback
        else {
            "Qwen/Qwen2.5-7B".to_string()
        }
    }

    /// Get the thinking tags configuration
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

        // Update tools in the agent
        {
            let mut tools_guard = self.tools.lock().await;
            *tools_guard = new_tools.clone();
        }

        // Update system prompt with new tools section
        let tools_section = generate_tools_section(&new_tools);
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
        let workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .display()
            .to_string();

        // Read system prompt and apply new tools section
        let system_prompt_template = read_system_prompt().unwrap_or_else(|_e| {
            // eprintln!("Warning: Failed to read .niterules, using default: {}", e);
            get_default_niterules()
        });

        // Update the system prompt with new tools
        let mut updated_system_prompt = system_prompt_template
            .replace("{tools_section}", &tools_section)
            .replace("{os_version}", &os_version)
            .replace("{workspace_path}", &workspace_path);

        // Add safety mode suffix if needed
        if let Some(suffix) = new_safety_config.get_system_prompt_suffix() {
            updated_system_prompt.push_str(&suffix);
        }

        // Update the system prompt in the agent
        {
            let mut system_prompt_guard = self.system_prompt.lock().await;
            *system_prompt_guard = updated_system_prompt;
        }

        // If there's an active conversation, update the tools in the request builder
        {
            let mut conversation_guard = self.conversation.lock().await;
            if let Some(ref mut conversation) = *conversation_guard {
                // Create a new request builder with updated tools
                *conversation_guard = Some(conversation.clone().set_tools(new_tools));
            }
        }

        Ok(())
    }

    /// Clear the conversation history
    pub async fn clear_conversation(&self) {
        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = None;
    }

    /// Inject a summary as the new conversation context
    /// This is used after compaction to give the model context about the previous conversation
    pub async fn inject_summary_context(&self, summary: &str) {
        let tools = {
            let tools_guard = self.tools.lock().await;
            tools_guard.clone()
        };

        // Get the system prompt (same as used when starting new conversations)
        let system_prompt_content = {
            let system_prompt_guard = self.system_prompt.lock().await;
            system_prompt_guard.clone()
        };

        let system_msg = "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand.";

        // Format the summary as if it were the first user message, including full system prompt
        let full_context_msg = format!(
            "{}\n\n\
             This session is being continued from a previous conversation that ran out of context. \
             The previous conversation has been summarized below:\n\n{}",
            system_prompt_content, summary
        );

        // Create a new conversation with the same structure as a normal conversation start
        let request_builder = RequestBuilder::new()
            .add_message(TextMessageRole::System, system_msg)
            .add_message(TextMessageRole::User, &full_context_msg)
            .set_tools(tools)
            .set_tool_choice(ToolChoice::Auto)
            .enable_thinking(true);

        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(request_builder);
    }

    /// Restore conversation from JSON string
    /// Takes a JSON array of messages with "role" and "content" fields
    pub async fn restore_conversation(&self, messages_json: &str) -> Result<()> {
        let messages: Vec<Value> = serde_json::from_str(messages_json)?;

        // Create a new RequestBuilder
        let tools = {
            let tools_guard = self.tools.lock().await;
            tools_guard.clone()
        };
        let mut request_builder = RequestBuilder::new()
            .set_tools(tools)
            .set_tool_choice(ToolChoice::Auto)
            .enable_thinking(true);

        // Add each message to the RequestBuilder
        for message in messages {
            if let (Some(role_str), Some(content_str)) = (
                message.get("role").and_then(|r| r.as_str()),
                message.get("content").and_then(|c| c.as_str()),
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

    pub async fn execute_step(&self, _step: SpecStep, _spec: &SpecSheet) -> Result<Task> {
        Err(anyhow::anyhow!("execute_step is not implemented yet"))
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

    /// Convert a structured chat message field into a plain string for token estimation.
    fn message_content_to_string(value: &MessageContent) -> String {
        match value {
            Either::Left(text) => text.clone(),
            Either::Right(chunks) => chunks
                .iter()
                .map(|chunk| serde_json::to_string(chunk).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// Roughly estimate the prompt tokens for a pending request by flattening all messages.
    fn estimate_prompt_tokens(&self, request_builder: &RequestBuilder) -> usize {
        let mut flattened = String::new();
        for message in request_builder.messages() {
            for (key, value) in message {
                flattened.push_str(key);
                flattened.push(':');
                flattened.push_str(&Self::message_content_to_string(value));
                flattened.push('\n');
            }
            flattened.push('\n');
        }
        self.count_tokens(&flattened)
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
            existing_conversation.add_message(TextMessageRole::User, &user_message)
        } else {
            // Start new conversation with system prompt
            let system_prompt_content = {
                let system_prompt_guard = self.system_prompt.lock().await;
                system_prompt_guard.clone()
            };
            let system_msg = "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand.";
            let full_user_msg = format!("{}\n\n{}", system_prompt_content, user_message);

            let tools = {
                let tools_guard = self.tools.lock().await;
                tools_guard.clone()
            };
            let mut builder = RequestBuilder::new()
                .add_message(TextMessageRole::System, system_msg)
                .add_message(TextMessageRole::User, &full_user_msg)
                .set_tools(tools)
                .set_tool_choice(ToolChoice::Auto)
                .enable_thinking(true);

            builder
        };
        drop(conversation_guard);

        self.run_generation(request_builder, tx).await
    }

    async fn run_generation(
        &self,
        request_builder: RequestBuilder,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> Result<()> {
        let mut current_request_builder = request_builder;
        let mut has_more_tool_calls = true;
        let mut _final_accumulated_content = String::new();

        while has_more_tool_calls {
            let prompt_token_estimate = self.estimate_prompt_tokens(&current_request_builder);

            let mut stream = self
                .backend
                .stream_chat_request(current_request_builder.clone())
                .await?;
            let mut accumulated_tool_calls: Vec<ToolCallResponse> = Vec::new();
            let mut accumulated_content = String::new();
            has_more_tool_calls = false;

            // Track timing for stats calculation (even on cancel)
            let stream_start_time = std::time::Instant::now();
            let mut first_token_time: Option<std::time::Instant> = None;
            let mut total_generated_tokens: usize = 0; // Tracks ALL tokens (thinking + completion) for tok/sec

            // Track thinking state
            // Start in non-thinking mode; switch to thinking when <think> is detected
            let mut in_thinking = false;
            let mut thinking_buffer = String::new();
            let mut allow_thinking_start = true;
            let mut pending_prefix = String::new();
            let mut pending_agent_response_prefix = String::new();
            let mut final_response_started = false;

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
                                for (summary, token_count, chunk_count) in
                                    summarizer_guard.get_new_summaries()
                                {
                                    let _ = tx.send(AgentMessage::ThinkingSummary(format!(
                                        "{}|{}|{}",
                                        summary, token_count, chunk_count
                                    )));
                                }
                            }

                            // Send any accumulated content as partial response
                            if !accumulated_content.is_empty() && !in_thinking {
                                let token_count = self.count_tokens(&accumulated_content);
                                let _ = tx
                                    .send(AgentMessage::AgentResponse(accumulated_content.clone(), token_count));
                            }

                            // Send partial GenerationStats even when cancelled
                            // This ensures context tracking continues to work
                            // Use local timing to calculate proper stats
                            let elapsed_sec = stream_start_time.elapsed().as_secs_f32();
                            let time_to_first = first_token_time
                                .map(|t| t.duration_since(stream_start_time).as_secs_f32())
                                .unwrap_or(0.0);
                            let completion_tokens = self.count_tokens(&accumulated_content);
                            // Calculate tok/sec using ALL generated tokens (thinking + completion)
                            let avg_tok_per_sec = if elapsed_sec > 0.0 && total_generated_tokens > 0 {
                                total_generated_tokens as f32 / elapsed_sec
                            } else {
                                0.0
                            };
                            let stats = GenerationStats {
                                avg_completion_tok_per_sec: avg_tok_per_sec,
                                completion_tokens,
                                prompt_tokens: prompt_token_estimate,
                                time_to_first_token_sec: time_to_first,
                                stop_reason: "cancelled".to_string(),
                            };
                            let _ = tx.send(AgentMessage::GenerationStats(stats));

                            // Save conversation state before returning so context isn't lost
                            // Add the partial response (if any) to preserve context
                            let mut updated_request = current_request_builder.clone();
                            if !accumulated_content.is_empty() {
                                updated_request = updated_request
                                    .add_message(TextMessageRole::Assistant, &accumulated_content);
                            }
                            let mut conversation_guard = self.conversation.lock().await;
                            *conversation_guard = Some(updated_request);
                            drop(conversation_guard);

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
                                let stop_reason = choices
                                    .first()
                                    .and_then(|c| c.finish_reason.as_ref())
                                    .cloned()
                                    .unwrap_or_else(|| "unknown".to_string());
                                let prompt_tokens = if usage_stats.prompt_tokens > 0 {
                                    usage_stats.prompt_tokens
                                } else if usage_stats.total_tokens > usage_stats.completion_tokens
                                {
                                    usage_stats.total_tokens - usage_stats.completion_tokens
                                } else {
                                    0
                                };

                                let stats = GenerationStats {
                                    avg_completion_tok_per_sec: usage_stats.avg_compl_tok_per_sec,
                                    completion_tokens: usage_stats.completion_tokens,
                                    prompt_tokens,
                                    time_to_first_token_sec: usage_stats.total_prompt_time_sec,
                                    stop_reason,
                                };

                                let _ = tx.send(AgentMessage::GenerationStats(stats));
                            }
                        }

                        if let Some(choice) = choices.first() {
                            match &choice.delta {
                                Delta {
                                    content: Some(content),
                                    tool_calls: None,
                                    ..
                                } => {
                                    if content.is_empty() {
                                        continue;
                                    }

                                    let thinking_tags_guard = self.thinking_tags.lock().await;
                                    let open_tag = thinking_tags_guard.open_tag.clone();
                                    drop(thinking_tags_guard);

                                    let mut chunk_content = content.clone();
                                    let mut process_as_thinking = in_thinking;

                                    if allow_thinking_start {
                                        pending_prefix.push_str(&chunk_content);

                                        match analyze_thinking_start(&pending_prefix, &open_tag) {
                                            ThinkingStartDecision::NeedMoreData => {
                                                check_cancel!();
                                                continue;
                                            }
                                            ThinkingStartDecision::Detected { content_start_idx } => {
                                                thinking_debug_log("Detected <think> start in HTTP chunk");
                                                allow_thinking_start = false;
                                                in_thinking = true;
                                                process_as_thinking = true;
                                                let after_tag = pending_prefix.split_off(content_start_idx);
                                                pending_prefix.clear();
                                                chunk_content = after_tag;
                                            }
                                            ThinkingStartDecision::NotThinking => {
                                                thinking_debug_log("Chunk does not start with <think>, treating as visible content");
                                                allow_thinking_start = false;
                                                chunk_content = pending_prefix.clone();
                                                pending_prefix.clear();
                                            }
                                        }
                                    }

                                    if process_as_thinking {
                                        if chunk_content.is_empty() {
                                            continue;
                                        }

                                        thinking_buffer.push_str(&chunk_content);

                                        let thinking_tags_guard = self.thinking_tags.lock().await;
                                        let close_tag = thinking_tags_guard.close_tag.clone();
                                        drop(thinking_tags_guard);

                                        let end_tag_result = thinking_buffer
                                            .find(close_tag.as_str())
                                            .map(|idx| (idx, close_tag.len()));

                                        if let Some((end_idx, end_tag_len)) = end_tag_result {
                                            in_thinking = false;
                                            thinking_debug_log("Detected </think> closing tag");

                                            let final_thinking = &thinking_buffer[..end_idx];
                                            if !final_thinking.is_empty() {
                                                let token_count = self.count_tokens(final_thinking);
                                                // Track first token time and total tokens for stats
                                                if first_token_time.is_none() && token_count > 0 {
                                                    first_token_time = Some(std::time::Instant::now());
                                                }
                                                total_generated_tokens += token_count;
                                                let _ = tx.send(AgentMessage::ThinkingContent(
                                                    final_thinking.to_string(),
                                                    token_count,
                                                ));
                                                thinking_debug_log(format!(
                                                    "Sent ThinkingContent (final) tokens={} preview=\"{}\"",
                                                    token_count,
                                                    preview_thinking(final_thinking)
                                                ));
                                                check_cancel!();

                                                let mut summarizer_guard =
                                                    self.thinking_summarizer.lock().await;
                                                summarizer_guard.add_thinking_chunk(final_thinking).await;
                                                for (summary, token_count, chunk_count) in
                                                    summarizer_guard.get_new_summaries()
                                                {
                                                    let _ = tx.send(AgentMessage::ThinkingSummary(
                                                        format!(
                                                            "{}|{}|{}",
                                                            summary, token_count, chunk_count
                                                        ),
                                                    ));
                                                    check_cancel!();
                                                }
                                            }

                                            let mut summarizer_guard =
                                                self.thinking_summarizer.lock().await;
                                            summarizer_guard.flush().await;
                                            for (summary, token_count, chunk_count) in
                                                summarizer_guard.get_new_summaries()
                                            {
                                                let _ = tx.send(AgentMessage::ThinkingSummary(
                                                    format!(
                                                        "{}|{}|{}",
                                                        summary, token_count, chunk_count
                                                    ),
                                                ));
                                                check_cancel!();
                                            }
                                            let residual_tokens =
                                                summarizer_guard.get_residual_token_count();
                                            if residual_tokens > 0 {
                                                let _ = tx.send(AgentMessage::ThinkingComplete(
                                                    residual_tokens,
                                                ));
                                            }

                                            let after_think =
                                                &thinking_buffer[end_idx + end_tag_len..];
                                            if !after_think.is_empty() {
                                                if has_visible_text(after_think) {
                                                    let mut outbound = String::new();
                                                    if !pending_agent_response_prefix.is_empty() {
                                                        outbound.push_str(
                                                            &pending_agent_response_prefix,
                                                        );
                                                        pending_agent_response_prefix.clear();
                                                    }
                                                    outbound.push_str(after_think);
                                                    accumulated_content.push_str(&outbound);
                                                    let token_count = self.count_tokens(&outbound);
                                                    // Track first token time and total tokens for stats
                                                    if first_token_time.is_none() && token_count > 0 {
                                                        first_token_time = Some(std::time::Instant::now());
                                                    }
                                                    total_generated_tokens += token_count;
                                                    let _ = tx.send(AgentMessage::AgentResponse(
                                                        outbound,
                                                        token_count,
                                                    ));
                                                    final_response_started = true;
                                                } else {
                                                    pending_agent_response_prefix
                                                        .push_str(after_think);
                                                }
                                            }
                                            thinking_buffer.clear();
                                        } else {
                                            let char_count = thinking_buffer.chars().count();
                                            if char_count > 11 {
                                                let send_char_count = char_count - 11;
                                                if let Some((byte_idx, _)) = thinking_buffer
                                                    .char_indices()
                                                    .nth(send_char_count)
                                                {
                                                    let to_send = &thinking_buffer[..byte_idx];
                                                    let mut remaining = to_send;
                                                    while !remaining.is_empty() {
                                                        check_cancel!();

                                                        let chunk_chars =
                                                            remaining.chars().take(100).count();
                                                        if let Some((chunk_byte_end, _)) =
                                                            remaining.char_indices().nth(chunk_chars)
                                                        {
                                                            let chunk = &remaining[..chunk_byte_end];
                                                            let token_count = self.count_tokens(chunk);
                                                            // Track first token time and total tokens for stats
                                                            if first_token_time.is_none() && token_count > 0 {
                                                                first_token_time = Some(std::time::Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(AgentMessage::ThinkingContent(
                                                                chunk.to_string(),
                                                                token_count,
                                                            ));
                                                            thinking_debug_log(format!(
                                                                "Sent ThinkingContent (stream) tokens={} preview=\"{}\"",
                                                                token_count,
                                                                preview_thinking(chunk)
                                                            ));
                                                            remaining = &remaining[chunk_byte_end..];
                                                        } else {
                                                            let token_count = self.count_tokens(remaining);
                                                            // Track first token time and total tokens for stats
                                                            if first_token_time.is_none() && token_count > 0 {
                                                                first_token_time = Some(std::time::Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(AgentMessage::ThinkingContent(
                                                                remaining.to_string(),
                                                                token_count,
                                                            ));
                                                            thinking_debug_log(format!(
                                                                "Sent ThinkingContent (final chunk) tokens={} preview=\"{}\"",
                                                                token_count,
                                                                preview_thinking(remaining)
                                                            ));
                                                            break;
                                                        }

                                                        check_cancel!();
                                                    }

                                                    let mut summarizer_guard =
                                                        self.thinking_summarizer.lock().await;
                                                    summarizer_guard.add_thinking_chunk(to_send).await;
                                                    for (summary, token_count, chunk_count) in
                                                        summarizer_guard.get_new_summaries()
                                                    {
                                                        let _ = tx.send(AgentMessage::ThinkingSummary(
                                                            format!(
                                                                "{}|{}|{}",
                                                                summary, token_count, chunk_count
                                                            ),
                                                        ));
                                                        check_cancel!();
                                                    }

                                                    thinking_buffer = thinking_buffer[byte_idx..].to_string();
                                                }
                                            }
                                        }
                                    } else {
                                        let chunk_has_visible = has_visible_text(&chunk_content);
                                        if !final_response_started && !chunk_has_visible {
                                            pending_agent_response_prefix
                                                .push_str(&chunk_content);
                                            check_cancel!();
                                            continue;
                                        }

                                        let mut outbound = String::new();
                                        if !pending_agent_response_prefix.is_empty() {
                                            outbound
                                                .push_str(&pending_agent_response_prefix);
                                            pending_agent_response_prefix.clear();
                                        }
                                        outbound.push_str(&chunk_content);
                                        accumulated_content.push_str(&outbound);
                                        let token_count = self.count_tokens(&outbound);
                                        // Track first token time and total tokens for stats
                                        if first_token_time.is_none() && token_count > 0 {
                                            first_token_time = Some(std::time::Instant::now());
                                        }
                                        total_generated_tokens += token_count;
                                        let _ = tx.send(AgentMessage::AgentResponse(outbound.clone(), token_count));
                                        if chunk_has_visible {
                                            final_response_started = true;
                                        }

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
                                        // Track first token time and total tokens for stats
                                        if first_token_time.is_none() && token_count > 0 {
                                            first_token_time = Some(std::time::Instant::now());
                                        }
                                        total_generated_tokens += token_count;
                                        let _ = tx.send(AgentMessage::ThinkingContent(
                                            thinking_buffer.clone(),
                                            token_count,
                                        ));
                                        thinking_debug_log(format!(
                                            "Flushing thinking content before tool call tokens={} preview=\"{}\"",
                                            token_count,
                                            preview_thinking(&thinking_buffer)
                                        ));

                                        // Add to summarizer
                                        let mut summarizer_guard =
                                            self.thinking_summarizer.lock().await;
                                        summarizer_guard.add_thinking_chunk(&thinking_buffer).await;

                                        // Force flush for residual
                                        summarizer_guard.flush().await;

                                        // Send new summaries
                                        for (summary, token_count, chunk_count) in
                                            summarizer_guard.get_new_summaries()
                                        {
                                            let _ =
                                                tx.send(AgentMessage::ThinkingSummary(format!(
                                                    "{}|{}|{}",
                                                    summary, token_count, chunk_count
                                                )));
                                        }

                                        // Send completion if residual >0 (though flush should handle)
                                        let residual_tokens =
                                            summarizer_guard.get_residual_token_count();
                                        if residual_tokens > 0 {
                                            let _ = tx.send(AgentMessage::ThinkingComplete(
                                                residual_tokens,
                                            ));
                                        }

                                        thinking_buffer.clear();
                                        in_thinking = false;
                                    }

                                    accumulated_tool_calls.extend(tool_calls.clone());
                                    for tool_call in tool_calls {
                                        let _ = tx.send(AgentMessage::ToolCallStarted(
                                            tool_call.function.name.clone(),
                                            tool_call.function.arguments.clone(),
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
                            let stop_reason = response
                                .choices
                                .first()
                                .map(|c| c.finish_reason.clone())
                                .unwrap_or_else(|| "unknown".to_string());
                            let prompt_tokens = if response.usage.prompt_tokens > 0 {
                                response.usage.prompt_tokens
                            } else if response.usage.total_tokens > response.usage.completion_tokens
                            {
                                response.usage.total_tokens - response.usage.completion_tokens
                            } else {
                                0
                            };

                            let stats = GenerationStats {
                                avg_completion_tok_per_sec: response.usage.avg_compl_tok_per_sec,
                                completion_tokens: response.usage.completion_tokens,
                                prompt_tokens,
                                time_to_first_token_sec: response.usage.total_prompt_time_sec,
                                stop_reason,
                            };

                            // Send stats before Done
                            let _ = tx.send(AgentMessage::GenerationStats(stats));
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

            if !pending_prefix.is_empty() {
                accumulated_content.push_str(&pending_prefix);
                let token_count = self.count_tokens(&pending_prefix);
                // Track first token time and total tokens for stats
                if first_token_time.is_none() && token_count > 0 {
                    first_token_time = Some(std::time::Instant::now());
                }
                total_generated_tokens += token_count;
                let _ = tx.send(AgentMessage::AgentResponse(pending_prefix.clone(), token_count));
            }

            // After stream ends, if still in thinking (no </think> found), flush residual
            if in_thinking && !thinking_buffer.is_empty() {
                let token_count = self.count_tokens(&thinking_buffer);
                // Track first token time and total tokens for stats
                if first_token_time.is_none() && token_count > 0 {
                    first_token_time = Some(std::time::Instant::now());
                }
                total_generated_tokens += token_count;
                let _ = tx.send(AgentMessage::ThinkingContent(
                    thinking_buffer.clone(),
                    token_count,
                ));
                thinking_debug_log(format!(
                    "Residual thinking flush tokens={} preview=\"{}\"",
                    token_count,
                    preview_thinking(&thinking_buffer)
                ));

                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                summarizer_guard.flush().await;
                for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                    let _ = tx.send(AgentMessage::ThinkingSummary(format!(
                        "{}|{}|{}",
                        summary, token_count, chunk_count
                    )));
                }
                let residual_tokens = summarizer_guard.get_residual_token_count();
                if residual_tokens > 0 {
                    let _ = tx.send(AgentMessage::ThinkingComplete(residual_tokens));
                }
            }

            // Store the accumulated content for final logging
            if accumulated_tool_calls.is_empty() {
                _final_accumulated_content = accumulated_content.clone();
            }

            if !accumulated_tool_calls.is_empty() {
                has_more_tool_calls = true;
                for tool_call in accumulated_tool_calls {
                    // Execute tool call and handle errors gracefully
                    let tool_result = match execute_tool_call(&tool_call, tx.clone()).await {
                        Ok(result) => {
                            let _ = tx.send(AgentMessage::ToolCallCompleted(
                                tool_call.function.name.clone(),
                                result.clone(),
                            ));

                            // Check if this was a background exec_command and extract session info
                            if tool_call.function.name == "exec_command" {
                                if let Ok(parsed) =
                                    serde_yaml::from_str::<serde_json::Value>(&result)
                                {
                                    if let Some(status) =
                                        parsed.get("status").and_then(|v| v.as_str())
                                    {
                                        if status == "Background" {
                                            let session_id = parsed
                                                .get("session_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let command = parsed
                                                .get("command")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let log_file = parsed
                                                .get("log_file")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let _ = tx.send(AgentMessage::BackgroundTaskStarted(
                                                session_id, command, log_file,
                                            ));
                                        }
                                    }
                                }
                            }

                            result
                        }
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
                            let error_msg = serde_yaml::to_string(&error_obj)
                                .unwrap_or_else(|_| "error: Failed to serialize error".to_string());
                            let _ =
                                tx.send(AgentMessage::Error(format!("Tool call failed: {}", e)));
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

fn build_summary(task: &Task, artifact_override: Option<&[String]>) -> TaskSummary {
    let (step_index, instructions) = extract_step_context(task);
    let artifacts = gather_artifacts(artifact_override);
    let commands = extract_tool_commands(task);
    let agent_response = extract_agent_message(task);
    let command_summary = if commands.is_empty() {
        "none".to_string()
    } else {
        commands.join(" | ")
    };
    let artifact_summary = if artifacts.is_empty() {
        "none".to_string()
    } else {
        artifacts.join(", ")
    };

    let summary_text = format!(
        "Step {step_index} summary:\nInstructions: {instructions}\nCommands: {command_summary}\nArtifacts: {artifact_summary}\nAgent result: {agent_response}"
    );

    TaskSummary {
        task_id: task.id.clone(),
        step_index,
        summary_text,
        artifacts_touched: artifacts,
        tests_run: Vec::<TestRun>::new(),
        verification: TaskVerification {
            status: VerificationStatus::Pending,
            feedback: vec![],
        },
    }
}

fn gather_artifacts(artifact_override: Option<&[String]>) -> Vec<String> {
    if let Some(values) = artifact_override {
        return values.iter().map(|value| value.to_string()).collect();
    }
    gather_git_changes()
}

fn gather_git_changes() -> Vec<String> {
    match Command::new("git").args(["status", "--short"]).output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    let path = if trimmed.len() > 3 {
                        trimmed[3..].trim()
                    } else {
                        trimmed
                    };
                    if path.is_empty() {
                        None
                    } else {
                        Some(path.to_string())
                    }
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn extract_tool_commands(task: &Task) -> Vec<String> {
    task
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.extra.get("toolLog"))
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("tool");
                    let args = entry
                        .get("arguments")
                        .and_then(|value| value.as_str())
                        .unwrap_or("{}");
                    let result = entry
                        .get("result")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let mut line = format!("{} -> {}", name, args);
                    if !result.is_empty() {
                        line.push_str(&format!(" = {}", result));
                    }
                    Some(line)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_agent_message(task: &Task) -> String {
    task
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Agent)
        .map(|message| message.text_content())
        .unwrap_or_else(|| "No agent response".to_string())
}

fn extract_step_context(task: &Task) -> (String, String) {
    if let Some(metadata) = &task.metadata {
        let index = metadata
            .extra
            .get("stepIndex")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let instructions = metadata
            .extra
            .get("stepInstructions")
            .and_then(|value| value.as_str())
            .unwrap_or("(no instructions)")
            .to_string();
        return (index, instructions);
    }
    ("unknown".to_string(), "(no instructions)".to_string())
}

fn build_split_spec(step: &SpecStep) -> Result<SpecSheet> {
    let created_at = Utc::now();
    let mut child_steps: Vec<SpecStep> = step
        .instructions
        .split(|ch| ch == '\n')
        .map(|fragment| fragment.trim())
        .filter(|fragment| !fragment.is_empty())
        .enumerate()
        .map(|(idx, fragment)| SpecStep {
            index: (idx + 1).to_string(),
            title: format!("{} - subtask {}", step.title, idx + 1),
            instructions: fragment.to_string(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        })
        .collect();

    if child_steps.is_empty() {
        child_steps.push(SpecStep {
            index: "1".to_string(),
            title: format!("{} - detail", step.title),
            instructions: step.instructions.clone(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        });
    }

    let child_spec = SpecSheet {
        id: format!("{}::split", step.index),
        title: format!("{} (split)", step.title),
        description: format!("Split from parent step {}", step.index),
        steps: child_steps,
        created_by: "nite-agent".to_string(),
        created_at,
        metadata: json!({"source": "split"}),
    };

    child_spec.validate()?;
    Ok(child_spec)
}

fn build_split_summary(task: &Task, step: &SpecStep, child: &SpecSheet) -> TaskSummary {
    TaskSummary {
        task_id: task.id.clone(),
        step_index: step.index.clone(),
        summary_text: format!(
            "Step {} split into spec {} with {} steps",
            step.index,
            child.id,
            child.steps.len()
        ),
        artifacts_touched: Vec::new(),
        tests_run: Vec::new(),
        verification: TaskVerification {
            status: VerificationStatus::Pending,
            feedback: vec![],
        },
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use agent_protocol::types::message::{Message, Role};
    use agent_protocol::types::task::TaskMetadata;

    #[test]
    fn summary_includes_commands_and_artifacts() {
        let mut task = Task::new();
        task.id = "task-1".to_string();
        let mut agent_msg = Message::agent("Done".to_string());
        agent_msg.role = Role::Agent;
        agent_msg.task_id = Some(task.id.clone());
        task.messages.push(agent_msg);

        let mut metadata = TaskMetadata::default();
        metadata
            .extra
            .insert("stepIndex".to_string(), json!("1"));
        metadata
            .extra
            .insert("stepInstructions".to_string(), json!("Implement feature"));
        metadata.extra.insert(
            "toolLog".to_string(),
            json!([
                {
                    "name": "exec_command",
                    "arguments": "cargo test",
                    "result": "ok"
                }
            ]),
        );
        task.metadata = Some(metadata);

        let summary = build_summary(&task, Some(&["src/lib.rs".into()]));
        assert!(summary.summary_text.contains("Implement feature"));
        assert!(summary.summary_text.contains("cargo test"));
        assert_eq!(summary.artifacts_touched, vec!["src/lib.rs".to_string()]);
        serde_json::to_string(&summary).expect("summary serializes");
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
