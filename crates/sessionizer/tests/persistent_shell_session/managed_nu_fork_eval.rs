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

/// Verify fork_eval's isolation contract:
/// - stateful Nushell commands run successfully inside the fork
/// - their effects are visible later in the same forked command
/// - none of those effects leak back into the live managed-Nu session
#[tokio::test]
async fn managed_nu_fork_eval_allows_stateful_commands_without_persisting_them()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let sourced = temp.path().join("sourced.nu");
    std::fs::write(&sourced, "def sourced-greet [] { 'hello-from-source' }")?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let fork_dir = temp.path().join("fork-dir");
    std::fs::create_dir_all(&fork_dir)?;

    // cd is allowed in the fork and affects later segments in the same command.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            format!("cd {}; pwd", fork_dir.display()),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert_eq!(result.stdout.trim(), fork_dir.display().to_string());

    // defs work in the fork and can be called later in the same command.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "def nope [] { 1 }; nope".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(result.stdout.contains('1'));

    // let/mut are also allowed in the fork.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "let nope = 1; mut counter = 2; $counter = 3; [$nope $counter] | to json -r"
                .to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(result.stdout.contains("[1,3]"), "{:?}", result.stdout);

    // alias works in the fork.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "alias nope = echo no; nope".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(result.stdout.contains("no"));

    // load-env / hide-env affect only the fork.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "load-env { X: 1 }; hide-env PATH; [$env.X ($env.PATH? | default 'missing')] | to json -r"
                .to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(
        result.stdout.contains("[1,\"missing\"]"),
        "{:?}",
        result.stdout
    );

    // $env.* assignments remain allowed in the fork.
    let result = manager
        .fork_eval_in_managed_nu_session(session_id.clone(), "$env.PATH = 'x'".to_string(), None)?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{:?}",
        result.aggregated_output
    );

    // module/use/source/overlay commands should also run in the fork.
    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            format!(
                "module inline {{ export def greet [] {{ 'hi-inline' }} }}; use inline greet; source {}; sourced-greet; greet",
                sourced.display()
            ),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(
        result.stdout.contains("hello-from-source"),
        "{:?}",
        result.stdout
    );
    assert!(result.stdout.contains("hi-inline"), "{:?}", result.stdout);

    let result = manager
        .fork_eval_in_managed_nu_session(
            session_id.clone(),
            "overlay new fork_overlay; overlay list | length".to_string(),
            None,
        )?
        .expect("session is managed nu");
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );

    // None of the fork-local changes leak into the live session.
    let live_snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_ne!(
        live_snapshot.env_vars.get("PATH").map(String::as_str),
        Some("x"),
        "fork_eval $env.PATH mutation must not affect live session"
    );

    let live_check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "pwd; $env.X? | default 'none'; which nope | length; which sourced-greet | length"
                .to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(live_check.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        live_check.stdout.contains("none"),
        "{:?}",
        live_check.stdout
    );
    assert!(live_check.stdout.contains('0'), "{:?}", live_check.stdout);
    assert!(
        !live_check.stdout.contains("fork-dir"),
        "{:?}",
        live_check.stdout
    );

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
