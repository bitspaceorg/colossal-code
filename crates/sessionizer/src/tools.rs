use crate::error::ColossalErr;
use crate::manager::SessionManager;
use crate::protocol::SandboxPolicy;
use crate::sandboxing::{SandboxCommand, SandboxManager};
use crate::shell::Shell;
use crate::types::ExecCommandParams;
use std::collections::HashSet;
use std::path::PathBuf;

pub async fn exec_command(
    command: Vec<String>,
    is_background: bool,
    _is_session: bool,
    _is_streaming: bool,
    manager: &SessionManager,
    shell: &Shell,
    sandbox_policy: &SandboxPolicy,
    _approved_commands: &HashSet<Vec<String>>,
) -> Result<serde_json::Value, ColossalErr> {
    let cwd = std::env::current_dir().map_err(|e| ColossalErr::Io(e))?;
    let default_timeout_ms = std::env::var("NITE_EXEC_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(120_000);

    // Use the PTY-based command execution from the manager
    let params = ExecCommandParams {
        command,
        shell: shell.clone(),
        cwd,
        env: Default::default(),
        timeout_ms: if is_background {
            None
        } else {
            Some(default_timeout_ms)
        },
        max_output_tokens: 1000,
        sandbox_policy: sandbox_policy.clone(),
        is_background,
        ask_for_approval: None,
    };

    let result = manager.handle_exec_command_request(params).await?;

    Ok(result.to_json_value())
}

/// Read output from a background process
pub async fn read_output(
    session_id: String,
    manager: &SessionManager,
) -> Result<serde_json::Value, ColossalErr> {
    let session_id = crate::types::SessionId::new(session_id);
    let output = manager.read_background_output(session_id).await?;

    Ok(serde_json::json!({
        "output": output
    }))
}

/// Execute the tools binary with sandboxing applied
pub async fn execute_tools_with_sandbox(
    args: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
) -> Result<std::process::Output, ColossalErr> {
    let tools_path = get_tools_path()?;
    let mut env = std::collections::HashMap::new();
    env.insert(
        "NITE_WORKSPACE_ROOT".to_string(),
        cwd.to_string_lossy().to_string(),
    );
    let request = SandboxManager::new().prepare_spawn(
        SandboxCommand {
            program: tools_path,
            args,
            cwd: cwd.clone(),
            env,
        },
        sandbox_policy,
    )?;

    let mut cmd = tokio::process::Command::new(&request.program);
    cmd.args(&request.args)
        .current_dir(&request.cwd)
        .envs(&request.env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped());
    cmd.output().await.map_err(ColossalErr::Io)
}

/// Get the path to the tools binary
pub fn resolve_tools_binary_path() -> Result<PathBuf, ColossalErr> {
    get_tools_path()
}

fn get_tools_path() -> Result<PathBuf, ColossalErr> {
    // Look for the tools binary in the same directory as the current executable
    let exe_path = std::env::current_exe().map_err(|e| ColossalErr::Io(e))?;
    let exe_dir = exe_path.parent().ok_or_else(|| {
        ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Executable directory not found",
        ))
    })?;

    let tools_path = exe_dir.join("tools");

    // If the tools binary doesn't exist in the same directory, look in common locations
    if !tools_path.exists() {
        // The tools binary is in crates/sessionizer/target/{debug,release}/tools
        // We need to find it relative to the main executable's target directory

        // Try to find the tools binary in crates/sessionizer/target
        let mut search_path = exe_dir.to_path_buf();

        // Navigate up to find the workspace target directory
        // Executable might be in target/debug/ or target/debug/examples/
        while search_path.parent().is_some() {
            let parent = search_path.parent().unwrap();

            // Check if this is the target directory (has debug/release subdirs)
            if parent.file_name().and_then(|n| n.to_str()) == Some("target") {
                // Found target directory, now look for tools binary in various locations
                let workspace_root = parent.parent().unwrap_or(parent);

                // Try workspace-level target/release/tools first (most common for cargo build --release)
                let workspace_release = workspace_root.join("target").join("release").join("tools");

                if workspace_release.exists() {
                    return Ok(workspace_release);
                }

                // Try workspace-level target/debug/tools
                let workspace_debug = workspace_root.join("target").join("debug").join("tools");

                if workspace_debug.exists() {
                    return Ok(workspace_debug);
                }

                // Try crates/sessionizer/target/release/tools
                let sessionizer_release = workspace_root
                    .join("crates")
                    .join("sessionizer")
                    .join("target")
                    .join("release")
                    .join("tools");

                if sessionizer_release.exists() {
                    return Ok(sessionizer_release);
                }

                // Try crates/sessionizer/target/debug/tools
                let sessionizer_debug = workspace_root
                    .join("crates")
                    .join("sessionizer")
                    .join("target")
                    .join("debug")
                    .join("tools");

                if sessionizer_debug.exists() {
                    return Ok(sessionizer_debug);
                }

                // Try crates/agent_core/target/release/tools
                let agent_core_release = workspace_root
                    .join("crates")
                    .join("agent_core")
                    .join("target")
                    .join("release")
                    .join("tools");

                if agent_core_release.exists() {
                    return Ok(agent_core_release);
                }

                // Try crates/agent_core/target/debug/tools
                let agent_core_debug = workspace_root
                    .join("crates")
                    .join("agent_core")
                    .join("target")
                    .join("debug")
                    .join("tools");

                if agent_core_debug.exists() {
                    return Ok(agent_core_debug);
                }

                break;
            }

            search_path = parent.to_path_buf();
        }

        // Fallback: return the default path and let it fail with a clear error
        Err(ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Tools binary not found. Searched in {} and workspace target/{{debug,release}}/tools, crates/sessionizer/target/{{debug,release}}/tools, crates/agent_core/target/{{debug,release}}/tools",
                tools_path.display()
            ),
        )))
    } else {
        Ok(tools_path)
    }
}
