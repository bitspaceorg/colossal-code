use super::*;

#[tokio::test]
async fn managed_nu_nu_builtins_work_in_embedded_runtime() -> Result<(), Box<dyn std::error::Error>>
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
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

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
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

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
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

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
async fn managed_nu_overlay_commands_fail_predictably() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &[
        "overlay use foo",
        "overlay hide foo",
        "overlay new myoverlay",
        "overlay list",
    ] {
        let failed = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                form.to_string(),
                Some(10_000),
                10_000,
                None,
            )
            .await?;

        assert!(
            matches!(failed.exit_status, ExitStatus::Completed { code: 1 }),
            "{form:?} should exit with code 1, got {:?}",
            failed.exit_status
        );
        assert!(
            failed
                .aggregated_output
                .contains("does not support overlay commands"),
            "{form:?} failure should be explicit: {:?}",
            failed.aggregated_output
        );
    }

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "failed overlay commands must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        snapshot.nu_aliases.is_empty(),
        "failed overlay commands must not corrupt nu_aliases: {:?}",
        snapshot.nu_aliases
    );
    assert!(
        snapshot.nu_variables.is_empty(),
        "failed overlay commands must not corrupt nu_variables: {:?}",
        snapshot.nu_variables
    );

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
        result.aggregated_output.contains("hello") && result.aggregated_output.contains("overlay"),
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
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
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
