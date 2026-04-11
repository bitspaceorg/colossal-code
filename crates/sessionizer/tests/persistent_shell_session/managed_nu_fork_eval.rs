use super::*;

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

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "$env.FORK_VAR".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(result.stdout.contains("visible"), "{:?}", result.stdout);

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

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "pwd".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(result.stdout.contains("sub"), "{:?}", result.stdout);

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("FORK_VAR").map(String::as_str),
        Some("visible")
    );
    assert_eq!(snapshot.current_cwd, nested);

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

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "cd /tmp".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "def nope [] { 1 }".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

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

    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "alias nope = echo no".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "load-env { X: 1 }".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "hide-env PATH".to_string(), None)?
        .expect("session is managed nu");
    assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });

    for form in &[
        "overlay use foo",
        "overlay hide foo",
        "overlay new myoverlay",
        "overlay list",
    ] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(result.exit_status, ExitStatus::Completed { code: 0 });
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
