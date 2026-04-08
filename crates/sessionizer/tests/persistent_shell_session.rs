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
