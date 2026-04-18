use super::*;

// ---------------------------------------------------------------------------
// Config mutation — live exec
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_mutation_works_live() -> Result<(), Box<dyn std::error::Error>> {
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

    // Sub-field mutation must succeed.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = false".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "$env.config sub-field mutation should succeed: {:?}",
        result.aggregated_output
    );

    // The mutated value should be readable in the same session.
    let read = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        read.exit_status,
        ExitStatus::Completed { code: 0 },
        "reading config after mutation should succeed: {:?}",
        read.aggregated_output
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_whole_replacement_works() -> Result<(), Box<dyn std::error::Error>> {
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

    // Whole-config replacement: pass an empty record (valid minimal config).
    // Nu will merge it with defaults, which is correct behavior.
    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config = {}".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        result.exit_status,
        ExitStatus::Completed { code: 0 },
        "whole $env.config replacement should succeed: {:?}",
        result.aggregated_output
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config mutation — snapshot/restore persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_persists_across_snapshot_restore()
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

    // Mutate config in the original session.
    let mutate = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = false".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        mutate.exit_status,
        ExitStatus::Completed { code: 0 },
        "config mutation must succeed: {:?}",
        mutate.aggregated_output
    );

    // Capture snapshot — nu_config must be serialized.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_config.is_some(),
        "snapshot must carry nu_config after $env.config mutation: {:?}",
        snapshot.nu_config
    );

    // Create a fresh session that has NOT had any config mutation applied.
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let restored_id = manager
        .create_persistent_shell_session(
            nu_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    // Restore via the real path — ManagedNuRuntime::restore() deserializes
    // nu_config from the snapshot JSON and sets stack.config directly.
    // No manual $env.config mutation is applied here.
    manager
        .restore_shell_session_from_snapshot(restored_id.clone(), &snapshot)
        .await?;

    // The restored session must see the config value that was set in the
    // original session, proving it came through the restore path.
    let read = manager
        .exec_command_in_shell_session(
            restored_id.clone(),
            "$env.config.show_banner".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        read.exit_status,
        ExitStatus::Completed { code: 0 },
        "config read after restore should succeed: {:?}",
        read.aggregated_output
    );
    assert!(
        read.stdout.trim() == "false",
        "restored session must see show_banner=false from snapshot, got: {:?}",
        read.stdout
    );

    manager.terminate_session(session_id).await?;
    manager.terminate_session(restored_id).await?;
    Ok(())
}

#[tokio::test]
async fn managed_nu_config_snapshot_field_absent_when_not_mutated()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // No config mutation — snapshot nu_config must be None.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_config.is_none(),
        "nu_config should be None when config was not mutated: {:?}",
        snapshot.nu_config
    );
    assert!(
        !snapshot.env_vars.contains_key("config"),
        "$env.config must not appear as a plain string env var: {:?}",
        snapshot.env_vars.keys().collect::<Vec<_>>()
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config mutation — fork_eval behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_mutation_allowed_in_fork_eval() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // Config mutations in fork_eval should succeed (they affect only the
    // discarded fork; the live session is unchanged).
    for form in &[
        "$env.config.show_banner = false",
        "$env.config = {}",
        "$env.config.table.mode = \"light\"",
    ] {
        let result = manager
            .fork_eval_in_managed_nu_session(session_id.clone(), form.to_string(), None, None)
            .await?
            .expect("session is managed nu");
        assert_eq!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "'{form}' should succeed in fork_eval: {:?}",
            result.aggregated_output
        );
    }

    // After fork_eval config mutations, the live session's config must be
    // unchanged (fork side effects are discarded).
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_config.is_none(),
        "fork_eval config mutations must not affect the live snapshot: {:?}",
        snapshot.nu_config
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config mutation — policy rotation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_persists_across_policy_rotation()
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

    // Mutate config in the pre-rotation session.
    let mutate = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = false".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        mutate.exit_status,
        ExitStatus::Completed { code: 0 },
        "pre-rotation config mutation must succeed: {:?}",
        mutate.aggregated_output
    );

    // Snapshot carries the config — this is what the rotation path uses.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_config.is_some(),
        "config must be in snapshot before rotation: {:?}",
        snapshot.nu_config
    );

    // Terminate the old session (simulating a policy-driven rotation that
    // tears down the old shell and brings up a new one).
    manager.terminate_session(session_id.clone()).await?;

    // Bring up the rotated session under a new (or same) policy — a fresh
    // runtime with no prior config mutations.
    let shared_state = Arc::new(SharedSessionState::new(snapshot.current_cwd.clone()));
    let rotated_id = manager
        .create_persistent_shell_session(
            nu_path.clone(),
            false,
            SandboxPolicy::DangerFullAccess,
            shared_state,
            Some(Duration::from_secs(30)),
        )
        .await?;

    // Drive the actual rotation restore path — restore_shell_session_from_snapshot
    // calls ManagedNuRuntime::restore() which deserializes nu_config from the
    // snapshot JSON and writes it into stack.config.
    // No manual $env.config mutation is applied here.
    manager
        .restore_shell_session_from_snapshot(rotated_id.clone(), &snapshot)
        .await?;

    // The rotated session must see the specific value set pre-rotation,
    // proving the config reached the new runtime through the restore path.
    let read = manager
        .exec_command_in_shell_session(
            rotated_id.clone(),
            "$env.config.show_banner".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(
        read.exit_status,
        ExitStatus::Completed { code: 0 },
        "config must be accessible after rotation: {:?}",
        read.aggregated_output
    );
    assert!(
        read.stdout.trim() == "false",
        "rotated session must see show_banner=false from pre-rotation snapshot, got: {:?}",
        read.stdout
    );

    manager.terminate_session(rotated_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config mutation — malformed / partial writes do not corrupt state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_malformed_mutation_does_not_corrupt_state()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = shell_test_lock();
    let Some(nu_path) = nushell_path() else {
        return Ok(());
    };

    let temp = tempfile::tempdir()?;
    let (manager, session_id) =
        create_shell_session(temp.path(), nu_path, SandboxPolicy::DangerFullAccess).await?;

    // First set a valid env var so we can verify it survives a bad config write.
    manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "load-env { BEFORE_BAD_CONFIG: \"ok\" }".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    // Attempt to assign a non-record type to $env.config.  Nu will either
    // reject this at parse/eval time or update_config will skip bad fields.
    // Either way, the session must not become unusable.
    let _result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.config.show_banner = \"not-a-bool\"".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await;
    // We don't assert success or failure here — Nu may accept this (and use
    // the default for that field) or reject it.  What matters is that the
    // runtime stays alive.

    // Session must still be functional.
    let check = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "$env.BEFORE_BAD_CONFIG".to_string(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert!(
        check.stdout.contains("ok"),
        "session must remain functional after bad config mutation: {:?}",
        check.stdout
    );

    // Snapshot must not be corrupted.
    let snapshot = manager.snapshot_shell_session(session_id.clone()).await?;
    assert!(
        snapshot.nu_custom_commands.is_empty(),
        "bad config mutation must not corrupt nu_custom_commands: {:?}",
        snapshot.nu_custom_commands
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Config reads are still allowed (regression guard from old rejection era)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn managed_nu_config_read_not_blocked() -> Result<(), Box<dyn std::error::Error>> {
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
        // None of these should produce an error about "does not support".
        assert!(
            !result.aggregated_output.contains("does not support"),
            "read-only '{form}' must not be rejected: {:?}",
            result.aggregated_output
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}
