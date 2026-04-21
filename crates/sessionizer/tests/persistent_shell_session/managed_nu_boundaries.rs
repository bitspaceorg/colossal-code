use super::*;

#[tokio::test]
async fn managed_nu_nu_builtins_work_in_embedded_runtime() -> Result<(), Box<dyn std::error::Error>>
{
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

/// External commands are SUPPORTED in managed Nu — `^cmd` and `run-external`
/// invoke the `ManagedExternalCommand` implementation, which spawns a real
/// child process.  The sandbox policy (not managed Nu) decides what is allowed.
#[tokio::test]
async fn managed_nu_external_commands_work_when_allowed() -> Result<(), Box<dyn std::error::Error>>
{
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

    // `^echo` via the caret sigil.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "^echo external-test".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "^echo should succeed in managed Nu: {:?}",
        result.aggregated_output
    );
    assert!(
        result.stdout.contains("external-test"),
        "^echo output should contain the argument: {:?}",
        result.stdout
    );

    // `run-external echo` via explicit form.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "run-external echo hello-from-run-external".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "run-external should succeed in managed Nu: {:?}",
        result.aggregated_output
    );
    assert!(
        result.stdout.contains("hello-from-run-external"),
        "run-external output should contain the argument: {:?}",
        result.stdout
    );

    // Running an external command must not corrupt snapshot state.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "external command must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_external_command_env_visible_to_child() -> Result<(), Box<dyn std::error::Error>>
{
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

    // Set an env var via managed Nu, then verify a child process can see it.
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { MY_MANAGED_VAR: \"hello-child\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "^sh -c 'echo $MY_MANAGED_VAR'".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "external command with env should succeed: {:?}",
        result.aggregated_output
    );
    assert!(
        result.stdout.contains("hello-child"),
        "child process must see env vars set in managed Nu: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_external_command_rotation_preserves_shell_state()
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

    // Set env, run external, then check snapshot carries env (not process state).
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { BEFORE_EXTERNAL: \"state-ok\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "^echo ran-external".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert_eq!(
        snapshot.env_vars.get("BEFORE_EXTERNAL").map(|s| s.as_str()),
        Some("state-ok"),
        "env set before external command must be in snapshot: {:?}",
        snapshot.env_vars
    );
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "external command must not add to nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_overlay_module_and_source_commands_work()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let sourced = temp.path().join("sourced.nu");
    std::fs::write(&sourced, "def sourced-greet [] { 'hello-from-source' }")?;
    let (manager, session_id) = create_shell_session(
        temp.path(),
        nu_path.clone(),
        SandboxPolicy::DangerFullAccess,
    )
    .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!(
                "module greetings {{ export def greet [] {{ 'hello-inline' }} }}; overlay use greetings; source {}; [(greet) (sourced-greet) (overlay list | length)] | to json -r",
                sourced.display()
            ),
            Some(10_000),
            10_000,
            None,
        )
        .await?;

    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert!(
        result.stdout.contains("hello-inline"),
        "{:?}",
        result.stdout
    );
    assert!(
        result.stdout.contains("hello-from-source"),
        "{:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        !snapshot.nu_replay_commands.is_empty(),
        "overlay/module/source commands should be captured in the replay journal: {:?}",
        snapshot.nu_replay_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_silent_command_completes_without_hanging()
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
            "load-env { SILENT_CHECK: 'ok' }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.trim().is_empty(),
        "silent managed-nu command produced unexpected output: {:?}",
        result.stdout
    );

    let check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.SILENT_CHECK".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(check.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(check.stdout.trim(), "ok");

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_blocking_external_command_times_out_and_retires_session()
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
            "^sh -c 'sleep 2'".to_string(),
            Some(100),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Timeout);
    assert!(
        manager.get_session_info(session_id.clone()).is_none(),
        "timed-out managed-nu session should be retired to avoid future hangs"
    );

    let replacement =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;
    let follow_up = replacement
        .0
        .exec_command_in_shell_session(
            replacement.1.clone(),
            "\"recovered\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(follow_up.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(follow_up.stdout.trim(), "recovered");

    replacement.0.terminate_session(replacement.1).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_save_builtin_writes_file() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let output_path = temp.path().join("test.txt");
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("'test' | save -f {}", output_path.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "{result:?}"
    );
    assert_eq!(std::fs::read_to_string(&output_path)?, "test");

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_filesystem_builtins_work_together() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let original = temp.path().join("original.txt");
    let renamed = temp.path().join("renamed.txt");
    let subdir = temp.path().join("subdir");
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let mkdir_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("mkdir {}", subdir.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(mkdir_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(subdir.is_dir());

    let save_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("'alpha' | save -f {}", original.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(save_result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(std::fs::read_to_string(&original)?, "alpha");

    let open_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("open {}", original.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(open_result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(open_result.stdout.trim(), "alpha");

    let ls_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "ls | get name | sort | str join ','".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(ls_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        ls_result.stdout.contains("original.txt"),
        "{:?}",
        ls_result.stdout
    );
    assert!(
        ls_result.stdout.contains("subdir"),
        "{:?}",
        ls_result.stdout
    );

    let cp_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("cp {} {}", original.display(), renamed.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(cp_result.exit_status, ExitStatus::Completed { code: 0 });
    assert_eq!(std::fs::read_to_string(&renamed)?, "alpha");

    let mv_target = subdir.join("moved.txt");
    let mv_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("mv {} {}", renamed.display(), mv_target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(mv_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(!renamed.exists());
    assert_eq!(std::fs::read_to_string(&mv_target)?, "alpha");

    let rm_result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            format!("rm {}", mv_target.display()),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(rm_result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(!mv_target.exists());

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_partial_output_preserved_on_error() -> Result<(), Box<dyn std::error::Error>> {
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
    assert!(
        result.stdout.contains("hello"),
        "partial output from successful segment should be in stdout: {:?}",
        result.stdout
    );
    assert!(
        !result.stderr.is_empty(),
        "error message should be in stderr"
    );
    assert!(
        result.aggregated_output.contains("hello")
            && result.aggregated_output.contains("Module not found"),
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

/// External commands in `fork_eval` execute normally — they run against the
/// forked (isolated) state, and process side effects are discarded with the fork.
/// The live session is untouched.
#[tokio::test]
async fn managed_nu_fork_eval_external_commands_work() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &["^echo hello", "run-external echo hello"] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None, None)
            .await?
            .expect("session is managed nu");
        assert_eq!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{form}' should work in fork_eval: {:?}",
            result.aggregated_output
        );
        assert!(
            result.stdout.contains("hello"),
            "'{form}' fork_eval output must contain the argument: {:?}",
            result.stdout
        );
    }

    // fork_eval external commands must not corrupt the live session's snapshot.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "fork_eval external must not affect live nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}
