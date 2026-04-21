use super::*;

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
async fn managed_nu_hide_env_removes_env_var() -> Result<(), Box<dyn std::error::Error>> {
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
async fn managed_nu_structured_env_value_survives_snapshot_restore()
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
            "load-env { META: { nested: 7, label: 'ok' } }".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id).await?;

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

    manager
        .restore_shell_session_from_snapshot(restored_id.clone(), &snapshot)
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$env.META.nested; $env.META.label".to_string(),
            Some(10_000),
            10_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(result.stdout.contains('7'), "{:?}", result.stdout);
    assert!(result.stdout.contains("ok"), "{:?}", result.stdout);

    manager.terminate_session(restored_id).await?;
    Ok(())
}

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
async fn managed_nu_hide_env_pwd_is_allowed_but_cwd_stays_tracked()
-> Result<(), Box<dyn std::error::Error>> {
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
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });

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
        "managed cwd tracking should survive hide-env PWD: {:?}",
        pwd_result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.env_vars.contains_key("PWD"),
        "hide-env PWD should be persisted through the replay journal"
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

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("SPECIAL").map(String::as_str),
        Some(special_value),
        "special chars should survive snapshot"
    );
    manager.terminate_session(session_id.clone()).await?;

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
