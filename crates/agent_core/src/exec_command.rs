use crate::{Agent, shell_session, tool_dispatch};
use anyhow::Result;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::tools::execute_tools_with_sandbox;
use colossal_linux_sandbox::types::ExitStatus;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::sync::mpsc;

pub async fn execute_tool_binary(
    args: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
) -> Result<String> {
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

#[derive(Serialize)]
pub struct ExecCommandResult {
    pub command: String,
    pub status: String,
    pub cmd_out: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub fn exec_command_output_to_yaml(
    command: &str,
    result: colossal_linux_sandbox::types::ExecCommandOutput,
) -> Result<String> {
    match result.exit_status {
        ExitStatus::Ongoing(session_id) => Ok(serde_yaml::to_string(&json!({
            "command": command,
            "status": "Background",
            "session_id": session_id.as_str(),
            "log_file": result.log_file.as_ref().map(|path| path.display().to_string()),
            "message": result.aggregated_output,
        }))?),
        exit_status => {
            let is_success = matches!(exit_status, ExitStatus::Completed { code } if code == 0);
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
                    Some(format!("{:?}", exit_status))
                },
            };
            Ok(serde_yaml::to_string(&exec_result)?)
        }
    }
}

pub async fn execute_tool_call(
    agent: &Agent,
    tool_call: &mistralrs::ToolCallResponse,
    tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    shell_session::ensure_global_state_initialized().await;

    if let Some(state) = shell_session::global_state() {
        let safety_cfg = agent.safety_config.lock().await.clone();
        let workspace_path = agent.execution_cwd().await?;
        let refreshed_policy =
            crate::sandbox_policy_from_config_with_workspace(&safety_cfg, workspace_path);
        let mut policy_guard = state.pending_sandbox_policy.lock().await;
        *policy_guard = refreshed_policy;
    }

    let name = &tool_call.function.name;
    let arguments: Value = serde_json::from_str(&tool_call.function.arguments)?;

    let result = if name != "exec_command"
        && let Some(result) =
            tool_dispatch::execute_non_exec_tool_call(agent, name, &arguments).await?
    {
        Ok(result)
    } else {
        match name.as_str() {
            "exec_command" => execute_exec_command(agent, &arguments, tx).await,
            _ => Ok(format!("Tool '{}' executed (not fully implemented)", name)),
        }
    };

    if result.is_ok() {
        agent.checkpoint_execution_after_tool().await?;
    }
    result
}

async fn execute_exec_command(
    agent: &Agent,
    arguments: &Value,
    tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    let state = shell_session::global_state().unwrap();
    let command = arguments["command"].as_str().unwrap_or("");
    let is_background = arguments
        .get("is_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let replay_state = arguments
        .get("replay_state")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let timeout_ms = arguments
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(600_000);

    let requires_approval = { agent.safety_config.lock().await.ask_permission };
    let safety_mode = if requires_approval {
        colossal_linux_sandbox::safety::AskForApproval::OnRequest
    } else {
        colossal_linux_sandbox::safety::AskForApproval::Never
    };

    let current_approval = Some(safety_mode);
    let uses_managed_nu_foreground = matches!(
        state.shell.kind(),
        colossal_linux_sandbox::shell::ShellKind::ManagedNu
    ) && !is_background;

    if replay_state && is_background {
        return Ok(serde_yaml::to_string(&json!({
            "status": "Failure",
            "command": command,
            "message": "Background commands cannot use replay_state because their shell state cannot be replayed safely"
        }))?);
    }

    if !replay_state && uses_managed_nu_foreground {
        execute_managed_nu_foreground(state, agent, command, timeout_ms, current_approval, tx).await
    } else if !replay_state && is_background {
        // Background commands are fire-and-forget: isolated execution is fine
        execute_isolated_exec(
            state,
            agent,
            command,
            is_background,
            timeout_ms,
            current_approval,
            tx,
        )
        .await
    } else {
        // All foreground non-ManagedNu commands (bash etc.) use the persistent session,
        // same as replay_state=true — state is maintained across calls.
        // replay_state=false means the command is not stored in the continuity journal.
        execute_replay_state(
            state,
            agent,
            command,
            is_background,
            timeout_ms,
            replay_state,
            current_approval,
            tx,
        )
        .await
    }
}

async fn execute_managed_nu_foreground(
    state: &shell_session::GlobalState,
    agent: &Agent,
    command: &str,
    timeout_ms: u64,
    mut current_approval: Option<colossal_linux_sandbox::safety::AskForApproval>,
    tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    let (manager, session_id) = shell_session::get_or_create_shell_session(
        Some(agent.execution_cwd().await?),
        agent.execution_env_overrides().await?,
    )
    .await?;

    loop {
        match manager
            .fork_eval_in_managed_nu_session(
                session_id.clone(),
                command.to_string(),
                Some(timeout_ms),
                current_approval,
            )
            .await
        {
            Ok(Some(result)) => return exec_command_output_to_yaml(command, result),
            Ok(None) => {
                break;
            }
            Err(e) => {
                if let colossal_linux_sandbox::error::ColossalErr::Sandbox(
                    colossal_linux_sandbox::error::SandboxErr::Denied(_, reason, _),
                ) = &e
                {
                    if reason == "User approval required" {
                        let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
                        {
                            let mut guard = state.pending_approval.lock().await;
                            *guard = Some(approval_tx);
                        }
                        let request_msg = format!("Allow command: {}", command);
                        let _ = tx.send(crate::AgentMessage::RequestApproval(request_msg));
                        match approval_rx.await {
                            Ok(true) => {
                                current_approval =
                                    Some(colossal_linux_sandbox::safety::AskForApproval::Never);
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

    execute_generic_exec(state, agent, command, false, 600_000, current_approval, tx).await
}

async fn execute_isolated_exec(
    state: &shell_session::GlobalState,
    agent: &Agent,
    command: &str,
    is_background: bool,
    timeout_ms: u64,
    mut current_approval: Option<colossal_linux_sandbox::safety::AskForApproval>,
    tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    loop {
        match shell_session::run_isolated_exec_command(
            state,
            command,
            is_background,
            timeout_ms,
            agent.execution_cwd().await?,
            agent.execution_env_overrides().await?,
            current_approval,
        )
        .await
        {
            Ok(result) => return exec_command_output_to_yaml(command, result),
            Err(e) => {
                if let colossal_linux_sandbox::error::ColossalErr::Sandbox(
                    colossal_linux_sandbox::error::SandboxErr::Denied(_, reason, _),
                ) = &e
                {
                    if reason == "User approval required" {
                        let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
                        {
                            let mut guard = state.pending_approval.lock().await;
                            *guard = Some(approval_tx);
                        }

                        let request_msg = if is_background {
                            format!("Allow background command: {}", command)
                        } else {
                            format!("Allow command: {}", command)
                        };
                        let _ = tx.send(crate::AgentMessage::RequestApproval(request_msg));

                        match approval_rx.await {
                            Ok(true) => {
                                current_approval =
                                    Some(colossal_linux_sandbox::safety::AskForApproval::Never);
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

async fn execute_replay_state(
    state: &shell_session::GlobalState,
    agent: &Agent,
    command: &str,
    _is_background: bool,
    timeout_ms: u64,
    replay_state: bool,
    mut current_approval: Option<colossal_linux_sandbox::safety::AskForApproval>,
    tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    let mut retried_session = false;

    loop {
        let (manager, session_id) = shell_session::get_or_create_shell_session(
            Some(agent.execution_cwd().await?),
            agent.execution_env_overrides().await?,
        )
        .await?;

        match manager
            .exec_command_in_shell_session(
                session_id.clone(),
                command.to_string(),
                Some(timeout_ms),
                1000,
                current_approval,
            )
            .await
        {
            Ok(result) => {
                if let Err(err) = shell_session::sync_continuity_state_from_session(
                    state,
                    session_id.clone(),
                    replay_state.then_some(command),
                )
                .await
                {
                    if matches!(result.exit_status, ExitStatus::Timeout | ExitStatus::Killed) {
                        let _ = state.manager.terminate_session(session_id.clone()).await;
                        let mut session_id_lock = state.shell_session_id.lock().await;
                        if session_id_lock.as_ref() == Some(&session_id) {
                            *session_id_lock = None;
                        }
                        return exec_command_output_to_yaml(command, result);
                    }
                    return Err(err);
                }
                return exec_command_output_to_yaml(command, result);
            }
            Err(e) => {
                if matches!(
                    &e,
                    colossal_linux_sandbox::error::ColossalErr::Io(err)
                        if err.to_string().contains("failed to send input to shell")
                ) && !retried_session
                {
                    retried_session = true;
                    if let Err(err) = state.manager.terminate_session(session_id.clone()).await {
                        tracing::warn!("Failed to terminate broken shell session: {}", err);
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
                        let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
                        {
                            let mut guard = state.pending_approval.lock().await;
                            *guard = Some(approval_tx);
                        }

                        let request_msg = format!("Allow command: {}", command);
                        let _ = tx.send(crate::AgentMessage::RequestApproval(request_msg));

                        match approval_rx.await {
                            Ok(true) => {
                                current_approval =
                                    Some(colossal_linux_sandbox::safety::AskForApproval::Never);
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

async fn execute_generic_exec(
    state: &shell_session::GlobalState,
    agent: &Agent,
    command: &str,
    _is_background: bool,
    timeout_ms: u64,
    current_approval: Option<colossal_linux_sandbox::safety::AskForApproval>,
    _tx: mpsc::UnboundedSender<crate::AgentMessage>,
) -> Result<String> {
    let (manager, session_id) = shell_session::get_or_create_shell_session(
        Some(agent.execution_cwd().await?),
        agent.execution_env_overrides().await?,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            command.to_string(),
            Some(timeout_ms),
            1000,
            current_approval,
        )
        .await?;

    shell_session::sync_continuity_state_from_session(state, session_id, None).await?;

    exec_command_output_to_yaml(command, result)
}
