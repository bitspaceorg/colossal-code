use crate::error::ColossalErr;
use crate::manager::SessionManager;
use crate::protocol::SandboxPolicy;
use crate::sandboxing::{SandboxCommand, SandboxManager};
use crate::shell::Shell;
use crate::types::ExecCommandParams;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

    // On Linux without bubblewrap, apply landlock/seccomp in the forked child
    #[cfg(target_os = "linux")]
    {
        if request.sandbox == crate::sandboxing::SandboxType::LinuxLandlock {
            if let Some(policy) = request.sandbox_policy {
                let cwd_for_sandbox = request.cwd.clone();
                unsafe {
                    cmd.pre_exec(move || {
                        crate::landlock::apply_sandbox_policy_to_current_thread(
                            &policy,
                            &cwd_for_sandbox,
                        )
                        .map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
                        })
                    });
                }
            }
        }
    }

    cmd.output().await.map_err(ColossalErr::Io)
}

/// Get the path to the tools binary
pub fn resolve_tools_binary_path() -> Result<PathBuf, ColossalErr> {
    get_tools_path()
}

fn get_tools_path() -> Result<PathBuf, ColossalErr> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = candidate_tools_paths(&cwd);

    for path in &candidates {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    if let Some(path) = find_tools_on_path() {
        return Ok(path);
    }

    let searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(ColossalErr::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "Tools binary not found. Searched: {searched}. Also checked PATH for 'tools' and env override NITE_TOOLS_BINARY"
        ),
    )))
}

fn candidate_tools_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = env_tools_override(cwd) {
        push_candidate(&mut candidates, &mut seen, path);
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        push_candidate(&mut candidates, &mut seen, exe_dir.join("tools"));

        if let Some(workspace_root) = workspace_root_from_exe_dir(exe_dir) {
            push_repo_workspace_candidates(&mut candidates, &mut seen, &workspace_root);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    push_workspace_target_candidates(&mut candidates, &mut seen, &manifest_dir);
    if let Some(repo_root) = manifest_dir.parent().and_then(|path| path.parent()) {
        push_repo_workspace_candidates(&mut candidates, &mut seen, repo_root);
    }

    push_repo_workspace_candidates(&mut candidates, &mut seen, cwd);

    candidates
}

fn env_tools_override(cwd: &Path) -> Option<PathBuf> {
    let raw = std::env::var_os("NITE_TOOLS_BINARY")?;
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(cwd.join(path))
    }
}

fn workspace_root_from_exe_dir(exe_dir: &Path) -> Option<PathBuf> {
    let mut search_path = exe_dir.to_path_buf();

    while let Some(parent) = search_path.parent() {
        if parent.file_name().and_then(|name| name.to_str()) == Some("target") {
            return Some(parent.parent().unwrap_or(parent).to_path_buf());
        }
        search_path = parent.to_path_buf();
    }

    None
}

fn push_repo_workspace_candidates(
    candidates: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    workspace_root: &Path,
) {
    push_workspace_target_candidates(candidates, seen, workspace_root);
    push_workspace_target_candidates(
        candidates,
        seen,
        &workspace_root.join("crates").join("sessionizer"),
    );
    push_workspace_target_candidates(
        candidates,
        seen,
        &workspace_root.join("crates").join("agent_core"),
    );
}

fn push_workspace_target_candidates(
    candidates: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    root: &Path,
) {
    push_candidate(
        candidates,
        seen,
        root.join("target").join("release").join("tools"),
    );
    push_candidate(
        candidates,
        seen,
        root.join("target").join("debug").join("tools"),
    );
}

fn push_candidate(candidates: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        candidates.push(path);
    }
}

fn find_tools_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("tools"))
        .find(|candidate| candidate.exists())
}

#[cfg(test)]
mod tests {
    use super::{
        push_repo_workspace_candidates, push_workspace_target_candidates,
        workspace_root_from_exe_dir,
    };
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    #[test]
    fn derives_workspace_root_from_target_executable_directory() {
        let exe_dir = Path::new("/tmp/cocode/target/release");
        assert_eq!(
            workspace_root_from_exe_dir(exe_dir),
            Some(PathBuf::from("/tmp/cocode"))
        );
    }

    #[test]
    fn includes_sessionizer_workspace_targets_in_repo_candidates() {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        push_repo_workspace_candidates(&mut candidates, &mut seen, Path::new("/repo"));

        assert!(candidates.contains(&PathBuf::from(
            "/repo/crates/sessionizer/target/release/tools"
        )));
        assert!(candidates.contains(&PathBuf::from("/repo/target/debug/tools")));
    }

    #[test]
    fn includes_release_and_debug_targets_for_workspace() {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        push_workspace_target_candidates(
            &mut candidates,
            &mut seen,
            Path::new("/repo/crates/sessionizer"),
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/repo/crates/sessionizer/target/release/tools"),
                PathBuf::from("/repo/crates/sessionizer/target/debug/tools"),
            ]
        );
    }
}
