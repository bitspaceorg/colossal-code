use super::*;

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
        "mut variable should work within session: stdout={:?}",
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
