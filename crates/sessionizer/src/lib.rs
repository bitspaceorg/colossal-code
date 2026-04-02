use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Child;

pub mod error;
pub mod hash_id;
#[cfg(target_os = "linux")]
pub mod landlock;
#[cfg(target_os = "linux")]
pub mod linux_sandbox;
pub mod manager;
pub mod protocol;
pub mod pty;
pub mod safety;
pub mod sandboxing;
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
#[cfg(target_os = "windows")]
pub mod windows_sandbox;

/// Execute tools binary with the given sandbox policy applied
pub async fn execute_tools_with_sandbox(
    args: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
) -> Result<std::process::Output, ColossalErr> {
    crate::tools::execute_tools_with_sandbox(args, sandbox_policy, cwd).await
}

pub fn resolve_tools_binary_path() -> Result<PathBuf, ColossalErr> {
    crate::tools::resolve_tools_binary_path()
}

pub async fn spawn_sandboxed_command(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
    env: HashMap<String, String>,
) -> Result<Child, ColossalErr> {
    #[cfg(target_os = "macos")]
    {
        let program = PathBuf::from(command.first().ok_or_else(|| {
            ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty command",
            ))
        })?);
        let request = sandboxing::SandboxManager::new().prepare_spawn(
            sandboxing::SandboxCommand {
                program,
                args: command[1..].to_vec(),
                cwd: cwd.clone(),
                env,
            },
            sandbox_policy,
        )?;
        spawn::spawn_child_async(
            request.program,
            request.args,
            None,
            request.cwd,
            sandbox_policy,
            spawn::StdioPolicy::RedirectForShellTool,
            request.env,
        )
        .await
        .map_err(ColossalErr::Io)
    }
    #[cfg(target_os = "linux")]
    {
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

        let request = sandboxing::SandboxManager::new().prepare_spawn(
            sandboxing::SandboxCommand {
                program: PathBuf::from(program),
                args: args.to_vec(),
                cwd: cwd.clone(),
                env,
            },
            sandbox_policy,
        )?;

        let mut cmd = Command::new(&request.program);
        cmd.args(&request.args)
            .current_dir(&request.cwd)
            .envs(&request.env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped());

        cmd.spawn().map_err(|e| ColossalErr::Io(e))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let request = sandboxing::SandboxManager::new().prepare_spawn(
            sandboxing::SandboxCommand {
                program: PathBuf::from(command.get(0).ok_or_else(|| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Empty command",
                    ))
                })?),
                args: command[1..].to_vec(),
                cwd: cwd.clone(),
                env,
            },
            sandbox_policy,
        )?;
        spawn::spawn_child_async(
            request.program,
            request.args,
            None,
            request.cwd,
            sandbox_policy,
            spawn::StdioPolicy::RedirectForShellTool,
            request.env,
        )
        .await
        .map_err(|e| ColossalErr::Io(e))
    }
}
