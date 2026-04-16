/// Integration tests for OSC 133 command-completion detection.
///
/// All tests here use bash with PROMPT_COMMAND configured to emit the D (CommandFinished)
/// marker after every command.  That makes `exec_command_in_shell_session` work correctly
/// because the D marker is what triggers the completion signal.  Tests are skipped when
/// bash is not available on the host.
use super::*;
use std::time::Duration;

fn bash_path() -> Option<String> {
    ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
        .iter()
        .find(|p| std::path::Path::new(*p).exists())
        .map(|s| s.to_string())
}

fn has_osc_sequences(s: &str) -> bool {
    s.contains("\x1b]")
}

/// Create a bash session pre-configured to emit OSC 133 markers.
///
/// `create_persistent_shell_session` injects `PROMPT_COMMAND` and `PS1` into the
/// bash environment before the process starts (see session.rs).  Bash reads them on
/// startup, so every prompt already emits A, B, and D markers — no further setup
/// command is needed.
async fn create_osc133_bash_session(
    cwd: &std::path::Path,
) -> Result<(SessionManager, colossal_linux_sandbox::types::SessionId), Box<dyn std::error::Error>>
{
    let bash = bash_path().ok_or("bash not found")?;
    create_shell_session(cwd, bash, SandboxPolicy::DangerFullAccess).await
}

// ── exec_command_in_shell_session ─────────────────────────────────────────────

#[tokio::test]
async fn exec_completes_with_exit_code_zero() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let result = manager
        .exec_command_in_shell_session(session_id.clone(), "true".into(), Some(5_000), 1_000, None)
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_captures_nonzero_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let result = manager
        .exec_command_in_shell_session(session_id.clone(), "false".into(), Some(5_000), 1_000, None)
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 1 });
    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_captures_specific_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "bash -c 'exit 42'".into(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 42 });
    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_output_contains_no_osc_escape_sequences() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo clean_output".into(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        result.stdout.contains("clean_output"),
        "expected 'clean_output' in stdout: {:?}",
        result.stdout
    );
    assert!(
        !has_osc_sequences(&result.stdout),
        "OSC sequences leaked into stdout: {:?}",
        result.stdout
    );
    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_sequential_commands_all_complete() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    for i in 0..5u32 {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                format!("echo 'iter_{i}'"),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert_eq!(
            result.exit_status,
            ExitStatus::Completed { code: 0 },
            "iteration {i} wrong status"
        );
        assert!(
            result.stdout.contains(&format!("iter_{i}")),
            "iteration {i} output missing: {:?}",
            result.stdout
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_session_usable_after_failed_command() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let fail = manager
        .exec_command_in_shell_session(session_id.clone(), "false".into(), Some(5_000), 1_000, None)
        .await?;
    assert_eq!(fail.exit_status, ExitStatus::Completed { code: 1 });

    let ok = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            "echo recovered".into(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;
    assert_eq!(ok.exit_status, ExitStatus::Completed { code: 0 });
    assert!(
        ok.stdout.contains("recovered"),
        "expected 'recovered': {:?}",
        ok.stdout
    );

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn exec_multiline_output_fully_captured() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let result = manager
        .exec_command_in_shell_session(
            session_id.clone(),
            r#"printf 'alpha\nbeta\ngamma\n'"#.into(),
            Some(5_000),
            1_000,
            None,
        )
        .await?;

    assert_eq!(result.exit_status, ExitStatus::Completed { code: 0 });
    for line in &["alpha", "beta", "gamma"] {
        assert!(
            result.stdout.contains(line),
            "missing '{line}' in output: {:?}",
            result.stdout
        );
    }
    manager.terminate_session(session_id).await?;
    Ok(())
}

/// The D marker carries the real exit code — verify it survives the full pipeline.
#[tokio::test]
async fn exec_d_marker_exit_code_survives_full_pipeline() -> Result<(), Box<dyn std::error::Error>>
{
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    // Run several commands with different exit codes and verify each one is captured.
    let cases: &[(&str, i32)] = &[
        ("true", 0),
        ("false", 1),
        ("bash -c 'exit 7'", 7),
        ("bash -c 'exit 127'", 127),
        ("true", 0), // confirm state resets after a non-zero exit
    ];

    for (cmd, expected_code) in cases {
        let result = manager
            .exec_command_in_shell_session(
                session_id.clone(),
                cmd.to_string(),
                Some(5_000),
                1_000,
                None,
            )
            .await?;
        assert_eq!(
            result.exit_status,
            ExitStatus::Completed {
                code: *expected_code
            },
            "command '{cmd}' exit code wrong"
        );
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

// ── send_command_to_shell_session streaming ───────────────────────────────────

#[tokio::test]
async fn streaming_command_emits_stdout_then_exit() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let stream = manager
        .send_command_to_shell_session(session_id.clone(), "echo streaming_test".into())
        .await?;

    let mut got_output = false;
    let mut got_exit = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        match tokio::time::timeout_at(deadline, stream.recv()).await {
            Ok(Ok(StreamEvent::Stdout(s))) => {
                if s.contains("streaming_test") {
                    got_output = true;
                }
            }
            Ok(Ok(StreamEvent::Exit(_))) => {
                got_exit = true;
                break;
            }
            Ok(Ok(StreamEvent::Stderr(_))) => {}
            Ok(Ok(StreamEvent::Error(e))) => return Err(format!("stream error: {e}").into()),
            Ok(Err(_)) => break,
            Err(_) => return Err("stream timed out".into()),
        }
    }

    assert!(got_output, "expected stdout containing 'streaming_test'");
    assert!(got_exit, "expected Exit event");
    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn streaming_stdout_contains_no_osc_sequences() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    let stream = manager
        .send_command_to_shell_session(session_id.clone(), "echo no_escape".into())
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, stream.recv()).await {
            Ok(Ok(StreamEvent::Stdout(s))) => {
                assert!(
                    !has_osc_sequences(&s),
                    "OSC sequences in streaming stdout: {:?}",
                    s
                );
            }
            Ok(Ok(StreamEvent::Exit(_))) | Ok(Err(_)) => break,
            Ok(Ok(_)) => {}
            Err(_) => return Err("stream timed out".into()),
        }
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}

#[tokio::test]
async fn streaming_sequential_commands_all_exit() -> Result<(), Box<dyn std::error::Error>> {
    if bash_path().is_none() {
        eprintln!("skipping: bash not found");
        return Ok(());
    }
    let _guard = shell_test_lock();
    let temp = tempfile::tempdir()?;
    let (manager, session_id) = create_osc133_bash_session(temp.path()).await?;

    for i in 0..3u32 {
        let stream = manager
            .send_command_to_shell_session(session_id.clone(), format!("echo 'stream_{i}'"))
            .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, stream.recv()).await {
                Ok(Ok(StreamEvent::Exit(_))) | Ok(Err(_)) => break,
                Ok(Ok(_)) => {}
                Err(_) => return Err(format!("stream {i} timed out").into()),
            }
        }
    }

    manager.terminate_session(session_id).await?;
    Ok(())
}
