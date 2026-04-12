use super::*;

#[tokio::test]
async fn managed_nu_config_mutations_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
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
            "$env.config.show_banner = false".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "$env.config sub-field mutation should be rejected: {:?}",
        result.exit_status
    );
    assert!(
        result.aggregated_output.contains("$env.config"),
        "rejection message should mention $env.config: {:?}",
        result.aggregated_output
    );

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config = {}".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "whole $env.config replacement should be rejected: {:?}",
        result.exit_status
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "config rejection must not corrupt state: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        !snapshot.env_vars.contains_key("config"),
        "$env.config must not appear as a string env var: {:?}",
        snapshot.env_vars.keys().collect::<Vec<_>>()
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_read_not_intercepted_by_rejection()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &[
        "$env.config",
        "$env.config.show_banner == false",
        "let x = $env.config",
    ] {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                form.to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert!(
            !result
                .aggregated_output
                .contains("does not support $env.config mutations"),
            "read-only '{form}' must not trigger config mutation rejection: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_mutation_rejected_in_fork_eval() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    for form in &[
        "$env.config.show_banner = false",
        "$env.config = {}",
        "$env.config.table.mode = \"light\"",
    ] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{form}' should be rejected in fork_eval: {:?}",
            result.aggregated_output
        );
        assert!(
            result.aggregated_output.contains("$env.config"),
            "'{form}' fork_eval rejection must mention $env.config: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_module_commands_fail_predictably() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let unsupported_commands = [
        "module foo { export def bar [] { 1 } }",
        "use std",
        "source nonexistent.nu",
        "source-env nonexistent.nu",
        "export use std",
        "export module mymod { }",
        "export extern my-ext []",
        "export const MY_CONST = 42",
    ];

    for cmd in &unsupported_commands {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                cmd.to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert!(
            matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
            "'{cmd}' should exit with code 1, got {:?}",
            result.exit_status
        );
        assert!(
            result.aggregated_output.contains("module/use/source"),
            "rejection for '{cmd}' should mention module/use/source: {:?}",
            result.aggregated_output
        );
    }

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "failed module commands must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );
    assert!(
        snapshot.nu_aliases.is_empty(),
        "failed module commands must not corrupt nu_aliases: {:?}",
        snapshot.nu_aliases
    );
    assert!(
        snapshot.nu_variables.is_empty(),
        "failed module commands must not corrupt nu_variables: {:?}",
        snapshot.nu_variables
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_module_command_atomicity_blocks_following_segment()
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
            "module bad {}; def should_not_exist [] { 99 }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        matches!(result.exit_status, ExitStatus::Completed { code: 1 }),
        "module + def must fail: {:?}",
        result.exit_status
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "def after a module command must not be registered: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_export_def_persists_and_survives_snapshot()
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
            "export def greet-exported [] { \"exported hello\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "greet-exported".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("exported hello"),
        "export def command should be callable: {:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot
            .nu_custom_commands
            .iter()
            .any(|c| c.contains("greet-exported")),
        "export def should be in snapshot custom_commands"
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

    let result = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "greet-exported".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("exported hello"),
        "export def should survive snapshot/restore: {:?}",
        result.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_export_alias_persists_and_survives_snapshot()
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
            "def base-cmd [] { \"alias base\" }; export alias ea = base-cmd".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "ea".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("alias base"),
        "export alias should be callable: {:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_aliases.iter().any(|a| a.name == "ea"),
        "export alias should be in snapshot aliases"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_export_def_redefinition_replaces_previous()
-> Result<(), Box<dyn std::error::Error>> {
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
            "export def redef [] { \"v1\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "export def redef [] { \"v2\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "redef".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        result.stdout.contains("v2"),
        "export def redefinition should use latest: {:?}",
        result.stdout
    );

    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    let matching = snapshot
        .nu_custom_commands
        .iter()
        .filter(|c| c.contains("redef"))
        .count();
    assert_eq!(
        matching, 1,
        "redefined export def should be stored once, not duplicated"
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_fork_eval_rejects_module_commands() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    let unsupported_commands = [
        "module foo { export def bar [] { 1 } }",
        "use std",
        "source nonexistent.nu",
        "source-env nonexistent.nu",
        "export use std",
        "export module mymod { }",
        "export extern my-ext []",
        "export const MY_CONST = 42",
    ];

    for cmd in &unsupported_commands {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), cmd.to_string(), None)?
            .expect("session is managed nu");
        assert_ne!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{cmd}' should fail in fork_eval: {:?}",
            result.stderr
        );
        assert!(
            result.aggregated_output.contains("module/use/source"),
            "'{cmd}' fork_eval rejection should be explicit: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}
