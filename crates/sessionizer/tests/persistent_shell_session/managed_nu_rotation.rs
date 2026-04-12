use super::*;

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

    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cd \"{}\"", subdir.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

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
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "alias a10 = add-ten".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "hide-env E2".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    manager.terminate_session(session_id.clone()).await?;

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

#[tokio::test]
async fn managed_nu_restore_shell_session_from_snapshot_fails_on_invalid_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(shell_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let manager = SessionManager::default();
    let shared_state = Arc::new(SharedSessionState::new(temp.path().to_path_buf()));
    let restored_id = manager
        .create_persistent_shell_session(
            shell_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    let snapshot = colossal_linux_sandbox::manager::PersistentSessionState {
        version: colossal_linux_sandbox::manager::SNAPSHOT_FORMAT_VERSION,
        session_id: restored_id.clone(),
        shell_path,
        initial_cwd: temp.path().to_path_buf(),
        env_vars: std::collections::HashMap::new(),
        current_cwd: temp.path().to_path_buf(),
        created_at: std::time::SystemTime::now(),
        structured_env_json: None,
        nu_aliases: Vec::new(),
        nu_custom_commands: vec!["def broken [x] { $x + }".to_string()],
        nu_variables: Vec::new(),
        replay_commands: Vec::new(),
    };

    let err = manager
        .restore_shell_session_from_snapshot(restored_id.clone(), &snapshot)
        .await
        .expect_err("invalid managed nu restore should fail");
    assert!(
        err.to_string().contains("Incomplete math expression")
            || err.to_string().contains("parse")
            || err.to_string().contains("nu::parser"),
        "unexpected restore error: {err}"
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}

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

    let fork_result = manager
        .fork_eval_in_managed_nu_session(restored_id.clone(), "square 7".to_string(), None)?
        .expect("should be Some for managed Nu");
    assert!(
        fork_result.stdout.contains("49"),
        "fork_eval should see restored def: {:?}",
        fork_result.stdout
    );

    let fork_result = manager
        .fork_eval_in_managed_nu_session(restored_id.clone(), "$env.MY_KEY".to_string(), None)?
        .expect("should be Some for managed Nu");
    assert!(
        fork_result.stdout.contains("rotation_val"),
        "fork_eval should see restored env: {:?}",
        fork_result.stdout
    );

    manager.terminate_session(restored_id).await?;
    Ok(())
}
