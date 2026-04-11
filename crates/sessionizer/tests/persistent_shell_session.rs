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

#[tokio::test]
async fn silent_redirect_command_completes_and_writes_file()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "printf 'hello world' > silent.txt".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.trim().is_empty(),
        "silent command produced visible output: {:?}",
        result.stdout
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("silent.txt"))?,
        "hello world"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn shell_session_preserves_cwd_and_environment_across_commands()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    std::fs::create_dir_all(temp.path().join("nested"))?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested && export TEST_VALUE=persisted && pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("nested"),
        "unexpected pwd output: {}",
        result.stdout
    );

    let pwd_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(pwd_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        pwd_result
            .stdout
            .contains(&temp.path().join("nested").display().to_string()),
        "unexpected pwd output: {:?}",
        pwd_result.stdout
    );

    let env_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "printf '%s' \"$TEST_VALUE\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(env_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        env_result.stdout.contains("persisted"),
        "unexpected env output: {:?}",
        env_result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn timeout_does_not_prevent_next_command_from_running()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let timeout_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "sleep 2".to_string(),
            Some(100),
            1_000,
            None,
        )
        .await?;
    assert_eq!(timeout_result.exit_status, ExitStatus::Timeout);

    let recovered = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo recovered".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(recovered.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        recovered.stdout.contains("recovered"),
        "unexpected recovery output: {:?}",
        recovered.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn non_zero_exit_preserves_output_and_status() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "sh -c 'printf boom >&2; exit 7'".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 7 });
    assert!(
        result.stdout.contains("boom"),
        "missing stderr text in PTY output: {}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_shell_completes_simple_command() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo sandbox-ok".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("sandbox-ok"),
        "unexpected sandbox output: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_completes_simple_command()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo user-shell-ok".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("user-shell-ok"),
        "unexpected user shell output: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_silent_redirect_completes_and_writes_file()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "printf 'sandbox data' > sandbox.txt".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.trim().is_empty(),
        "unexpected visible output for sandbox redirect: {:?}",
        result.stdout
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("sandbox.txt"))?,
        "sandbox data"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_env_output_is_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let _ = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export TEST_VALUE=cleanvalue".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo $TEST_VALUE".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(result.stdout, "cleanvalue");

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_cwd_output_is_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    std::fs::create_dir_all(temp.path().join("nested"))?;
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested && pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(
        result.stdout,
        temp.path().join("nested").display().to_string()
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_stream_output_is_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let stream = manager
        .send_command_to_shell_session(
            session_id.clone(),
            "printf 'stream-one\\nstream-two\\n'".to_string(),
        )
        .await?;

    let mut stream = stream;
    let mut saw_output = String::new();
    let mut exit_code = None;
    for _ in 0..10 {
        let event = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await??;
        match event {
            StreamEvent::Stdout(output) => saw_output.push_str(&output),
            StreamEvent::Exit(code) => {
                exit_code = Some(code);
                break;
            }
            StreamEvent::Stderr(output) => saw_output.push_str(&output),
            StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
        }
    }

    assert_eq!(exit_code, Some(0));
    assert_eq!(saw_output.trim(), "stream-one\nstream-two");

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_compound_cwd_output_is_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("nested");
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!(
                "mkdir -p {} && cd {} && pwd",
                target.display(),
                target.display()
            ),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(result.stdout, target.display().to_string());

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn workspace_write_default_shell_stream_compound_output_is_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let default_shell = shell::default_user_shell().await;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        default_shell.path().to_string_lossy().to_string(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let stream = manager
        .send_command_to_shell_session(
            session_id.clone(),
            "echo 'This is streaming output' && sleep 1 && echo 'Streaming complete'".to_string(),
        )
        .await?;

    let mut stream = stream;
    let mut saw_output = String::new();
    let mut exit_code = None;
    for _ in 0..12 {
        let event = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await??;
        match event {
            StreamEvent::Stdout(output) => saw_output.push_str(&output),
            StreamEvent::Exit(code) => {
                exit_code = Some(code);
                break;
            }
            StreamEvent::Stderr(output) => saw_output.push_str(&output),
            StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
        }
    }

    assert_eq!(exit_code, Some(0));
    assert_eq!(
        saw_output.trim(),
        "This is streaming output\nStreaming complete"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn streaming_command_emits_exit_when_command_finishes()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let stream = manager
        .send_command_to_shell_session(
            session_id.clone(),
            "printf 'stream-one\\nstream-two\\n'".to_string(),
        )
        .await?;

    let mut saw_output = String::new();
    let mut exit_code = None;
    for _ in 0..10 {
        let event = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await??;
        match event {
            StreamEvent::Stdout(output) => saw_output.push_str(&output),
            StreamEvent::Exit(code) => {
                exit_code = Some(code);
                break;
            }
            StreamEvent::Stderr(output) => saw_output.push_str(&output),
            StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
        }
    }

    assert_eq!(exit_code, Some(0));
    assert!(saw_output.contains("stream-one"));
    assert!(saw_output.contains("stream-two"));

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_snapshot_and_replay_preserve_tracked_state()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    std::fs::create_dir_all(temp.path().join("nested"))?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager.update_cwd_in_shell_session(session_id.clone(), temp.path().join("nested"))?;
    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "TEST_VALUE".to_string(),
            "persisted".to_string(),
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(snapshot.current_cwd, temp.path().join("nested"));
    assert_eq!(
        snapshot.env_vars.get("TEST_VALUE").map(String::as_str),
        Some("persisted")
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let replay_session_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    manager.update_cwd_in_shell_session(replay_session_id.clone(), snapshot.current_cwd.clone())?;
    manager
        .set_env_in_shell_session(
            replay_session_id.clone(),
            "TEST_VALUE".to_string(),
            "persisted".to_string(),
        )
        .await?;

    let replayed = manager
        .exec_command_in_shell_session(
            replay_session_id.clone(),
            "$env.TEST_VALUE".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    assert!(
        replayed.stdout.contains("persisted"),
        "stdout={:?} stderr={:?} agg={:?}",
        replayed.stdout,
        replayed.stderr,
        replayed.aggregated_output
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(replay_session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_load_env_command_updates_runtime_owned_env()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { TEST_VALUE: persisted }; $env.TEST_VALUE".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    assert!(result.stdout.contains("persisted"), "{}", result.stdout);

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("TEST_VALUE").map(String::as_str),
        Some("persisted")
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_def_persists_across_exec_and_survives_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define a custom command
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def double [n: int] { $n * 2 }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    // Call it in a separate exec
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "double 21".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("42"),
        "def should persist across exec calls: stdout={:?}",
        result.stdout
    );

    // Snapshot should capture the def structurally
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.nu_custom_commands.is_empty(),
        "snapshot should have custom commands"
    );
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|s| s.contains("double")),
        "snapshot should contain the double def"
    );

    // Restore into a new session and verify the def survived
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for (key, value) in &snapshot.env_vars {
        manager
            .set_env_in_shell_session(restored_id.clone(), key.clone(), value.clone())
            .await?;
    }
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "double 21".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("42"),
        "def should survive snapshot/restore: stdout={:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_alias_persists_across_exec_and_survives_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define a custom command, then alias it
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def greet [] { \"hi\" }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias say_hi = greet".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    // Call alias in a separate exec
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "say_hi".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("hi"),
        "alias should persist across exec calls: stdout={:?}",
        result.stdout
    );

    // Snapshot should capture both def and alias structurally
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.nu_aliases.is_empty(),
        "snapshot should have aliases"
    );
    assert!(
        snapshot.nu_aliases.iter().any(|a| a.name == "say_hi"),
        "snapshot should contain the say_hi alias"
    );

    // Restore into a new session
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }
    for alias in &snapshot.nu_aliases {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                format!("alias {} = {}", alias.name, alias.expansion),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "say_hi".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("hi"),
        "alias should survive snapshot/restore: stdout={:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_command_updates_and_persists_cwd() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let nested = std::fs::canonicalize(temp.path())?.join("nested");
    std::fs::create_dir_all(&nested)?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Command-driven cd
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    // Verify cwd changed via a separate exec
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("nested"),
        "cd should update cwd: stdout={:?}",
        result.stdout
    );

    // Snapshot should capture the cwd
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(snapshot.current_cwd, nested);
    // cd should NOT be in replay_commands (it's first-class via current_cwd)
    assert!(
        !snapshot
            .replay_commands
            .iter()
            .any(|c| c.starts_with("cd ")),
        "cd should not be in replay_commands: {:?}",
        snapshot.replay_commands
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;

    let restored = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "pwd".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        restored.stdout.contains(&nested.display().to_string()),
        "restored cwd should be observed by command execution: stdout={:?}",
        restored.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_invalid_cd_fails_without_corrupting_cwd()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let nested = std::fs::canonicalize(temp.path())?.join("nested");
    std::fs::create_dir_all(&nested)?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager.update_cwd_in_shell_session(session_id.clone(), nested.clone())?;

    let failed = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd does-not-exist".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    assert!(matches!(
        failed.exit_status,
        ExitStatus::Completed { code: 1 }
    ));

    let cwd_check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        cwd_check.stdout.contains(&nested.display().to_string()),
        "invalid cd must not corrupt managed cwd: stdout={:?}",
        cwd_check.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_overlay_commands_fail_predictably() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // All four overlay subcommands must be rejected with a stable error.
    for form in &[
        "overlay use foo",
        "overlay hide foo",
        "overlay new myoverlay",
        "overlay list",
    ] {
        let failed = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                form.to_string(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;

        assert!(
            matches!(failed.exit_status, ExitStatus::Completed { code: 1 }),
            "{form:?} should exit with code 1, got {:?}",
            failed.exit_status
        );
        assert!(
            failed
                .aggregated_output
                .contains("does not support overlay commands"),
            "{form:?} failure should be explicit: {:?}",
            failed.aggregated_output
        );
    }

    // After all rejections the managed-nu state must be clean.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "failed overlay commands must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        snapshot.nu_aliases.is_empty(),
        "failed overlay commands must not corrupt nu_aliases: {:?}",
        snapshot.nu_aliases
    );
    assert!(
        snapshot.nu_variables.is_empty(),
        "failed overlay commands must not corrupt nu_variables: {:?}",
        snapshot.nu_variables
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_hide_env_removes_env_var() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Set then unset via command
    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "REMOVE_ME".to_string(),
            "exists".to_string(),
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "hide-env REMOVE_ME".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("REMOVE_ME"),
        "hide-env should remove the var from snapshot env: {:?}",
        snapshot.env_vars.get("REMOVE_ME")
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_def_redefinition_replaces_previous() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def val [] { 1 }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def val [] { 2 }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    // Calling val should return the latest definition
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "val".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains('2'),
        "redefined def should use latest: stdout={:?}",
        result.stdout
    );

    // Snapshot should have exactly one entry for 'val'
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    let val_entries: Vec<_> = snapshot
        .nu_custom_commands
        .iter()
        .filter(|s| s.contains("val"))
        .collect();
    assert_eq!(
        val_entries.len(),
        1,
        "redefined def should replace, not accumulate: {:?}",
        val_entries
    );
    assert!(
        val_entries[0].contains('2'),
        "snapshot should contain the latest def: {:?}",
        val_entries[0]
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_alias_redefinition_replaces_previous() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def a1 [] { \"first\" }; def a2 [] { \"second\" }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias thing = a1".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias thing = a2".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "thing".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("second"),
        "redefined alias should use latest: stdout={:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    let thing_entries: Vec<_> = snapshot
        .nu_aliases
        .iter()
        .filter(|a| a.name == "thing")
        .collect();
    assert_eq!(
        thing_entries.len(),
        1,
        "redefined alias should replace, not accumulate: {:?}",
        thing_entries
    );
    assert!(
        thing_entries[0].expansion.contains("a2"),
        "snapshot should contain the latest alias expansion: {:?}",
        thing_entries[0].expansion
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_load_env_overwrites_existing_key() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { MY_KEY: original }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { MY_KEY: updated }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.MY_KEY".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("updated"),
        "load-env should overwrite existing key: stdout={:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("MY_KEY").map(String::as_str),
        Some("updated")
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_let_variable_survives_snapshot_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let persisted = 42; $persisted".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("42"),
        "let binding should work within a session: stdout={:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("persisted"),
        "let-bound variable should not leak into env snapshot"
    );
    assert!(
        snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$persisted" && !variable.mutable),
        "let-bound variable should be captured structurally: {:?}",
        snapshot.nu_variables
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    for variable in &snapshot.nu_variables {
        manager
            .set_managed_variable_in_shell_session(restored_id.clone(), variable.clone())
            .await?;
    }
    let restored_snapshot = manager.snapshot_shell_session(restored_id.clone()).await?;
    assert!(
        restored_snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$persisted"),
        "restored snapshot should contain let variable: {:?}",
        restored_snapshot.nu_variables
    );

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$\"($persisted)\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "let-bound variable should survive snapshot/restore: stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );
    assert!(
        result.stdout.contains("42"),
        "stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_unsupported_load_env_type_fails_predictably()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // load-env with a non-record value should fail, not silently corrupt state
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env \"not a record\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "load-env with non-record should fail: stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );

    // Existing env should be unaffected
    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "SAFE_KEY".to_string(),
            "intact".to_string(),
        )
        .await?;
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("SAFE_KEY").map(String::as_str),
        Some("intact"),
        "failed load-env should not corrupt existing env state"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── error model consistency tests ────────────────────────────────────

#[tokio::test]
async fn managed_nu_partial_output_preserved_on_error() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // First segment succeeds, second fails
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "\"hello\"; overlay use nonexistent".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "compound command with failing segment should return non-zero"
    );
    // Partial stdout from successful first segment should be preserved
    assert!(
        result.stdout.contains("hello"),
        "partial output from successful segment should be in stdout: {:?}",
        result.stdout
    );
    // Error should be in stderr
    assert!(
        !result.stderr.is_empty(),
        "error message should be in stderr"
    );
    // aggregated_output should have both
    assert!(
        result.aggregated_output.contains("hello") && result.aggregated_output.contains("overlay"),
        "aggregated_output should contain both partial output and error: {:?}",
        result.aggregated_output
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_error_preserves_prior_state_changes() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // First segment sets env, second fails
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { BEFORE_ERROR: \"yes\" }; overlay use nonexistent".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 },);

    // State change from the first segment should have taken effect
    let check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.BEFORE_ERROR".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        check.stdout.contains("yes"),
        "state changes before error should persist: {:?}",
        check.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── def/alias semantics hardening tests ──────────────────────────────

#[tokio::test]
async fn managed_nu_def_with_env_flag() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // def --env sets environment from within the command body
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def --env setfoo [] { $env.MY_FLAG_VAR = \"flagged\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // Call the env-mutating def
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "setfoo".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.MY_FLAG_VAR".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("flagged"),
        "def --env should allow env mutation from the body: {:?}",
        result.stdout
    );

    // Verify the def was registered (name extraction handles --env flag)
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|s| s.contains("setfoo")),
        "def --env should be registered in custom_commands"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_def_calling_another_def_survives_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define A then B which calls A
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def add-one [n: int] { $n + 1 }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def add-two [n: int] { add-one (add-one $n) }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // Verify it works before snapshot
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "add-two 10".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("12"),
        "add-two should return 12: {:?}",
        result.stdout
    );

    // Snapshot and restore
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }

    // Verify the chained def works after restore
    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "add-two 10".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("12"),
        "chained def should survive restore: {:?}",
        result.stdout
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_alias_referencing_def_survives_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define a custom command, then alias it
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def triple [n: int] { $n * 3 }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias t = triple".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "t 7".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("21"),
        "alias of def should work: {:?}",
        result.stdout
    );

    // Snapshot and restore — defs are restored before aliases
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }
    for alias in &snapshot.nu_aliases {
        let source = format!("alias {} = {}", alias.name, alias.expansion);
        manager
            .exec_command_in_shell_session(restored_id.clone(), source, Some(10_000), 10_000, None)
            .await?;
    }

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "t 7".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("21"),
        "alias of def should survive restore: {:?}",
        result.stdout
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_multiline_def_preserved_as_one_segment()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Multiline def using braces — semicolons inside braces should NOT split
    let multiline_def = "def compute [n: int] { let x = $n * 2; $x + 1 }";
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            multiline_def.to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "compute 10".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("21"),
        "multiline def should work (10*2+1=21): {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_invalid_def_does_not_register() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Invalid def syntax — should fail
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def { broken }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "invalid def should fail"
    );

    // Snapshot should have no custom commands
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "invalid def should not be registered: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── env semantics hardening tests ────────────────────────────────────

#[tokio::test]
async fn managed_nu_hide_env_with_quoted_name() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "QUOTED_VAR".to_string(),
            "exists".to_string(),
        )
        .await?;

    // hide-env with double-quoted name should work
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "hide-env \"QUOTED_VAR\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("QUOTED_VAR"),
        "hide-env with quoted name should remove the var"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_hide_env_pwd_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let canonical = std::fs::canonicalize(temp.path())?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "hide-env PWD".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "hide-env PWD should be rejected"
    );

    // cwd should be unchanged
    let pwd_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        pwd_result.stdout.contains(&canonical.display().to_string()),
        "PWD should be unaffected after rejected hide-env PWD: {:?}",
        pwd_result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_env_assignment_syntax() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // $env.FOO = "bar" syntax (native Nu env assignment)
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.DIRECT_SET = \"hello_direct\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.DIRECT_SET".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("hello_direct"),
        "$env.X = val should be readable: {:?}",
        result.stdout
    );

    // Should survive snapshot
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("DIRECT_SET").map(String::as_str),
        Some("hello_direct"),
        "$env.X = val should appear in snapshot"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_load_env_pwd_syncs_cwd() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let subdir = std::fs::canonicalize(temp.path())?.join("envpwd");
    std::fs::create_dir_all(&subdir)?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Setting PWD via load-env should update the tracked cwd
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("load-env {{ PWD: \"{}\" }}", subdir.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("envpwd"),
        "load-env PWD should sync tracked cwd: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_env_special_chars_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Set env with special characters
    let special_value = "hello=world;foo&bar|baz";
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("load-env {{ SPECIAL: \"{}\" }}", special_value),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // Snapshot
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("SPECIAL").map(String::as_str),
        Some(special_value),
        "special chars should survive snapshot"
    );
    manager.terminate_session(session_id.clone()).await?;

    // Restore by creating new session and replaying env
    let (manager2, session_id2) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;
    for (key, value) in &snapshot.env_vars {
        manager2
            .set_env_in_shell_session(session_id2.clone(), key.clone(), value.clone())
            .await?;
    }

    let result = manager2
        .exec_command_in_shell_session(
            session_id2.clone(),
            "$env.SPECIAL".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains(special_value),
        "special chars should survive snapshot+restore: {:?}",
        result.stdout
    );

    manager2.terminate_session(session_id2).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_env_var_survives_snapshot_restore() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Set an env via load-env and another via $env.X = val
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { VIA_LOAD: \"loadval\" }; $env.VIA_ASSIGN = \"assignval\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

    // Restore by creating new session and replaying env
    let (manager2, session_id2) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;
    for (key, value) in &snapshot.env_vars {
        manager2
            .set_env_in_shell_session(session_id2.clone(), key.clone(), value.clone())
            .await?;
    }

    let r1 = manager2
        .exec_command_in_shell_session(
            session_id2.clone(),
            "$env.VIA_LOAD".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        r1.stdout.contains("loadval"),
        "load-env var should survive restore: {:?}",
        r1.stdout
    );
    let r2 = manager2
        .exec_command_in_shell_session(
            session_id2.clone(),
            "$env.VIA_ASSIGN".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        r2.stdout.contains("assignval"),
        "$env.X = val should survive restore: {:?}",
        r2.stdout
    );

    manager2.terminate_session(session_id2).await?;
    Ok(())
}

// ── cwd semantics hardening tests ────────────────────────────────────

#[tokio::test]
async fn managed_nu_cd_bare_goes_to_home() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        result.stdout.contains(&home),
        "bare cd should go to HOME: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_tilde_goes_to_home() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd ~".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        result.stdout.contains(&home),
        "cd ~ should go to HOME: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_tilde_subdir() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Create a directory under HOME to test ~/subdir
    let home = std::env::var("HOME").unwrap_or_default();
    let test_dir = std::path::PathBuf::from(&home);
    // Use an existing directory under home (we'll use . which is always valid)
    if test_dir.exists() {
        manager
            .exec_command_in_shell_session(
                session_id.clone(),
                "cd ~/".to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                "pwd".to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert!(
            result.stdout.contains(&home),
            "cd ~/ should resolve to HOME: {:?}",
            result.stdout
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_dotdot() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let nested = std::fs::canonicalize(temp.path())?.join("a").join("b");
    std::fs::create_dir_all(&nested)?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager.update_cwd_in_shell_session(session_id.clone(), nested.clone())?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd ..".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    let parent = std::fs::canonicalize(temp.path())?.join("a");
    assert!(
        result.stdout.contains(&parent.display().to_string()),
        "cd .. should go to parent: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_absolute_path() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let target = std::fs::canonicalize(temp.path())?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cd {}", target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains(&target.display().to_string()),
        "cd with absolute path should work: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_nonexistent_fails() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let original = std::fs::canonicalize(temp.path())?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd /nonexistent_path_12345".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "cd to nonexistent path should fail"
    );

    // cwd should be unchanged
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains(&original.display().to_string()),
        "cwd should be unchanged after failed cd: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_cd_path_with_spaces() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let spaced = std::fs::canonicalize(temp.path())?.join("dir with spaces");
    std::fs::create_dir_all(&spaced)?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // cd with quoted path containing spaces
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cd \"{}\"", spaced.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("dir with spaces"),
        "cd with spaces in path should work: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── full end-to-end rotation suite ───────────────────────────────────

/// Comprehensive test simulating a full session rotation: build up all state
/// types (env, cwd, defs, aliases), snapshot, terminate, create fresh session,
/// restore from snapshot, and verify every piece of state survived.
#[tokio::test]
async fn managed_nu_full_rotation_preserves_all_state() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let subdir = std::fs::canonicalize(temp.path())?.join("workdir");
    std::fs::create_dir_all(&subdir)?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // 1. Set multiple env vars via different methods
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { E1: \"via_load_env\", E2: \"second\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.E3 = \"via_assign\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .set_env_in_shell_session(session_id.clone(), "E4".to_string(), "via_api".to_string())
        .await?;

    // 2. Change working directory
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cd \"{}\"", subdir.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // 3. Define custom commands
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def add-ten [n: int] { $n + 10 }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def greet [name: string] { $\"hello ($name)\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // 4. Define aliases
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias a10 = add-ten".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // 5. Remove one env var (verify hide-env is reflected)
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "hide-env E2".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // ─── Snapshot ───────────────────────────────────────────────────
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

    // Verify snapshot contents
    assert_eq!(
        snapshot.env_vars.get("E1").map(String::as_str),
        Some("via_load_env")
    );
    assert!(!snapshot.env_vars.contains_key("E2"), "E2 was hidden");
    assert_eq!(
        snapshot.env_vars.get("E3").map(String::as_str),
        Some("via_assign")
    );
    assert_eq!(
        snapshot.env_vars.get("E4").map(String::as_str),
        Some("via_api")
    );
    assert_eq!(snapshot.current_cwd, subdir);
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|s| s.contains("add-ten"))
    );
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|s| s.contains("greet"))
    );
    assert!(snapshot.nu_aliases.iter().any(|a| a.name == "a10"));

    // ─── Restore into fresh session ─────────────────────────────────
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    // Replay snapshot state (mimics what the rotation logic does)
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for (key, value) in &snapshot.env_vars {
        manager
            .set_env_in_shell_session(restored_id.clone(), key.clone(), value.clone())
            .await?;
    }
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }
    for alias in &snapshot.nu_aliases {
        let source = format!("alias {} = {}", alias.name, alias.expansion);
        manager
            .exec_command_in_shell_session(restored_id.clone(), source, Some(10_000), 10_000, None)
            .await?;
    }

    // ─── Verify all state survived ─────────────────────────────────

    // Env vars
    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$env.E1".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("via_load_env"), "E1: {:?}", r.stdout);

    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$env.E3".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("via_assign"), "E3: {:?}", r.stdout);

    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$env.E4".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("via_api"), "E4: {:?}", r.stdout);

    // Working directory
    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "pwd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("workdir"), "cwd: {:?}", r.stdout);

    // Custom commands
    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "add-ten 5".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("15"), "add-ten 5: {:?}", r.stdout);

    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "greet world".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("hello world"), "greet: {:?}", r.stdout);

    // Alias
    let r = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "a10 5".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(r.stdout.contains("15"), "alias a10: {:?}", r.stdout);

    manager.terminate_session(restored_id).await?;
    Ok(())
}

/// Test that fork_eval works correctly after a rotation — state accumulated
/// before rotation is visible in forked evaluation after restore.
#[tokio::test]
async fn managed_nu_fork_eval_works_after_rotation() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Build some state
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def square [n: int] { $n * $n }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { MY_KEY: \"rotation_val\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // Snapshot and terminate
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

    // Restore
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            snapshot.shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    manager.update_cwd_in_shell_session(restored_id.clone(), snapshot.current_cwd.clone())?;
    for (key, value) in &snapshot.env_vars {
        manager
            .set_env_in_shell_session(restored_id.clone(), key.clone(), value.clone())
            .await?;
    }
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }

    // fork_eval should see the restored state
    let fork_result = manager.fork_eval_in_managed_nu_session(
        restored_id.clone(),
        "square 7".to_string(),
        None,
    )?;
    let fork_result = fork_result.expect("should be Some for managed Nu");
    assert!(
        fork_result.stdout.contains("49"),
        "fork_eval should see restored def: {:?}",
        fork_result.stdout
    );

    let fork_result = manager.fork_eval_in_managed_nu_session(
        restored_id.clone(),
        "$env.MY_KEY".to_string(),
        None,
    )?;
    let fork_result = fork_result.expect("should be Some for managed Nu");
    assert!(
        fork_result.stdout.contains("rotation_val"),
        "fork_eval should see restored env: {:?}",
        fork_result.stdout
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}

// ── shell variable boundary tests ────────────────────────────────────

#[tokio::test]
async fn managed_nu_mut_variable_survives_snapshot_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "mut x = 10; $x = 20; $x".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("20"),
        "mut variable should work within session: {:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("x"),
        "mut variable should not leak into env snapshot"
    );
    assert!(
        snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$x" && variable.mutable),
        "mut variable should be in nu_variables: {:?}",
        snapshot.nu_variables
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    for variable in &snapshot.nu_variables {
        manager
            .set_managed_variable_in_shell_session(restored_id.clone(), variable.clone())
            .await?;
    }
    let restored_snapshot = manager.snapshot_shell_session(restored_id.clone()).await?;
    assert!(
        restored_snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$x"),
        "restored snapshot should contain mut variable: {:?}",
        restored_snapshot.nu_variables
    );

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$x = 30; $\"($x)\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "mut variable should survive snapshot/restore: stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );
    assert!(
        result.stdout.contains("30"),
        "stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_const_works_within_session() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // const works within a single command
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "const PI = 3.14; $PI".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("3.14"),
        "const should work within session: {:?}",
        result.stdout
    );

    // But const should NOT be in snapshot
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("PI"),
        "const should not leak into env snapshot"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_immutable_let_reassignment_fails_predictably()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let answer = 41; $answer = 42".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$answer".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert!(check.stdout.contains("41"), "{:?}", check.stdout);

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_block_local_variable_does_not_leak_into_persistent_state()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "if true { let hidden = 7 }; $hidden".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$hidden"),
        "block-local variable should not leak into persistent snapshot: {:?}",
        snapshot.nu_variables
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    let restored = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$hidden".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_ne!(restored.exit_status, ExitStatus::Completed { code: 0 });

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_variable_value_shapes_survive_snapshot_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let s = 'quo\"te'; let n = 7; let b = true; let items = [1 2 'x']; let rec = {a: 1, nested: {b: 'y'}}; $\"($s)|($n)|($b)|($items)|($rec)\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$items")
    );
    assert!(
        snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$rec")
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;
    for variable in &snapshot.nu_variables {
        manager
            .set_managed_variable_in_shell_session(restored_id.clone(), variable.clone())
            .await?;
    }
    let restored_snapshot = manager.snapshot_shell_session(restored_id.clone()).await?;
    assert!(
        restored_snapshot
            .nu_variables
            .iter()
            .any(|variable| variable.name == "$items"),
        "restored snapshot should contain structured variables: {:?}",
        restored_snapshot.nu_variables
    );

    let restored = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$\"($s)|($n)|($b)|($items)|($rec)\"".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(
        restored.exit_status,
        ExitStatus::Completed { code: 0 },
        "stdout={:?} stderr={:?}",
        restored.stdout,
        restored.stderr
    );
    assert_eq!(result.stdout, restored.stdout);

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

// ── external command and boundary tests ──────────────────────────────

#[tokio::test]
async fn managed_nu_nu_builtins_work_in_embedded_runtime() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Nu lang builtins should work in the embedded runtime
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "3 + 4".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("7"),
        "Nu builtins should work: {:?}",
        result.stdout
    );

    // String operations that are part of nu-cmd-lang
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            r#""hello" ++ " world""#.to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("hello world"),
        "string concat should work: {:?}",
        result.stdout
    );

    // Conditionals
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "if true { \"yes\" } else { \"no\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("yes"),
        "conditionals should work: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_external_commands_explicitly_rejected() -> Result<(), Box<dyn std::error::Error>>
{
    // ^cmd and run-external are explicitly rejected with a stable error message
    // directing agents to the agent-core exec path.
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // ^cmd form
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "^echo external-test".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "^echo should be explicitly rejected: {:?}",
        result.exit_status
    );
    assert!(
        result.aggregated_output.contains("external commands"),
        "rejection message should mention external commands: {:?}",
        result.aggregated_output
    );

    // run-external form
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "run-external echo external-test".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "run-external should be explicitly rejected: {:?}",
        result.exit_status
    );
    assert!(
        result.aggregated_output.contains("external commands"),
        "run-external rejection message should mention external commands: {:?}",
        result.aggregated_output
    );

    // Rejection must not corrupt state.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "external command rejection must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_fork_eval_rejects_external_commands() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &["^echo hello", "^git status", "run-external echo hello"] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{form}' should be rejected in fork_eval: {:?}",
            result.aggregated_output
        );
        assert!(
            result.aggregated_output.contains("external commands"),
            "'{form}' fork_eval rejection must mention external commands: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── config mutation tests ────────────────────────────────────────────

#[tokio::test]
async fn managed_nu_config_mutations_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    // $env.config mutations are explicitly rejected — the config record is not
    // snapshot/restore-safe, so silent success followed by silent drop on
    // rotation would be a best-effort middle ground.
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Sub-field mutation must be rejected.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = false".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "$env.config sub-field mutation should be rejected: {:?}",
        result.exit_status
    );
    assert!(
        result.aggregated_output.contains("$env.config"),
        "rejection message should mention $env.config: {:?}",
        result.aggregated_output
    );

    // Whole-config replacement must also be rejected.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config = {}".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "whole $env.config replacement should be rejected: {:?}",
        result.exit_status
    );

    // After rejections the managed-nu state must be clean.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "config rejection must not corrupt state: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        !snapshot.env_vars.contains_key("config"),
        "$env.config must not appear as a string env var: {:?}",
        snapshot.env_vars.keys().collect::<Vec<_>>()
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_read_not_intercepted_by_rejection()
-> Result<(), Box<dyn std::error::Error>> {
    // Read-only accesses to $env.config must NOT trigger our explicit rejection.
    // (In embedded nu-cmd-lang, $env.config may not exist, so Nu produces its own
    // "Cannot find column" error — that is acceptable.  What must NOT happen is our
    // stable rejection message appearing for a read expression.)
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &[
        "$env.config",
        "$env.config.show_banner == false",
        "let x = $env.config",
    ] {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                form.to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        // Whatever happens, it must NOT be our config-mutation rejection.
        assert!(
            !result
                .aggregated_output
                .contains("does not support $env.config mutations"),
            "read-only '{form}' must not trigger config mutation rejection: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_mutation_rejected_in_fork_eval() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &[
        "$env.config.show_banner = false",
        "$env.config = {}",
        "$env.config.table.mode = \"light\"",
    ] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{form}' should be rejected in fork_eval: {:?}",
            result.aggregated_output
        );
        assert!(
            result.aggregated_output.contains("$env.config"),
            "'{form}' fork_eval rejection must mention $env.config: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── exported declaration and module/use tests ────────────────────────

#[tokio::test]
async fn managed_nu_export_def_persists_and_survives_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define an exported command
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export def greet-exported [] { \"exported hello\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });

    // It should be callable
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "greet-exported".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("exported hello"),
        "export def command should be callable: {:?}",
        result.stdout
    );

    // Snapshot and restore
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|c| c.contains("greet-exported")),
        "export def should be in snapshot custom_commands"
    );

    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path,
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    // Replay custom commands from snapshot
    for source in &snapshot.nu_custom_commands {
        manager
            .exec_command_in_shell_session(
                restored_id.clone(),
                source.clone(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;
    }

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "greet-exported".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("exported hello"),
        "export def should survive snapshot/restore: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_export_alias_persists_and_survives_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Define an exported alias
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def base-cmd [] { \"alias base\" }; export alias ea = base-cmd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "ea".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("alias base"),
        "export alias should be callable: {:?}",
        result.stdout
    );

    // Snapshot should contain it
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_aliases.iter().any(|a| a.name == "ea"),
        "export alias should be in snapshot aliases"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_export_def_redefinition_replaces_previous()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export def redef [] { \"v1\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export def redef [] { \"v2\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "redef".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("v2"),
        "export def redefinition should use latest: {:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    let matching = snapshot
        .nu_custom_commands
        .iter()
        .filter(|c| c.contains("redef"))
        .count();
    assert_eq!(
        matching, 1,
        "redefined export def should be stored once, not duplicated"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_module_commands_fail_predictably() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // All eight explicitly-rejected module/use/source forms must fail with a
    // stable, explicit error message.
    let unsupported_commands = [
        "module foo { export def bar [] { 1 } }",
        "use std",
        "source nonexistent.nu",
        "source-env nonexistent.nu",
        "export use std",
        "export module mymod { }",
        "export extern my-ext []",
        "export const MY_CONST = 42",
    ];

    for cmd in &unsupported_commands {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                cmd.to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert!(
            matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
            "'{cmd}' should exit with code 1, got {:?}",
            result.exit_status
        );
        assert!(
            result.aggregated_output.contains("module/use/source"),
            "rejection for '{cmd}' should mention module/use/source: {:?}",
            result.aggregated_output
        );
    }

    // After all rejections the managed-nu state must be clean.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "failed module commands must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        snapshot.nu_aliases.is_empty(),
        "failed module commands must not corrupt nu_aliases: {:?}",
        snapshot.nu_aliases
    );
    assert!(
        snapshot.nu_variables.is_empty(),
        "failed module commands must not corrupt nu_variables: {:?}",
        snapshot.nu_variables
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_module_command_atomicity_blocks_following_segment()
-> Result<(), Box<dyn std::error::Error>> {
    // A multi-segment command where the first segment is a rejected module
    // command must not allow subsequent segments to execute.
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // module command fails → the def that follows must NOT be registered.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "module bad {}; def should_not_exist [] { 99 }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "module + def must fail: {:?}",
        result.exit_status
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "def after a module command must not be registered: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_fork_eval_rejects_module_commands() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let unsupported_commands = [
        "module foo { export def bar [] { 1 } }",
        "use std",
        "source nonexistent.nu",
        "source-env nonexistent.nu",
        "export use std",
        "export module mymod { }",
        "export extern my-ext []",
        "export const MY_CONST = 42",
    ];

    for cmd in &unsupported_commands {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), cmd.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{cmd}' should fail in fork_eval: {:?}",
            result.stderr
        );
        assert!(
            result.aggregated_output.contains("module/use/source"),
            "'{cmd}' fork_eval rejection should be explicit: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── fork_eval tests ──────────────────────────────────────────────────

#[tokio::test]
async fn managed_nu_fork_eval_sees_session_state_without_mutating_it()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let nested = std::fs::canonicalize(temp.path())?.join("sub");
    std::fs::create_dir_all(&nested)?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    // Set up state in the real runtime
    manager
        .set_env_in_shell_session(
            session_id.clone(),
            "FORK_VAR".to_string(),
            "visible".to_string(),
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "def fork-greet [] { \"hello from fork\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "let fork_let = 12; mut fork_mut = 20; $fork_mut = 21".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager.update_cwd_in_shell_session(session_id.clone(), nested.clone())?;

    // fork_eval should see the env var
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "$env.FORK_VAR".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("visible"),
        "fork_eval should see session env: {:?}",
        result.stdout
    );

    // fork_eval should see the custom command
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "fork-greet".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("hello from fork"),
        "fork_eval should see custom commands: {:?}",
        result.stdout
    );

    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "$fork_let; $fork_mut".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("12") && result.stdout.contains("21"),
        "fork_eval should see persistent variables: {:?}",
        result.stdout
    );

    // fork_eval should see the cwd
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "pwd".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("sub"),
        "fork_eval should see cwd: {:?}",
        result.stdout
    );

    // Verify the real runtime was NOT mutated: snapshot should match pre-fork state
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("FORK_VAR").map(String::as_str),
        Some("visible"),
        "original env should be intact"
    );
    assert_eq!(
        snapshot.current_cwd, nested,
        "original cwd should be intact"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_fork_eval_rejects_state_mutations() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // cd should be rejected in fork mode
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "cd /tmp".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "cd should fail in fork_eval: {:?}",
        result.stderr
    );

    // def should be rejected
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "def nope [] { 1 }".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "def should fail in fork_eval: {:?}",
        result.stderr
    );

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "let nope = 1".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "mut nope = 1".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "$env.PATH = 'x'".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    // alias should be rejected
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "alias nope = echo no".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "alias should fail in fork_eval: {:?}",
        result.stderr
    );

    // load-env should be rejected
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "load-env { X: 1 }".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "load-env should fail in fork_eval: {:?}",
        result.stderr
    );

    // hide-env should be rejected
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "hide-env PATH".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "hide-env should fail in fork_eval: {:?}",
        result.stderr
    );

    // all overlay subcommands should be rejected in fork_eval
    for form in &[
        "overlay use foo",
        "overlay hide foo",
        "overlay new myoverlay",
        "overlay list",
    ] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "{form:?} should fail in fork_eval: {:?}",
            result.stderr
        );
        assert!(
            result
                .aggregated_output
                .contains("does not support overlay commands"),
            "{form:?} fork_eval rejection should be explicit: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_fork_eval_returns_none_for_posix_session()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager.fork_eval_in_managed_nu_session(
        session_id.clone(),
        "echo test".to_string(),
        None,
    )?;
    assert!(
        result.is_none(),
        "fork_eval should return None for POSIX sessions"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}
