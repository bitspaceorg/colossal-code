use colossal_linux_sandbox::manager::SessionManager;
use colossal_linux_sandbox::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
use colossal_linux_sandbox::shell;
use colossal_linux_sandbox::types::{ExecCommandParams, StreamEvent};

async fn collect_until_timeout_event(
    stream: async_channel::Receiver<StreamEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = stream;
    for _ in 0..8 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.recv())
            .await?
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        match event {
            StreamEvent::Stdout(_) | StreamEvent::Stderr(_) => continue,
            StreamEvent::Error(error) => {
                assert_eq!(error, "Command timed out");
                return Ok(());
            }
            StreamEvent::Exit(code) => {
                return Err(format!("expected timeout error, got exit code {code}").into());
            }
        }
    }

    Err("timed out waiting for stream timeout event".into())
}

fn workspace_write_policy(cwd: &std::path::Path) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![WritableRoot {
            root: cwd.to_path_buf(),
            recursive: true,
            read_only_subpaths: vec![],
        }],
        network_access: NetworkAccess::Restricted,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    }
}

#[tokio::test]
async fn stream_exec_command_enhanced_emits_timeout_error() -> Result<(), Box<dyn std::error::Error>>
{
    let manager = SessionManager::default();
    let cwd = tempfile::tempdir()?;
    let shell = shell::default_user_shell().await;
    let params = ExecCommandParams {
        command: vec!["bash".into(), "-c".into(), "sleep 2".into()],
        shell,
        cwd: cwd.path().to_path_buf(),
        env: Default::default(),
        timeout_ms: Some(100),
        max_output_tokens: 1000,
        sandbox_policy: workspace_write_policy(cwd.path()),
        is_background: false,
        ask_for_approval: None,
    };

    let (_session_id, stream) = manager.stream_exec_command_enhanced(params).await?;
    collect_until_timeout_event(stream).await?;
    Ok(())
}

#[tokio::test]
async fn stream_exec_command_emits_timeout_error() -> Result<(), Box<dyn std::error::Error>> {
    let manager = SessionManager::default();
    let cwd = tempfile::tempdir()?;
    let shell = shell::default_user_shell().await;
    let params = ExecCommandParams {
        command: vec!["bash".into(), "-c".into(), "sleep 2".into()],
        shell,
        cwd: cwd.path().to_path_buf(),
        env: Default::default(),
        timeout_ms: Some(100),
        max_output_tokens: 1000,
        sandbox_policy: workspace_write_policy(cwd.path()),
        is_background: false,
        ask_for_approval: None,
    };

    let (_session_id, stream) = manager.stream_exec_command(params).await?;
    collect_until_timeout_event(stream).await?;
    Ok(())
}
