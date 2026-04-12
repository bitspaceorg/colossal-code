use super::*;

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

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "cd nested".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

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

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(snapshot.current_cwd, nested);
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

    let home = std::env::var("HOME").unwrap_or_default();
    let test_dir = std::path::PathBuf::from(&home);
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
