use anyhow::Result;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use colossal_linux_sandbox::types::SessionId;
use once_cell::sync::OnceCell;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct GlobalState {
    pub(crate) manager: Arc<colossal_linux_sandbox::manager::SessionManager>,
    pub(crate) shell_session_id: tokio::sync::Mutex<Option<SessionId>>,
    pub(crate) shell: colossal_linux_sandbox::shell::Shell,
    pub(crate) continuity_state:
        tokio::sync::Mutex<colossal_linux_sandbox::manager::PersistentSessionState>,
    pub(crate) pending_sandbox_policy: tokio::sync::Mutex<SandboxPolicy>,
    pub(crate) effective_sandbox_policy: tokio::sync::Mutex<SandboxPolicy>,
    pub(crate) session_has_background_process: tokio::sync::Mutex<bool>,
    pub(crate) pending_approval: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
}

static GLOBAL_STATE: OnceCell<GlobalState> = OnceCell::new();

fn should_seed_cwd(
    current_cwd: &std::path::Path,
    seed_cwd: &std::path::Path,
    env_overrides: &std::collections::HashMap<String, String>,
) -> bool {
    let workspace_root = env_overrides
        .get("NITE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(crate::resolve_workspace_root);
    current_cwd == crate::resolve_workspace_root()
        || !current_cwd.starts_with(&workspace_root)
        || !current_cwd.starts_with(seed_cwd)
}

pub(crate) fn default_continuity_state(
    shell: &colossal_linux_sandbox::shell::Shell,
) -> colossal_linux_sandbox::manager::PersistentSessionState {
    let workspace_root = crate::resolve_workspace_root();
    colossal_linux_sandbox::manager::PersistentSessionState {
        version: colossal_linux_sandbox::manager::SNAPSHOT_FORMAT_VERSION,
        session_id: SessionId::new("pending-shell-state".to_string()),
        shell_path: shell.path().to_string_lossy().to_string(),
        initial_cwd: workspace_root.clone(),
        env_vars: std::collections::HashMap::new(),
        current_cwd: workspace_root,
        created_at: std::time::SystemTime::now(),
        structured_env_json: None,
        nu_aliases: Vec::new(),
        nu_custom_commands: Vec::new(),
        nu_variables: Vec::new(),
        nu_replay_commands: Vec::new(),
        replay_commands: Vec::new(),
        nu_config: None,
    }
}

pub(crate) fn global_state() -> Option<&'static GlobalState> {
    GLOBAL_STATE.get()
}

pub async fn execution_mode_badge() -> &'static str {
    let shell_kind = if colossal_linux_sandbox::bundled_nu::managed_nu_requested() {
        colossal_linux_sandbox::shell::ShellKind::ManagedNu
    } else {
        let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        colossal_linux_sandbox::shell::shell_kind_from_path(&shell_path)
    };

    match shell_kind {
        colossal_linux_sandbox::shell::ShellKind::ManagedNu => "exec: NU",
        colossal_linux_sandbox::shell::ShellKind::Posix => "exec: PTY",
    }
}

pub(crate) async fn ensure_global_state_initialized() {
    if GLOBAL_STATE.get().is_none() {
        let shell = colossal_linux_sandbox::shell::default_user_shell().await;
        let safety_config = crate::safety_config::SafetyConfig::load().unwrap_or_default();
        let sandbox_policy = crate::sandbox_policy_from_config(&safety_config);
        let continuity_state = default_continuity_state(&shell);

        let _ = GLOBAL_STATE.set(GlobalState {
            manager: Arc::new(colossal_linux_sandbox::manager::SessionManager::default()),
            shell_session_id: tokio::sync::Mutex::new(None),
            shell,
            continuity_state: tokio::sync::Mutex::new(continuity_state),
            pending_sandbox_policy: tokio::sync::Mutex::new(sandbox_policy.clone()),
            effective_sandbox_policy: tokio::sync::Mutex::new(sandbox_policy),
            session_has_background_process: tokio::sync::Mutex::new(false),
            pending_approval: tokio::sync::Mutex::new(None),
        });
    }
}

pub async fn add_writable_root(path: PathBuf) -> Result<()> {
    ensure_global_state_initialized().await;

    let state = GLOBAL_STATE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Global state not initialized"))?;

    let mut policy_lock = state.pending_sandbox_policy.lock().await;

    match &mut *policy_lock {
        SandboxPolicy::WorkspaceWrite { writable_roots, .. } => {
            if writable_roots.iter().any(|root| root.root == path) {
                return Ok(());
            }

            writable_roots.push(colossal_linux_sandbox::protocol::WritableRoot {
                root: path,
                recursive: true,
                read_only_subpaths: vec![],
            });

            Ok(())
        }
        SandboxPolicy::DangerFullAccess => Ok(()),
        SandboxPolicy::ReadOnly => Err(anyhow::anyhow!(
            "Cannot add writable root in read-only mode"
        )),
    }
}

pub(crate) async fn get_or_create_shell_session(
    seed_cwd: Option<PathBuf>,
    env_overrides: std::collections::HashMap<String, String>,
) -> Result<(
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

    let has_background = *state.session_has_background_process.lock().await;
    let pending_policy = state.pending_sandbox_policy.lock().await.clone();
    let mut continuity_state = state.continuity_state.lock().await.clone();
    continuity_state.env_vars.extend(env_overrides);

    if let Some(seed_cwd) = seed_cwd.clone() {
        if should_seed_cwd(
            &continuity_state.current_cwd,
            &seed_cwd,
            &continuity_state.env_vars,
        ) {
            continuity_state.current_cwd = seed_cwd.clone();
            continuity_state.initial_cwd = seed_cwd;
        }
    }

    if let Some(existing) = session_id_lock.clone() {
        let effective_policy = state.effective_sandbox_policy.lock().await.clone();
        if !has_background && effective_policy != pending_policy {
            state.manager.terminate_session(existing).await?;
            *session_id_lock = None;

            let new_session_id =
                spawn_shell_session_with_snapshot(state, &pending_policy, &continuity_state)
                    .await?;
            *session_id_lock = Some(new_session_id.clone());
            *state.effective_sandbox_policy.lock().await = pending_policy.clone();
            return Ok((state.manager.clone(), new_session_id));
        }
    }

    if session_id_lock.is_none() || has_background {
        let session_id =
            spawn_shell_session_with_snapshot(state, &pending_policy, &continuity_state).await?;
        *session_id_lock = Some(session_id.clone());
        *state.effective_sandbox_policy.lock().await = pending_policy;
    }

    Ok((state.manager.clone(), session_id_lock.clone().unwrap()))
}

pub(crate) async fn spawn_shell_session_with_snapshot(
    state: &GlobalState,
    sandbox_policy: &SandboxPolicy,
    snapshot: &colossal_linux_sandbox::manager::PersistentSessionState,
) -> Result<SessionId> {
    let shared_state = Arc::new(colossal_linux_sandbox::session::SharedSessionState::new(
        snapshot.current_cwd.clone(),
    ));

    let session_id = state
        .manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            sandbox_policy.clone(),
            shared_state,
            None,
        )
        .await?;

    state
        .manager
        .restore_shell_session_from_snapshot(session_id.clone(), snapshot)
        .await?;

    Ok(session_id)
}

pub(crate) async fn sync_continuity_state_from_session(
    state: &GlobalState,
    session_id: SessionId,
    replayed_command: Option<&str>,
) -> Result<()> {
    let mut snapshot = state.manager.snapshot_shell_session(session_id).await?;
    let mut continuity_state = state.continuity_state.lock().await;
    if matches!(
        colossal_linux_sandbox::shell::shell_kind_from_path(&snapshot.shell_path),
        colossal_linux_sandbox::shell::ShellKind::ManagedNu
    ) {
        snapshot.replay_commands.clear();
    } else {
        snapshot.replay_commands = continuity_state.replay_commands.clone();
        if let Some(command) = replayed_command {
            snapshot.replay_commands.push(command.to_string());
        }
    }
    *continuity_state = snapshot;
    Ok(())
}

pub(crate) async fn run_isolated_exec_command(
    state: &GlobalState,
    command: &str,
    is_background: bool,
    timeout_ms: u64,
    cwd_hint: PathBuf,
    env_overrides: std::collections::HashMap<String, String>,
    ask_for_approval: Option<colossal_linux_sandbox::safety::AskForApproval>,
) -> std::result::Result<
    colossal_linux_sandbox::types::ExecCommandOutput,
    colossal_linux_sandbox::error::ColossalErr,
> {
    let mut continuity_state = state.continuity_state.lock().await.clone();
    continuity_state.env_vars.extend(env_overrides.clone());
    let sandbox_policy = state.pending_sandbox_policy.lock().await.clone();
    let shell_kind = state.shell.kind();
    let cwd = if should_seed_cwd(
        &continuity_state.current_cwd,
        &cwd_hint,
        &continuity_state.env_vars,
    ) {
        cwd_hint
    } else {
        continuity_state.current_cwd.clone()
    };

    if matches!(
        shell_kind,
        colossal_linux_sandbox::shell::ShellKind::ManagedNu
    ) && !is_background
    {
        let mut runtime = colossal_linux_sandbox::managed_nu::ManagedNuRuntime::from_snapshot(
            continuity_state.shell_path.clone(),
            sandbox_policy.clone(),
            &continuity_state,
        )
        .map_err(|err| {
            colossal_linux_sandbox::error::ColossalErr::Io(std::io::Error::other(err.to_string()))
        })?;
        runtime.update_cwd(cwd).map_err(|err| {
            colossal_linux_sandbox::error::ColossalErr::Io(std::io::Error::other(err.to_string()))
        })?;
        return runtime.fork_eval(command.to_string()).map_err(|err| {
            colossal_linux_sandbox::error::ColossalErr::Io(std::io::Error::other(err.to_string()))
        });
    }

    state
        .manager
        .handle_exec_command_request(colossal_linux_sandbox::types::ExecCommandParams {
            command: vec![command.to_string()],
            shell: state.shell.clone(),
            cwd,
            env: if matches!(
                shell_kind,
                colossal_linux_sandbox::shell::ShellKind::ManagedNu
            ) {
                env_overrides
            } else {
                continuity_state.env_vars
            },
            timeout_ms: if is_background {
                None
            } else {
                Some(timeout_ms)
            },
            max_output_tokens: 1000,
            sandbox_policy,
            is_background,
            ask_for_approval,
        })
        .await
}
