use crate::error::ColossalErr;
use crate::manager::SessionManager;
use crate::protocol::SandboxPolicy;
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

    // Use the PTY-based command execution from the manager
    let params = ExecCommandParams {
        command,
        shell: shell.clone(),
        cwd,
        env: Default::default(),
        timeout_ms: if is_background { None } else { Some(10000) },
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
    // Get the path to the tools binary
    let tools_path = get_tools_path()?;

    // Execute the tools binary with the provided arguments using the sandboxing mechanism
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};

        // Clone the sandbox policy for the blocking task
        let sandbox_policy = sandbox_policy.clone();
        let cwd_clone = cwd.clone();

        // Use spawn_blocking to run the sandboxed command in a blocking thread
        // This ensures the sandbox is applied to the thread that spawns the child process
        let output = tokio::task::spawn_blocking(move || {
            // Apply the sandbox policy to this thread before spawning the process
            crate::landlock::apply_sandbox_policy_to_current_thread(&sandbox_policy, &cwd_clone)?;

            let mut cmd = Command::new(tools_path);
            cmd.args(args)
                .current_dir(cwd_clone)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::piped());

            cmd.output()
                .map_err(|e| ColossalErr::Io(e))
        })
        .await
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

        Ok(output)
    }
    
    #[cfg(target_os = "macos")]
    {
        // For macOS, we need to run the tools binary through the sandbox-exec mechanism
        let mut seatbelt_args = vec![
            "-p".to_string(),
            crate::seatbelt::MACOS_SEATBELT_BASE_POLICY.to_string(),
        ];
        
        // Add the appropriate rules based on the sandbox_policy
        seatbelt_args.extend_from_slice(&create_seatbelt_args_for_policy(sandbox_policy, &cwd));
        seatbelt_args.push("--".to_string());
        seatbelt_args.push(tools_path.to_string_lossy().to_string());
        seatbelt_args.extend(args);
        
        let output = Command::new(crate::seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(seatbelt_args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| ColossalErr::Io(e))?;
        
        Ok(output)
    }
    
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // For other platforms, execute directly without additional sandboxing
        let mut cmd = Command::new(tools_path);
        cmd.args(args)
            .current_dir(cwd);
        
        let output = cmd.output().await
            .map_err(|e| ColossalErr::Io(e))?;
        
        Ok(output)
    }
}

/// Create seatbelt arguments for a given sandbox policy (macOS only)
#[cfg(target_os = "macos")]
fn create_seatbelt_args_for_policy(sandbox_policy: &SandboxPolicy, cwd: &Path) -> Vec<String> {
    // This would use similar logic to the create_seatbelt_command_args function in seatbelt.rs
    // but adapted to generate the proper arguments for running a program with sandbox-exec
    
    // For now, we'll return an empty vector, but in a full implementation, 
    // this would generate the proper seatbelt arguments based on the policy
    let mut args = Vec::new();
    
    match sandbox_policy {
        crate::protocol::SandboxPolicy::WorkspaceWrite { 
            writable_roots, 
            network_access, 
            exclude_tmpdir_env_var, 
            exclude_slash_tmp 
        } => {
            // Add parameters for writable roots
            for (index, wr) in writable_roots.iter().enumerate() {
                let canonical_root = wr.root.canonicalize().unwrap_or_else(|_| wr.root.clone());
                let root_param = format!("WRITABLE_ROOT_{}", index);
                args.push(format!("-D{}={}", root_param, canonical_root.to_string_lossy()));
            }
            
            // Handle /tmp access
            if !exclude_slash_tmp {
                args.push(format!("-DWRITABLE_ROOT_TMP={}", std::path::Path::new("/tmp").to_string_lossy()));
            }
            
            // Handle TMPDIR access
            if !exclude_tmpdir_env_var {
                if let Ok(tmpdir) = std::env::var("TMPDIR") {
                    args.push(format!("-DWRITABLE_ROOT_TMPEXTRA={}", tmpdir));
                }
            }
            
            // Handle working directory
            let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
            args.push(format!("-DWRITABLE_ROOT_CWD={}", canonical_cwd.to_string_lossy()));
        },
        crate::protocol::SandboxPolicy::DangerFullAccess => {
            // For DangerFullAccess, we might not need specific arguments, 
            // or we could add arguments to allow everything
        }
    }
    
    args
}

/// Get the path to the tools binary
fn get_tools_path() -> Result<PathBuf, ColossalErr> {
    // Look for the tools binary in the same directory as the current executable
    let exe_path = std::env::current_exe()
        .map_err(|e| ColossalErr::Io(e))?;
    let exe_dir = exe_path.parent()
        .ok_or_else(|| ColossalErr::Io(
            std::io::Error::new(std::io::ErrorKind::NotFound, "Executable directory not found")
        ))?;

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
                let workspace_release = workspace_root
                    .join("target")
                    .join("release")
                    .join("tools");

                if workspace_release.exists() {
                    return Ok(workspace_release);
                }

                // Try workspace-level target/debug/tools
                let workspace_debug = workspace_root
                    .join("target")
                    .join("debug")
                    .join("tools");

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
            format!("Tools binary not found. Searched in {} and workspace target/{{debug,release}}/tools, crates/sessionizer/target/{{debug,release}}/tools, crates/agent_core/target/{{debug,release}}/tools", tools_path.display())
        )))
    } else {
        Ok(tools_path)
    }
}
