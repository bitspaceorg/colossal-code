use super::*;

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

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.nu_aliases.is_empty(),
        "snapshot should have aliases"
    );
    assert!(
        snapshot.nu_aliases.iter().any(|a| a.name == "say_hi"),
        "snapshot should contain the say_hi alias"
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
        "alias of def should work: stdout={:?}",
        result.stdout
    );

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
        "alias of def should survive restore: stdout={:?}",
        result.stdout
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}
