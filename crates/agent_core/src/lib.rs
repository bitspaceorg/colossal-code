use anyhow::Result;
// Re-export spec types from agent_protocol for convenience
pub use agent_protocol::types::spec::{
    SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus,
};
use agent_protocol::types::{message::Role, task::Task};
use chrono::Utc;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use colossal_linux_sandbox::types::{ExitStatus, SessionId};
use futures::StreamExt;
use indexmap::IndexMap;
use mistralrs::{
    CalledFunction, ChatCompletionChunkResponse, Delta, Model, RequestBuilder, RequestLike,
    Response, TextMessageRole, Tool, ToolCallResponse, ToolCallType, ToolChoice,
};
use once_cell::sync::OnceCell;
use serde::Serialize;
use serde_json::{Value, json};
use shell_escape::escape;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};

static WORKSPACE_ROOT_OVERRIDE: OnceCell<std::sync::Mutex<Option<PathBuf>>> = OnceCell::new();

pub mod config;
mod llm_backend;
pub use llm_backend::{HttpBackend, LLMBackend, LocalBackend, NoneBackend};
pub mod a2a;
pub mod model_config;
pub mod orchestrator;
pub mod safety_config;
pub mod thinking_summarizer;
pub mod tools;
pub mod web_search;

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
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
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

fn push_writable_root_unique(
    writable_roots: &mut Vec<colossal_linux_sandbox::protocol::WritableRoot>,
    root: PathBuf,
) {
    if writable_roots.iter().any(|existing| existing.root == root) {
        return;
    }
    writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
        root,
        recursive: true,
        read_only_subpaths: vec![],
    });
}

fn sandbox_policy_from_config_with_workspace(
    safety_config: &safety_config::SafetyConfig,
    workspace_path: PathBuf,
) -> SandboxPolicy {
    let mut writable_roots = Vec::new();
    push_writable_root_unique(&mut writable_roots, workspace_path.clone());

    if let Some(parent) = workspace_path.parent() {
        push_writable_root_unique(&mut writable_roots, parent.to_path_buf());
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_parent) = exe_path.parent().and_then(|p| p.parent()) {
            push_writable_root_unique(&mut writable_roots, exe_parent.to_path_buf());
        }
    }

    if let Ok(tools_path) = resolve_tools_binary_path_for_runtime()
        && let Some(tools_parent) = tools_path.parent()
    {
        push_writable_root_unique(&mut writable_roots, tools_parent.to_path_buf());
    }

    if let Ok(extra_roots) = std::env::var("SANDBOX_EXTRA_ROOTS") {
        for root_path in extra_roots.split(':') {
            if !root_path.is_empty() {
                writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                    root: PathBuf::from(root_path),
                    recursive: true,
                    read_only_subpaths: vec![],
                });
            }
        }
    }

    match safety_config.mode {
        safety_config::SafetyMode::ReadOnly => SandboxPolicy::ReadOnly,
        safety_config::SafetyMode::Regular => {
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
        safety_config::SafetyMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

fn sandbox_policy_from_config(safety_config: &safety_config::SafetyConfig) -> SandboxPolicy {
    sandbox_policy_from_config_with_workspace(safety_config, resolve_workspace_root())
}

pub(crate) fn workspace_root_override() -> Option<PathBuf> {
    WORKSPACE_ROOT_OVERRIDE
        .get_or_init(|| {
            std::sync::Mutex::new(std::env::var("NITE_WORKSPACE_ROOT").ok().and_then(|raw| {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let candidate = PathBuf::from(trimmed);
                if candidate.is_absolute() {
                    Some(candidate)
                } else {
                    std::env::current_dir().ok().map(|cwd| cwd.join(candidate))
                }
            }))
        })
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(None)
}

pub fn set_workspace_root_override(path: impl AsRef<Path>) {
    let absolute = if path.as_ref().is_absolute() {
        path.as_ref().to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path.as_ref())
    };
    let slot = WORKSPACE_ROOT_OVERRIDE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(absolute);
    }
}

pub(crate) fn resolve_workspace_root() -> PathBuf {
    workspace_root_override()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn resolve_tools_binary_path_for_runtime() -> Result<PathBuf> {
    colossal_linux_sandbox::resolve_tools_binary_path().map_err(|e| anyhow::anyhow!("{}", e))
}

fn role_from_str(role: &str) -> TextMessageRole {
    match role {
        "system" => TextMessageRole::System,
        "user" => TextMessageRole::User,
        "assistant" => TextMessageRole::Assistant,
        "tool" => TextMessageRole::Tool,
        other => TextMessageRole::Custom(other.to_string()),
    }
}

fn value_left_str(value: &serde_json::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("Left").and_then(|inner| inner.as_str()))
}

fn message_left_str<'a>(message: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    message.get(key).and_then(value_left_str)
}

fn message_right<'a>(message: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    message
        .get(key)
        .and_then(|value| value.get("Right").or(Some(value)))
}

fn parse_tool_calls(value: &serde_json::Value) -> Vec<ToolCallResponse> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, entry)| {
            let id = entry.get("id")?.as_str()?.to_string();
            let function = entry.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let arguments = function.get("arguments")?.as_str()?.to_string();
            Some(ToolCallResponse {
                index,
                id,
                tp: ToolCallType::Function,
                function: CalledFunction { name, arguments },
            })
        })
        .collect()
}

fn rebuild_request_builder(
    existing: &RequestBuilder,
    system_prompt: &str,
    tools: Vec<Tool>,
    reminder: Option<&str>,
) -> Result<RequestBuilder> {
    let serialized = serde_json::to_value(existing.messages_ref())?;
    let mut builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, system_prompt)
        .set_tools(tools)
        .set_tool_choice(ToolChoice::Auto)
        .enable_thinking(true);

    if let Some(messages) = serialized.as_array() {
        for message in messages {
            let Some(role) = message_left_str(message, "role") else {
                continue;
            };
            if role == "system" {
                continue;
            }

            if role == "tool"
                && let (Some(content), Some(tool_call_id)) = (
                    message_left_str(message, "content"),
                    message_left_str(message, "tool_call_id"),
                )
            {
                builder = builder.add_tool_message(content, tool_call_id);
                continue;
            }

            let role = role_from_str(role);
            let content_text = message_left_str(message, "content").unwrap_or_default();

            if let Some(functions) = message_right(message, "function") {
                let tool_calls = parse_tool_calls(functions);
                if !tool_calls.is_empty() {
                    builder = builder.add_message_with_tool_call(role, content_text, tool_calls);
                    continue;
                }
            }

            if !content_text.is_empty() {
                builder = builder.add_message(role, content_text);
            }
        }
    }

    if let Some(reminder) = reminder {
        builder = builder.add_message(TextMessageRole::System, reminder);
    }

    Ok(builder)
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

async fn execute_tool_binary(
    args: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
) -> Result<String> {
    // 10 minute timeout for tool execution to allow cargo/npm/etc operations
    let timeout_duration = std::time::Duration::from_secs(600);
    let output = match tokio::time::timeout(
        timeout_duration,
        execute_tools_with_sandbox(args, sandbox_policy, cwd),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Tool execution timed out after {} seconds",
                timeout_duration.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Tool execution failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn ensure_global_state_initialized() {
    if GLOBAL_STATE.get().is_none() {
        let shell = colossal_linux_sandbox::shell::default_user_shell().await;
        let safety_config = safety_config::SafetyConfig::load().unwrap_or_default();
        let sandbox_policy = sandbox_policy_from_config(&safety_config);

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

pub(crate) async fn get_or_create_shell_session() -> Result<(
    Arc<colossal_linux_sandbox::manager::SessionManager>,
    colossal_linux_sandbox::types::SessionId,
)> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE.get().unwrap();
    let mut session_id_lock = state.shell_session_id.lock().await;

    if let Some(existing) = session_id_lock.clone() {
        if state.manager.get_session_info(existing.clone()).is_none() {
            *session_id_lock = None;
            let mut background = state.session_has_background_process.lock().await;
            *background = false;
        }
    }

    let has_background = state.session_has_background_process.lock().await;

    // Create new session if:
    // 1. No session exists yet, OR
    // 2. Current session has a background process running
    if session_id_lock.is_none() || *has_background {
        let workspace_path = resolve_workspace_root();

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
                format!("cd {}", escape(workspace_path.to_string_lossy())),
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
    agent: &Agent,
    tool_call: &ToolCallResponse,
    tx: mpsc::UnboundedSender<AgentMessage>,
) -> Result<String> {
    ensure_global_state_initialized().await;

    if let Some(state) = GLOBAL_STATE.get() {
        let safety_cfg = agent.safety_config.lock().await.clone();
        let workspace_path = agent.effective_cwd();
        let refreshed_policy =
            sandbox_policy_from_config_with_workspace(&safety_cfg, workspace_path);
        let mut policy_guard = state.sandbox_policy.lock().await;
        *policy_guard = refreshed_policy;
    }

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
            // Parse timeout from arguments, default to 10 minutes (600000ms) for long-running commands
            let timeout_ms = arguments
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(600_000); // 10 minutes default

            let requires_approval = { agent.safety_config.lock().await.ask_permission };
            let safety_mode = if requires_approval {
                colossal_linux_sandbox::safety::AskForApproval::OnRequest
            } else {
                colossal_linux_sandbox::safety::AskForApproval::Never
            };

            let mut current_approval = Some(safety_mode);
            let mut retried_session = false;

            loop {
                // Get or create shell session (will create new one if current has background process)
                let (manager, session_id) = get_or_create_shell_session().await?;

                if is_background {
                    // Mark session as busy BEFORE starting the command
                    let mut has_background = state.session_has_background_process.lock().await;
                    *has_background = true;
                    drop(has_background); // Release lock

                    // Create log file for background output
                    let log_file_path =
                        colossal_linux_sandbox::manager::background_log_path(&session_id)
                            .display()
                            .to_string();

                    // Run command in background with output redirected to log file
                    // Strip trailing & if present since we'll add it with redirection
                    let command_clean = command.trim_end().trim_end_matches('&').trim_end();
                    let bg_command = format!(
                        "{} > {} 2>&1 &",
                        agent.wrap_command_for_shell(command_clean),
                        log_file_path
                    );

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
                            if matches!(
                                &e,
                                colossal_linux_sandbox::error::ColossalErr::Io(err)
                                    if err.to_string().contains("failed to send input to shell")
                            ) && !retried_session
                            {
                                retried_session = true;
                                if let Err(err) =
                                    state.manager.terminate_session(session_id.clone()).await
                                {
                                    tracing::warn!(
                                        "Failed to terminate broken shell session: {}",
                                        err
                                    );
                                }
                                let mut session_id_lock = state.shell_session_id.lock().await;
                                *session_id_lock = None;
                                let mut has_background =
                                    state.session_has_background_process.lock().await;
                                *has_background = false;
                                continue;
                            }

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
                            return Ok(serde_yaml::to_string(&json!({
                                "status": "Failure",
                                "command": command,
                                "message": format!("{}", e)
                            }))?);
                        }
                    }
                } else {
                    // Foreground command - wait for completion
                    match manager
                        .exec_command_in_shell_session(
                            session_id.clone(),
                            agent.wrap_command_for_shell(command),
                            Some(timeout_ms),
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
                            if matches!(
                                &e,
                                colossal_linux_sandbox::error::ColossalErr::Io(err)
                                    if err.to_string().contains("failed to send input to shell")
                            ) && !retried_session
                            {
                                retried_session = true;
                                if let Err(err) =
                                    state.manager.terminate_session(session_id.clone()).await
                                {
                                    tracing::warn!(
                                        "Failed to terminate broken shell session: {}",
                                        err
                                    );
                                }
                                let mut session_id_lock = state.shell_session_id.lock().await;
                                *session_id_lock = None;
                                continue;
                            }

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
                            return Ok(serde_yaml::to_string(&json!({
                                "status": "Failure",
                                "command": command,
                                "message": format!("{}", e)
                            }))?);
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
        "submit_verification" => {
            let status = arguments["status"].as_str().unwrap_or("unknown");
            let feedback = arguments.get("feedback").and_then(|v| v.as_str());
            let end_convo = arguments
                .get("end_convo")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let payload = serde_json::json!({
                "status": "Success",
                "verification_status": status,
                "feedback": feedback,
                "end_convo": end_convo,
            });
            Ok(serde_yaml::to_string(&payload)?)
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

                    let start_byte = arguments
                        .get("start_byte_one_indexed")
                        .and_then(|v| v.as_u64());
                    let end_byte = arguments
                        .get("end_byte_one_indexed")
                        .and_then(|v| v.as_u64());

                    let start_line = arguments
                        .get("start_line_one_indexed")
                        .and_then(|v| v.as_u64());
                    let end_line = arguments
                        .get("end_line_one_indexed")
                        .and_then(|v| v.as_u64());
                    let line_limit = arguments.get("limit").and_then(|v| v.as_i64());
                    let line_offset = arguments.get("offset").and_then(|v| v.as_i64());

                    args.push(path.to_string());
                    if start_byte.is_some() || end_byte.is_some() {
                        args.push("bytes".to_string());
                        args.push(start_byte.map(|v| v.to_string()).unwrap_or_default());
                        args.push(end_byte.map(|v| v.to_string()).unwrap_or_default());
                    } else {
                        let mut use_lines = false;
                        let mut offset_lines: i64 = 0;
                        let mut limit_lines: Option<i64> = None;

                        if let Some(start) = start_line {
                            use_lines = true;
                            offset_lines = start.saturating_sub(1) as i64;
                            if let Some(end) = end_line {
                                if end >= start {
                                    limit_lines = Some((end - start + 1) as i64);
                                }
                            }
                        } else if line_limit.is_some() || line_offset.is_some() {
                            use_lines = true;
                            offset_lines = line_offset.unwrap_or(0);
                            limit_lines = line_limit;
                        }

                        if use_lines {
                            args.push("lines".to_string());
                            args.push(offset_lines.to_string());
                            let limit_arg = limit_lines.unwrap_or(-1);
                            args.push(limit_arg.to_string());
                        } else if should_read_entire {
                            args.push("entire".to_string());
                        } else {
                            // Default to the whole file when no explicit limits are provided
                            args.push("lines".to_string());
                            args.push("0".to_string());
                            args.push("-1".to_string());
                        }
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

                    let limit = arguments
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(200)
                        .min(200);
                    args.push(limit.to_string());

                    if let Some(offset) = arguments.get("offset").and_then(|v| v.as_u64()) {
                        args.push(offset.to_string());
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
                "search_files_with_regex" => {
                    // tools binary expects: <path> <regex_pattern> [limit] [case_sensitive]
                    let path = arguments["path"].as_str().unwrap_or(".");
                    let regex_pattern = arguments["regex_pattern"].as_str().unwrap_or("");
                    let case_sensitive = arguments["case_sensitive"].as_bool().unwrap_or(false);
                    let limit = arguments["limit"].as_u64();
                    args.push(path.to_string());
                    args.push(regex_pattern.to_string());
                    // limit is optional but must come before case_sensitive
                    args.push(
                        limit
                            .map(|l| l.to_string())
                            .unwrap_or_else(|| "1000".to_string()),
                    );
                    args.push(case_sensitive.to_string());
                }
                "delete_many" => {
                    // delete_many expects paths as a JSON array string
                    if let Some(paths) = arguments.get("paths").and_then(|v| v.as_array()) {
                        let paths_json =
                            serde_json::to_string(paths).unwrap_or_else(|_| "[]".to_string());
                        args.push(paths_json);
                    }
                }
                _ => {}
            }

            let sandbox_policy = state.sandbox_policy.lock().await.clone();
            let output = execute_tool_binary(args, &sandbox_policy, agent.effective_cwd()).await?;

            // Tools binary outputs YAML, just return it directly
            Ok(output)
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
        "orchestrate_task" => {
            let goal = arguments
                .get("goal")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let reason = arguments
                .get("reason")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let payload = serde_json::json!({
                "status": "orchestration_requested",
                "goal": goal,
                "reason": reason
            });
            Ok(serde_json::to_string(&payload)?)
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
    /// No backend configured — TUI starts in setup mode
    None,
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
    None,
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
    /// Conversation history (RequestBuilder maintains all messages)
    conversation: Arc<Mutex<Option<RequestBuilder>>>,
    /// Thinking tags configuration (opening/closing tags and summary interval)
    thinking_tags: Arc<Mutex<model_config::ThinkingTags>>,
    /// Safety configuration for tool access
    safety_config: Arc<Mutex<safety_config::SafetyConfig>>,
    /// Optional working directory override for orchestration (worktree support)
    working_directory: Option<PathBuf>,
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
            .or_else(|| crate::workspace_root_override())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Wrap a shell command so it executes inside the agent's working directory
    /// without mutating the shared shell session state.
    fn wrap_command_for_shell(&self, command: &str) -> String {
        if let Some(cwd) = self
            .working_directory
            .clone()
            .or_else(|| crate::workspace_root_override())
        {
            let escaped = escape(cwd.to_string_lossy());
            format!("(cd {} && {})", escaped, command)
        } else {
            command.to_string()
        }
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

        if let Some(state) = GLOBAL_STATE.get() {
            let mut policy_guard = state.sandbox_policy.lock().await;
            *policy_guard =
                sandbox_policy_from_config_with_workspace(&new_safety_config, self.effective_cwd());
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

    pub async fn inject_system_reminder(&self, reminder: &str) -> Result<()> {
        let tools = {
            let tools_guard = self.tools.lock().await;
            tools_guard.clone()
        };
        let system_prompt_content = {
            let system_prompt_guard = self.system_prompt.lock().await;
            system_prompt_guard.clone()
        };

        let mut conversation_guard = self.conversation.lock().await;
        let rebuilt = if let Some(ref conversation) = *conversation_guard {
            rebuild_request_builder(conversation, &system_prompt_content, tools, Some(reminder))?
        } else {
            RequestBuilder::new()
                .add_message(TextMessageRole::System, &system_prompt_content)
                .add_message(TextMessageRole::System, reminder)
                .set_tools(tools)
                .set_tool_choice(ToolChoice::Auto)
                .enable_thinking(true)
        };
        *conversation_guard = Some(rebuilt);
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
            let messages = request_builder.messages_ref();
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

    /// Execute a spec step.
    ///
    /// NOTE: Full LLM integration requires refactoring sessionizer to use tokio::sync::Mutex.
    /// For now, this creates the task structure and marks it as completed to allow
    /// orchestration flow to proceed.
    ///
    /// TODO: Integrate with process_message once sessionizer Send issue is resolved.
    pub async fn execute_step(&self, step: SpecStep, spec: &SpecSheet) -> Result<Task> {
        use agent_protocol::{Task, TaskState, TaskVerification, VerificationStatus};

        // Build the prompt that would be sent to the LLM
        let prompt = format!(
            "## Current Task: {}\n\n\
            **Spec:** {}\n\n\
            **Step {} of {}:** {}\n\n\
            **Instructions:**\n{}\n\n\
            **Acceptance Criteria:**\n{}",
            step.title,
            spec.title,
            step.index,
            spec.steps.len(),
            step.title,
            step.instructions,
            step.acceptance_criteria
                .iter()
                .map(|c| format!("- {}", c))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Build the task
        let mut task = Task::new();
        task.context_id = Some(spec.id.clone());

        // Create the TaskSummary that the orchestrator expects
        let summary = TaskSummary {
            task_id: task.id.clone(),
            step_index: step.index.clone(),
            summary_text: format!(
                "Step {} completed: {}\nInstructions: {}",
                step.index, step.title, step.instructions
            ),
            artifacts_touched: Vec::new(),
            tests_run: Vec::new(),
            verification: TaskVerification {
                status: VerificationStatus::Passed,
                feedback: Vec::new(),
            },
            worktree: None,
        };

        // Set metadata with step info AND the required summary
        let mut metadata = agent_protocol::TaskMetadata::default();
        metadata.summary = Some(serde_json::to_value(&summary)?);
        metadata.extra.insert(
            "step_index".to_string(),
            serde_json::Value::String(step.index.clone()),
        );
        metadata
            .extra
            .insert("prompt".to_string(), serde_json::Value::String(prompt));
        metadata.extra.insert(
            "step_title".to_string(),
            serde_json::Value::String(step.title.clone()),
        );
        task.metadata = Some(metadata);

        // Mark as completed
        task.set_state(
            TaskState::Completed,
            Some(format!("Step {} completed: {}", step.index, step.title)),
        );

        Ok(task)
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
        self.execute_step_with_events_and_cancel(step, spec, prefix, event_tx, None)
            .await
    }

    /// Execute a spec step with real LLM execution, emitting tool events.
    /// If `cancel_rx` is provided and receives a message, execution will be interrupted.
    pub async fn execute_step_with_events_and_cancel(
        &self,
        step: SpecStep,
        spec: &SpecSheet,
        prefix: &str,
        event_tx: Option<mpsc::UnboundedSender<crate::orchestrator::OrchestratorEvent>>,
        mut cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Result<Task> {
        use agent_protocol::{Task, TaskState, TaskVerification, VerificationStatus};
        use tokio::runtime::Builder;

        // Clear conversation from previous step - each orchestration step should start fresh
        self.clear_conversation().await;

        // Build the prompt from step instructions
        // Include worktree context if this agent has a working_directory set
        let worktree_context = if let Some(ref cwd) = self.working_directory {
            format!(
                "\n\n**IMPORTANT - Working Directory:**\n\
                You are working in an isolated git worktree at: {}\n\
                ALL file operations (create, edit, read) MUST use this directory as the base.\n\
                Use relative paths from this directory, or absolute paths starting with this path.\n\
                Do NOT use paths like ~/rust/todo or /home/user/... - use the worktree path instead.",
                cwd.display()
            )
        } else {
            String::new()
        };

        let prompt = format!(
            "## Current Task: {}\n\n\
            **Spec:** {}\n\n\
            **Step {} of {}:** {}\n\n\
            **Instructions:**\n{}{}\n\n\
            **Acceptance Criteria:**\n{}",
            step.title,
            spec.title,
            step.index,
            spec.steps.len(),
            step.title,
            step.instructions,
            worktree_context,
            step.acceptance_criteria
                .iter()
                .map(|c| format!("- {}", c))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Build the task
        let mut task = Task::new();
        task.context_id = Some(spec.id.clone());

        // Create channel for AgentMessage
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = self.clone();
        let prompt_clone = prompt.clone();

        // Use spawn_blocking pattern (like A2A) to handle sessionizer mutex
        let blocking = tokio::task::spawn_blocking(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| anyhow::anyhow!("failed to create agent runtime: {e}"))?;
            runtime.block_on(async move { agent.process_message(prompt_clone, tx).await })
        });

        // Process AgentMessage stream and emit orchestrator events
        let mut latest_response = String::new();
        let mut error_message: Option<String> = None;
        let mut tool_log: Vec<serde_json::Value> = Vec::new();
        let prefix_owned = prefix.to_string();
        let mut thinking_start_time = std::time::Instant::now();

        // Emit the user prompt as the first sub-agent message
        if let Some(ref tx) = event_tx {
            let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                prefix: prefix_owned.clone(),
                message: crate::orchestrator::SubAgentMessage::UserPrompt {
                    content: prompt.clone(),
                },
            });
        }

        let mut cancel_requested = false;

        loop {
            if cancel_requested || cancel_rx.is_none() {
                match rx.recv().await {
                    Some(agent_msg) => {
                        if Self::handle_agent_message(
                            agent_msg,
                            &mut latest_response,
                            &mut tool_log,
                            &mut thinking_start_time,
                            &mut error_message,
                            &event_tx,
                            &prefix_owned,
                        ) {
                            break;
                        }
                    }
                    None => break,
                }
            } else {
                // Safe to unwrap since we checked is_some above
                let mut cancel_stream = cancel_rx.take().unwrap();
                tokio::select! {
                    msg = rx.recv() => {
                        cancel_rx = Some(cancel_stream);
                        match msg {
                            Some(agent_msg) => {
                                if Self::handle_agent_message(
                                    agent_msg,
                                    &mut latest_response,
                                    &mut tool_log,
                                    &mut thinking_start_time,
                                    &mut error_message,
                                    &event_tx,
                                    &prefix_owned,
                                ) {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    signal = cancel_stream.recv() => {
                        if signal.is_some() {
                            cancel_requested = true;
                            self.request_cancel();
                        }
                        // Drop cancel_stream so future iterations don't poll again
                    }
                }
            }
        }

        let was_cancelled = cancel_requested;

        // Wait for blocking task
        match blocking.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if error_message.is_none() {
                    error_message = Some(err.to_string());
                }
            }
            Err(err) => {
                if error_message.is_none() {
                    error_message = Some(err.to_string());
                }
            }
        }

        if was_cancelled {
            let mut task = Task::new();
            task.context_id = Some(spec.id.clone());
            task.status = agent_protocol::TaskStatus {
                state: TaskState::Cancelled,
                timestamp: Some(chrono::Utc::now()),
                message: Some("Step cancelled by user".to_string()),
                error: None,
            };
            return Ok(task);
        }

        // Build summary and complete task
        if let Some(err) = error_message {
            task.set_state(TaskState::Failed, Some(err));
        } else {
            let summary = TaskSummary {
                task_id: task.id.clone(),
                step_index: step.index.clone(),
                summary_text: if latest_response.is_empty() {
                    format!("Step {} completed: {}", step.index, step.title)
                } else {
                    latest_response.clone()
                },
                artifacts_touched: Vec::new(),
                tests_run: Vec::new(),
                verification: TaskVerification {
                    status: VerificationStatus::Passed,
                    feedback: Vec::new(),
                },
                worktree: None,
            };

            let mut metadata = agent_protocol::TaskMetadata::default();
            metadata.summary = Some(serde_json::to_value(&summary)?);
            metadata.extra.insert(
                "step_index".to_string(),
                serde_json::Value::String(step.index.clone()),
            );
            metadata
                .extra
                .insert("prompt".to_string(), serde_json::Value::String(prompt));
            metadata.extra.insert(
                "step_title".to_string(),
                serde_json::Value::String(step.title.clone()),
            );
            metadata
                .extra
                .insert("toolLog".to_string(), serde_json::Value::Array(tool_log));
            task.metadata = Some(metadata);
            task.set_state(
                TaskState::Completed,
                Some(format!("Step {} completed", step.index)),
            );
        }

        Ok(task)
    }

    fn handle_agent_message(
        agent_msg: AgentMessage,
        latest_response: &mut String,
        tool_log: &mut Vec<serde_json::Value>,
        thinking_start_time: &mut std::time::Instant,
        error_message: &mut Option<String>,
        event_tx: &Option<mpsc::UnboundedSender<crate::orchestrator::OrchestratorEvent>>,
        prefix_owned: &String,
    ) -> bool {
        match agent_msg {
            AgentMessage::AgentResponse(content, _) => {
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::Text {
                            content: content.clone(),
                        },
                    });
                }
                *latest_response = content;
            }
            AgentMessage::ThinkingContent(content, _) => {
                *thinking_start_time = std::time::Instant::now();
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::Thinking {
                            content,
                            duration_secs: 0,
                        },
                    });
                }
            }
            AgentMessage::ThinkingComplete(_) => {
                let duration = thinking_start_time.elapsed().as_secs();
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::Thinking {
                            content: String::new(),
                            duration_secs: duration,
                        },
                    });
                }
            }
            AgentMessage::Error(err) => {
                *error_message = Some(err.clone());
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::Error {
                            message: err.clone(),
                        },
                    });
                }
                return true;
            }
            AgentMessage::ToolCallStarted(name, args) => {
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::ToolCallStarted {
                        prefix: prefix_owned.clone(),
                        tool_name: name.clone(),
                        arguments: args.clone(),
                    });
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::ToolCall {
                            tool_name: name.clone(),
                            arguments: args.clone(),
                            result: None,
                            is_error: false,
                        },
                    });
                }
                tool_log.push(serde_json::json!({
                    "name": name,
                    "arguments": args,
                    "result": serde_json::Value::Null,
                }));
            }
            AgentMessage::ToolCallCompleted(name, result) => {
                let is_error = result.starts_with("Error:")
                    || result.starts_with("error:")
                    || result.starts_with("Failed:")
                    || result.starts_with("failed:")
                    || result.starts_with("Permission denied")
                    || result.starts_with("No such file")
                    || result.starts_with("Command failed")
                    || (result.len() < 500 && result.contains("\"error\""))
                    || (result.len() < 500 && result.contains("\"is_error\": true"))
                    || result.contains("status: Failure");

                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::ToolCallCompleted {
                        prefix: prefix_owned.clone(),
                        tool_name: name.clone(),
                        result: result.clone(),
                        is_error,
                    });
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::ToolCall {
                            tool_name: name.clone(),
                            arguments: String::new(),
                            result: Some(result.clone()),
                            is_error,
                        },
                    });
                }

                if let Some(entry) = tool_log.iter_mut().rev().find(|e| {
                    e.get("name").and_then(|n| n.as_str()) == Some(&name)
                        && e.get("result").map(|r| r.is_null()).unwrap_or(false)
                }) {
                    entry["result"] = serde_json::Value::String(result);
                }
            }
            AgentMessage::GenerationStats(stats) => {
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::GenerationStats {
                            tokens_per_sec: stats.avg_completion_tok_per_sec,
                            input_tokens: stats.prompt_tokens,
                            output_tokens: stats.completion_tokens,
                        },
                    });
                }
            }
            AgentMessage::Done => {
                if let Some(tx) = event_tx {
                    let _ = tx.send(crate::orchestrator::OrchestratorEvent::AgentMessage {
                        prefix: prefix_owned.clone(),
                        message: crate::orchestrator::SubAgentMessage::Done,
                    });
                }
                return true;
            }
            _ => {}
        }

        false
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

    /// Get the current spec status as a JSON snapshot.
    /// Returns serialized SpecSheet with current step statuses and history.
    pub fn get_spec_status(&self, spec: &SpecSheet) -> Result<String> {
        serde_json::to_string_pretty(spec)
            .map_err(|e| anyhow::anyhow!("Failed to serialize spec: {}", e))
    }

    /// Convert a structured chat message field into a plain string for token estimation.
    /// Approximate token count for a block of text using the ~4 chars/token heuristic.
    /// Used for bulk text estimation (thinking buffer flushes, cancellation stats).
    /// Real-time streaming counts use 1-per-SSE-delta instead (more accurate).
    /// Final accurate counts come from the provider API response.
    fn estimate_tokens_heuristic(text: &str) -> usize {
        // ~4 chars per token, consistent with APPROX_CHARS_PER_TOKEN in constants.rs
        text.len() / 4
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

            let tools = {
                let tools_guard = self.tools.lock().await;
                tools_guard.clone()
            };
            let builder = RequestBuilder::new()
                .add_message(TextMessageRole::System, &system_prompt_content)
                .add_message(TextMessageRole::User, &user_message)
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
            let mut stream = self
                .backend
                .stream_chat_request(current_request_builder.clone())
                .await?;
            let mut accumulated_tool_calls: IndexMap<usize, ToolCallResponse> = IndexMap::new();
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
                                let token_count =
                                    Self::estimate_tokens_heuristic(&accumulated_content);
                                let _ = tx.send(AgentMessage::AgentResponse(
                                    accumulated_content.clone(),
                                    token_count,
                                ));
                            }

                            // Send partial GenerationStats even when cancelled.
                            // Use API-reported usage via get_latest_usage() when available
                            // (populated by stream_options.include_usage for OpenAI,
                            //  or message_start for Anthropic).
                            let elapsed_sec = stream_start_time.elapsed().as_secs_f32();
                            let time_to_first = first_token_time
                                .map(|t| t.duration_since(stream_start_time).as_secs_f32())
                                .unwrap_or(0.0);
                            let api_usage = self.backend.get_latest_usage().await;
                            let completion_tokens = total_generated_tokens;
                            let prompt_tokens =
                                api_usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0);
                            // Calculate tok/sec using ALL generated tokens (thinking + completion)
                            let avg_tok_per_sec = if elapsed_sec > 0.0 && total_generated_tokens > 0
                            {
                                total_generated_tokens as f32 / elapsed_sec
                            } else {
                                0.0
                            };
                            let stats = GenerationStats {
                                avg_completion_tok_per_sec: avg_tok_per_sec,
                                completion_tokens,
                                prompt_tokens,
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
                                } else if usage_stats.total_tokens > usage_stats.completion_tokens {
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
                                            ThinkingStartDecision::Detected {
                                                content_start_idx,
                                            } => {
                                                thinking_debug_log(
                                                    "Detected <think> start in HTTP chunk",
                                                );
                                                allow_thinking_start = false;
                                                in_thinking = true;
                                                process_as_thinking = true;
                                                let after_tag =
                                                    pending_prefix.split_off(content_start_idx);
                                                pending_prefix.clear();
                                                chunk_content = after_tag;
                                            }
                                            ThinkingStartDecision::NotThinking => {
                                                thinking_debug_log(
                                                    "Chunk does not start with <think>, treating as visible content",
                                                );
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
                                                let token_count =
                                                    Self::estimate_tokens_heuristic(final_thinking);
                                                // Track first token time and total tokens for stats
                                                if first_token_time.is_none() && token_count > 0 {
                                                    first_token_time =
                                                        Some(std::time::Instant::now());
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
                                                summarizer_guard
                                                    .add_thinking_chunk(final_thinking)
                                                    .await;
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
                                                    let token_count =
                                                        Self::estimate_tokens_heuristic(&outbound);
                                                    // Track first token time and total tokens for stats
                                                    if first_token_time.is_none() && token_count > 0
                                                    {
                                                        first_token_time =
                                                            Some(std::time::Instant::now());
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
                                                        if let Some((chunk_byte_end, _)) = remaining
                                                            .char_indices()
                                                            .nth(chunk_chars)
                                                        {
                                                            let chunk =
                                                                &remaining[..chunk_byte_end];
                                                            let token_count =
                                                                Self::estimate_tokens_heuristic(
                                                                    chunk,
                                                                );
                                                            // Track first token time and total tokens for stats
                                                            if first_token_time.is_none()
                                                                && token_count > 0
                                                            {
                                                                first_token_time =
                                                                    Some(std::time::Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(
                                                                AgentMessage::ThinkingContent(
                                                                    chunk.to_string(),
                                                                    token_count,
                                                                ),
                                                            );
                                                            thinking_debug_log(format!(
                                                                "Sent ThinkingContent (stream) tokens={} preview=\"{}\"",
                                                                token_count,
                                                                preview_thinking(chunk)
                                                            ));
                                                            remaining =
                                                                &remaining[chunk_byte_end..];
                                                        } else {
                                                            let token_count =
                                                                Self::estimate_tokens_heuristic(
                                                                    remaining,
                                                                );
                                                            // Track first token time and total tokens for stats
                                                            if first_token_time.is_none()
                                                                && token_count > 0
                                                            {
                                                                first_token_time =
                                                                    Some(std::time::Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(
                                                                AgentMessage::ThinkingContent(
                                                                    remaining.to_string(),
                                                                    token_count,
                                                                ),
                                                            );
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
                                                    summarizer_guard
                                                        .add_thinking_chunk(to_send)
                                                        .await;
                                                    for (summary, token_count, chunk_count) in
                                                        summarizer_guard.get_new_summaries()
                                                    {
                                                        let _ = tx.send(
                                                            AgentMessage::ThinkingSummary(format!(
                                                                "{}|{}|{}",
                                                                summary, token_count, chunk_count
                                                            )),
                                                        );
                                                        check_cancel!();
                                                    }

                                                    thinking_buffer =
                                                        thinking_buffer[byte_idx..].to_string();
                                                }
                                            }
                                        }
                                    } else {
                                        let chunk_has_visible = has_visible_text(&chunk_content);
                                        if !final_response_started && !chunk_has_visible {
                                            pending_agent_response_prefix.push_str(&chunk_content);
                                            check_cancel!();
                                            continue;
                                        }

                                        let mut outbound = String::new();
                                        if !pending_agent_response_prefix.is_empty() {
                                            outbound.push_str(&pending_agent_response_prefix);
                                            pending_agent_response_prefix.clear();
                                        }
                                        outbound.push_str(&chunk_content);
                                        accumulated_content.push_str(&outbound);
                                        // Each SSE delta is ~1 token; if prefix was prepended, estimate it separately
                                        let token_count = 1 + Self::estimate_tokens_heuristic(
                                            &outbound[..outbound
                                                .len()
                                                .saturating_sub(chunk_content.len())],
                                        );
                                        // Track first token time and total tokens for stats
                                        if first_token_time.is_none() && token_count > 0 {
                                            first_token_time = Some(std::time::Instant::now());
                                        }
                                        total_generated_tokens += token_count;
                                        let _ = tx.send(AgentMessage::AgentResponse(
                                            outbound.clone(),
                                            token_count,
                                        ));
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
                                        let token_count =
                                            Self::estimate_tokens_heuristic(&thinking_buffer);
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

                                    for tool_call in tool_calls {
                                        let previous_args = accumulated_tool_calls
                                            .get(&tool_call.index)
                                            .map(|existing| existing.function.arguments.clone());
                                        let is_new = previous_args.is_none();
                                        accumulated_tool_calls
                                            .insert(tool_call.index, tool_call.clone());
                                        let args_now = tool_call.function.arguments.clone();
                                        let args_changed = previous_args
                                            .as_ref()
                                            .map(|prev| prev != &args_now)
                                            .unwrap_or(true);

                                        if is_new || args_changed {
                                            let _ = tx.send(AgentMessage::ToolCallStarted(
                                                tool_call.function.name.clone(),
                                                args_now,
                                            ));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Response::Done(response) => {
                        if accumulated_tool_calls.is_empty()
                            && let Some(tool_calls) = response
                                .choices
                                .first()
                                .and_then(|choice| choice.message.tool_calls.clone())
                        {
                            for tool_call in tool_calls {
                                let args_now = tool_call.function.arguments.clone();
                                accumulated_tool_calls.insert(tool_call.index, tool_call.clone());
                                let _ = tx.send(AgentMessage::ToolCallStarted(
                                    tool_call.function.name.clone(),
                                    args_now,
                                ));
                            }
                        }

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
                let token_count = Self::estimate_tokens_heuristic(&pending_prefix);
                let _ = tx.send(AgentMessage::AgentResponse(
                    pending_prefix.clone(),
                    token_count,
                ));
            }

            // After stream ends, if still in thinking (no </think> found), flush residual
            if in_thinking && !thinking_buffer.is_empty() {
                let token_count = Self::estimate_tokens_heuristic(&thinking_buffer);
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
                for tool_call in accumulated_tool_calls.values().cloned() {
                    // Execute tool call and handle errors gracefully
                    let tool_result = match execute_tool_call(self, &tool_call, tx.clone()).await {
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
                            let error_yaml = serde_yaml::to_string(&json!({
                                "status": "Failure",
                                "message": e.to_string(),
                                "tool": tool_call.function.name.clone(),
                            }))
                            .unwrap_or_else(|_| {
                                "status: Failure\nmessage: Tool execution failed".to_string()
                            });

                            let _ = tx.send(AgentMessage::ToolCallCompleted(
                                tool_call.function.name.clone(),
                                error_yaml.clone(),
                            ));

                            error_yaml
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
        worktree: None,
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
                    // git status --short format: XY filename
                    // Skip the first 3 characters (status indicators) safely
                    let path: String = trimmed.chars().skip(3).collect::<String>();
                    let path = path.trim();
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
    task.metadata
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
    task.messages
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
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
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
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
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
        worktree: None,
    }
}

/// Decompose a single-line goal into multiple development steps.
/// Uses heuristics to identify features and create appropriate workflow.
fn decompose_goal_into_steps(goal: &str) -> Vec<SpecStep> {
    let goal_lower = goal.to_lowercase();
    let mut steps = Vec::new();

    // Helper to create a step
    fn make_step(
        index: usize,
        title: &str,
        instructions: &str,
        criteria: Vec<&str>,
        deps: Vec<usize>,
    ) -> SpecStep {
        SpecStep {
            index: index.to_string(),
            title: title.to_string(),
            instructions: instructions.to_string(),
            acceptance_criteria: criteria.iter().map(|s| s.to_string()).collect(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        }
    }

    // Step 1: Project setup
    steps.push(make_step(
        1,
        "Initialize project structure",
        &format!("Set up the project structure for: {}\n\nCreate necessary directories, initialize Cargo.toml with required dependencies, and set up the basic module structure.", goal),
        vec!["Project compiles with `cargo check`", "All dependencies declared in Cargo.toml"],
        vec![],
    ));

    // Step 2: Core data models
    steps.push(make_step(
        2,
        "Define core data models",
        "Create the core data structures and types needed for the application. Define structs, enums, and implement basic traits (Debug, Clone, Serialize/Deserialize as needed).",
        vec!["All core types defined", "Types implement required traits"],
        vec![1],
    ));

    let mut next_idx = 3;

    // Step 3: Storage/persistence layer (if mentioned)
    if goal_lower.contains("sqlite")
        || goal_lower.contains("database")
        || goal_lower.contains("storage")
        || goal_lower.contains("persist")
    {
        steps.push(make_step(
            next_idx,
            "Implement storage layer",
            "Create the database/storage layer. Set up SQLite connection, define schema, implement CRUD operations for all entities.",
            vec!["Database schema created", "CRUD operations work correctly", "Data persists across restarts"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    // Core business logic
    steps.push(make_step(
        next_idx,
        "Implement core functionality",
        &format!("Implement the main business logic for: {}\n\nThis includes all core operations and algorithms.", goal),
        vec!["Core features work as specified", "Error handling is proper"],
        vec![next_idx - 1],
    ));
    next_idx += 1;

    // CLI/UI layer (if CLI app)
    if goal_lower.contains("cli")
        || goal_lower.contains("command")
        || goal_lower.contains("terminal")
    {
        let colored = if goal_lower.contains("color") {
            " with colored output"
        } else {
            ""
        };
        steps.push(make_step(
            next_idx,
            "Build CLI interface",
            &format!("Create the command-line interface{}. Parse arguments, implement subcommands, format output nicely.", colored),
            vec!["CLI parses all required commands", "Help text is clear", "Output is well-formatted"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    // Tests (if mentioned)
    if goal_lower.contains("test") {
        steps.push(make_step(
            next_idx,
            "Write tests",
            "Write comprehensive tests for the application. Include unit tests for core logic and integration tests for the full workflow.",
            vec!["All tests pass", "Core functionality is covered", "Edge cases are tested"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    // Documentation (if mentioned)
    if goal_lower.contains("readme")
        || goal_lower.contains("documentation")
        || goal_lower.contains("doc")
    {
        steps.push(make_step(
            next_idx,
            "Create documentation",
            "Write README.md with installation instructions, usage examples, and feature documentation.",
            vec!["README is complete", "Examples are clear", "Installation steps work"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    // Final integration
    steps.push(make_step(
        next_idx,
        "Final integration and polish",
        "Ensure all components work together. Run full test suite, fix any issues, clean up code.",
        vec![
            "All tests pass",
            "cargo clippy has no warnings",
            "Application works end-to-end",
        ],
        vec![next_idx - 1],
    ));

    steps
}

/// Build a SpecSheet from a goal description string.
/// Each non-empty line in the goal becomes a separate step.
/// If the goal is a single line, it becomes a single-step spec.
fn build_spec_from_goal(goal: &str) -> Result<SpecSheet> {
    let created_at = Utc::now();
    let spec_id = format!("spec-{}", created_at.timestamp_millis());

    // Parse goal into steps - each non-empty line becomes a step
    let lines: Vec<&str> = goal
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    // Create steps from lines
    let steps: Vec<SpecStep> = if lines.is_empty() {
        // If no lines, use the entire goal as a single step
        vec![SpecStep {
            index: "1".to_string(),
            title: truncate_title(goal, 50),
            instructions: goal.to_string(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        }]
    } else if lines.len() == 1 {
        // Single line goal - decompose into standard software development steps
        decompose_goal_into_steps(goal)
    } else {
        // Multiple lines - first line is title/description, rest are steps
        lines
            .iter()
            .enumerate()
            .map(|(idx, line)| SpecStep {
                index: (idx + 1).to_string(),
                title: truncate_title(line, 50),
                instructions: line.to_string(),
                acceptance_criteria: Vec::new(),
                required_tools: Vec::new(),
                constraints: Vec::new(),
                dependencies: if idx > 0 {
                    vec![idx.to_string()] // Each step depends on the previous
                } else {
                    Vec::new()
                },
                is_parallel: false,
                requires_verification: true,
                max_parallelism: None,
                status: StepStatus::Pending,
                sub_spec: None,
                completed_at: None,
            })
            .collect()
    };

    let title = if lines.is_empty() {
        truncate_title(goal, 80)
    } else {
        truncate_title(lines[0], 80)
    };

    let spec = SpecSheet {
        id: spec_id,
        title,
        description: goal.to_string(),
        steps,
        created_by: "cli".to_string(),
        created_at,
        metadata: json!({}),
    };

    spec.validate().map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(spec)
}

/// Truncate a string to a maximum number of characters, adding "..." if truncated.
/// This is UTF-8 safe - it counts characters, not bytes.
fn truncate_title(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
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
        metadata.extra.insert("stepIndex".to_string(), json!("1"));
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

#[cfg(test)]
mod exec_command_tests {
    use super::*;
    use mistralrs::CalledFunction;
    use mistralrs::ToolCallType;
    use std::sync::{Mutex as StdMutex, OnceLock};

    fn exec_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    async fn reset_global_shell_state() {
        ensure_global_state_initialized().await;
        if let Some(state) = GLOBAL_STATE.get() {
            if let Some(existing) = state.shell_session_id.lock().await.clone() {
                let _ = state.manager.terminate_session(existing).await;
            }
            *state.shell_session_id.lock().await = None;
            *state.session_has_background_process.lock().await = false;
        }
    }

    fn build_test_agent(cwd: PathBuf) -> Agent {
        let safety = safety_config::SafetyConfig::from_mode(safety_config::SafetyMode::Yolo);
        Agent::new_with_backend(
            BackendConfig::None,
            String::new(),
            vec![],
            safety,
            "test".to_string(),
        )
        .with_working_directory(cwd)
    }

    fn make_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-core-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCallResponse {
        ToolCallResponse {
            index: 0,
            id: "call-1".to_string(),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn exec_command_reports_success_for_foreground_command() {
        let _guard = exec_test_lock();
        let temp = make_test_dir("success");
        set_workspace_root_override(&temp);
        reset_global_shell_state().await;
        let agent = build_test_agent(temp.clone());
        let (tx, _rx) = mpsc::unbounded_channel();

        let result = execute_tool_call(
            &agent,
            &tool_call("exec_command", json!({ "command": "printf 'hello'" })),
            tx,
        )
        .await
        .expect("exec command succeeds");

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
        assert_eq!(parsed["status"].as_str(), Some("Success"));
        assert_eq!(parsed["cmd_out"].as_str(), Some("hello"));
    }

    #[tokio::test]
    async fn exec_command_uses_agent_working_directory() {
        let _guard = exec_test_lock();
        let temp = make_test_dir("working-directory");
        let nested = temp.join("nested");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        set_workspace_root_override(&temp);
        reset_global_shell_state().await;
        let agent = build_test_agent(nested.clone());
        let (tx, _rx) = mpsc::unbounded_channel();

        let result = execute_tool_call(
            &agent,
            &tool_call("exec_command", json!({ "command": "pwd" })),
            tx,
        )
        .await
        .expect("exec command succeeds");

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
        assert_eq!(parsed["status"].as_str(), Some("Success"));
        assert_eq!(
            parsed["cmd_out"].as_str(),
            Some(nested.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn exec_command_reports_background_metadata() {
        let _guard = exec_test_lock();
        let temp = make_test_dir("background");
        set_workspace_root_override(&temp);
        reset_global_shell_state().await;
        let agent = build_test_agent(temp.clone());
        let (tx, _rx) = mpsc::unbounded_channel();

        let result = execute_tool_call(
            &agent,
            &tool_call(
                "exec_command",
                json!({ "command": "sleep 1", "is_background": true }),
            ),
            tx,
        )
        .await
        .expect("background command result returned");

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).expect("yaml result");
        assert_eq!(parsed["status"].as_str(), Some("Background"));
        assert!(parsed["session_id"].as_str().is_some());
        assert!(parsed["log_file"].as_str().is_some());

        reset_global_shell_state().await;
    }

    #[tokio::test]
    async fn update_safety_config_preserves_context_and_replaces_system_prompt() {
        let _guard = exec_test_lock();
        let temp = make_test_dir("mode-change");
        set_workspace_root_override(&temp);
        reset_global_shell_state().await;
        let agent = build_test_agent(temp.clone());

        agent
            .restore_conversation(
                r#"[
                    {"role":"system","content":"**READ-ONLY MODE ACTIVE**"},
                    {"role":"user","content":"previous message"},
                    {"role":"assistant","content":"previous reply"}
                ]"#,
            )
            .await
            .expect("restore conversation");

        agent
            .update_safety_config(safety_config::SafetyConfig::from_mode(
                safety_config::SafetyMode::Regular,
            ))
            .await
            .expect("update safety config");

        let exported = agent
            .export_conversation()
            .await
            .expect("conversation preserved");
        let messages: serde_json::Value = serde_json::from_str(&exported).expect("json messages");
        let entries = messages.as_array().expect("message array");

        assert_eq!(message_left_str(&entries[0], "role"), Some("system"));
        assert!(
            !message_left_str(&entries[0], "content")
                .unwrap_or_default()
                .contains("READ-ONLY MODE ACTIVE")
        );
        assert_eq!(
            message_left_str(&entries[1], "content"),
            Some("previous message")
        );
        assert_eq!(
            message_left_str(&entries[2], "content"),
            Some("previous reply")
        );
    }

    #[tokio::test]
    async fn inject_system_reminder_preserves_context_and_appends_reminder() {
        let _guard = exec_test_lock();
        let temp = make_test_dir("reminder");
        set_workspace_root_override(&temp);
        reset_global_shell_state().await;
        let agent = build_test_agent(temp.clone());

        agent
            .restore_conversation(
                r#"[
                    {"role":"system","content":"base system"},
                    {"role":"user","content":"keep this context"}
                ]"#,
            )
            .await
            .expect("restore conversation");

        let reminder = "<system-reminder>\nYour operational mode has changed from plan to build.\nYou are no longer in read-only mode.\nYou are permitted to make file changes, run shell commands, and utilize your arsenal of tools as needed.\n</system-reminder>";
        agent
            .inject_system_reminder(reminder)
            .await
            .expect("inject reminder");

        let exported = agent
            .export_conversation()
            .await
            .expect("conversation preserved");
        let messages: serde_json::Value = serde_json::from_str(&exported).expect("json messages");
        let entries = messages.as_array().expect("message array");

        assert_eq!(
            message_left_str(&entries[1], "content"),
            Some("keep this context")
        );
        assert_eq!(message_left_str(&entries[2], "role"), Some("system"));
        assert_eq!(message_left_str(&entries[2], "content"), Some(reminder));
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
