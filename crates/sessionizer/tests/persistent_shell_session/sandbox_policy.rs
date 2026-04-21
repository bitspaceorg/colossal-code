use super::*;

fn zsh_path() -> Option<String> {
    let path = std::path::Path::new("/bin/zsh");
    path.exists().then(|| path.to_string_lossy().to_string())
}

// ── PTY (bash/sh) ────────────────────────────────────────────────────────────

#[tokio::test]
async fn pty_workspace_write_allows_write() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("out.txt");

    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("printf 'written' > {}", target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(std::fs::read_to_string(&target)?, "written");
    Ok(())
}

#[tokio::test]
async fn pty_readonly_blocks_write() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("blocked.txt");

    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::ReadOnly,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("printf 'blocked' > {}", target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "write should have been blocked"
    );
    assert!(!target.exists(), "file should not exist");
    Ok(())
}

#[tokio::test]
async fn pty_workspace_write_allows_read() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "readable")?;

    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        workspace_write_policy(temp.path()),
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cat {}", source.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("readable"),
        "got: {:?}",
        result.stdout
    );
    Ok(())
}

#[tokio::test]
async fn pty_readonly_allows_read() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "readable")?;

    let (manager, session_id) = create_shell_session(
        temp.path(),
        deterministic_shell_path(),
        SandboxPolicy::ReadOnly,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cat {}", source.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("readable"),
        "got: {:?}",
        result.stdout
    );
    Ok(())
}

#[tokio::test]
async fn pty_zsh_foreground_command_completes() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(zsh) = zsh_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;

    let (manager, session_id) =
        create_shell_session(temp.path(), zsh, workspace_write_policy(temp.path())).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "printf 'zsh-ok'".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(result.stdout.contains("zsh-ok"), "got: {:?}", result.stdout);
    Ok(())
}

#[tokio::test]
async fn pty_zsh_command_completes_after_approval_retry() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(zsh) = zsh_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("approved.txt");

    let (manager, session_id) =
        create_shell_session(temp.path(), zsh, workspace_write_policy(temp.path())).await?;

    let denied = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("printf 'approved' > {}", target.display()),
            Some(5_000),
            1_000,
            Some(colossal_linux_sandbox::safety::AskForApproval::OnRequest),
        )
        .await;

    assert!(denied.is_err(), "command should require approval");

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("printf 'approved' > {}", target.display()),
            Some(5_000),
            1_000,
            Some(colossal_linux_sandbox::safety::AskForApproval::Never),
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(std::fs::read_to_string(&target)?, "approved");
    Ok(())
}

// ── Nu ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn nu_workspace_write_allows_write() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("out.txt");

    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, workspace_write_policy(temp.path())).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("\"written\" | save {}", target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        std::fs::read_to_string(&target)?.contains("written"),
        "file should contain written content"
    );
    Ok(())
}

#[tokio::test]
async fn nu_readonly_blocks_write() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("blocked.txt");

    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::ReadOnly).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("\"blocked\" | save {}", target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_ne!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "write should have been blocked"
    );
    assert!(!target.exists(), "file should not exist");
    Ok(())
}

#[tokio::test]
async fn nu_workspace_write_allows_read() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "readable")?;

    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, workspace_write_policy(temp.path())).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("open {}", source.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("readable"),
        "got: {:?}",
        result.stdout
    );
    Ok(())
}

#[tokio::test]
async fn nu_readonly_allows_read() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "readable")?;

    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::ReadOnly).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("open {}", source.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    manager.terminate_session(session_id).await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("readable"),
        "got: {:?}",
        result.stdout
    );
    Ok(())
}
