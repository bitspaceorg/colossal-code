use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Child;

pub mod error;
pub mod hash_id;
#[cfg(target_os = "linux")]
pub mod landlock;
pub mod manager;
pub mod protocol;
pub mod pty;
pub mod safety;
pub mod search_results;
pub mod seatbelt;
pub mod semantic_search;
pub mod semantic_search_lib;
pub mod session;
pub mod shell;
pub mod spawn;
pub mod tools;
pub mod types;
pub mod utils;

/// Execute tools binary with the given sandbox policy applied
pub async fn execute_tools_with_sandbox(
    args: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
) -> Result<std::process::Output, ColossalErr> {
    crate::tools::execute_tools_with_sandbox(args, sandbox_policy, cwd).await
}

pub async fn spawn_sandboxed_command(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
    env: HashMap<String, String>,
) -> Result<Child, ColossalErr> {
    #[cfg(target_os = "macos")]
    {
        seatbelt::spawn_command_under_seatbelt(
            command,
            sandbox_policy,
            cwd,
            spawn::StdioPolicy::RedirectForShellTool,
            env,
        )
        .await
    }
    #[cfg(target_os = "linux")]
    {
        use crate::landlock::apply_sandbox_policy_to_current_thread;
        use std::process::Stdio;
        use tokio::process::Command;

        let shell = shell::default_user_shell().await;
        let formatted_command = shell
            .format_default_shell_invocation(command.clone(), false)
            .ok_or_else(|| {
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Failed to format command for shell",
                ))
            })?;

        let (program, args) = formatted_command.split_first().ok_or_else(|| {
            ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "command args are empty",
            ))
        })?;

        apply_sandbox_policy_to_current_thread(sandbox_policy, &cwd)?;

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&cwd)
            .envs(&env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());

        cmd.spawn().map_err(|e| ColossalErr::Io(e))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        spawn::spawn_child_async(
            PathBuf::from(command.get(0).ok_or_else(|| {
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Empty command",
                ))
            })?),
            command[1..].to_vec(),
            None,
            cwd,
            sandbox_policy,
            spawn::StdioPolicy::RedirectForShellTool,
            env,
        )
        .await
        .map_err(|e| ColossalErr::Io(e))
    }
}
