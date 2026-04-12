use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::session::SharedSessionState;
use colossal_linux_sandbox::shell;
use colossal_linux_sandbox::types::ExitStatus;
use colossal_linux_sandbox::types::StreamEvent;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

fn shell_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

async fn create_shell_session(
    cwd: &Path,
    shell_path: String,
    sandbox_policy: SandboxPolicy,
) -> Result<(SessionManager, colossal_linux_sandbox::types::SessionId), Box<dyn std::error::Error>>
{
    let manager = SessionManager::default();
    let shared_state = Arc::new(SharedSessionState::new(cwd.to_path_buf()));
    let session_id = manager
        .create_persistent_shell_session(
            shell_path,
            false,
            sandbox_policy,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    Ok((manager, session_id))
}

fn deterministic_shell_path() -> String {
    if Path::new("/bin/sh").exists() {
        "/bin/sh".to_string()
    } else {
        "/bin/bash".to_string()
    }
}

fn nushell_path() -> Option<String> {
    let output = std::process::Command::new("sh")
        .arg("-lc")
        .arg("command -v nu")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn workspace_write_policy(cwd: &Path) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![colossal_linux_sandbox::protocol::WritableRoot {
            root: cwd.to_path_buf(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: colossal_linux_sandbox::protocol::NetworkAccess::Restricted,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    }
}

#[path = "persistent_shell_session/basic.rs"]
mod basic;

#[path = "persistent_shell_session/managed_nu_rotation.rs"]
mod managed_nu_rotation;

#[path = "persistent_shell_session/managed_nu_fork_eval.rs"]
mod managed_nu_fork_eval;

#[path = "persistent_shell_session/managed_nu_env.rs"]
mod managed_nu_env;

#[path = "persistent_shell_session/managed_nu_cwd.rs"]
mod managed_nu_cwd;

#[path = "persistent_shell_session/managed_nu_variables.rs"]
mod managed_nu_variables;

#[path = "persistent_shell_session/managed_nu_defs.rs"]
mod managed_nu_defs;

#[path = "persistent_shell_session/managed_nu_aliases.rs"]
mod managed_nu_aliases;

#[path = "persistent_shell_session/managed_nu_boundaries.rs"]
mod managed_nu_boundaries;

#[path = "persistent_shell_session/managed_nu_config.rs"]
mod managed_nu_config;
