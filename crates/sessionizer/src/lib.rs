use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Child;

pub mod bundled_nu;
pub mod error;
pub mod hash_id;
#[cfg(target_os = "linux")]
pub mod landlock;
#[cfg(target_os = "linux")]
pub mod linux_sandbox;
pub mod managed_nu;
pub mod manager;
pub mod osc133;
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
#[cfg(target_os = "linux")]
pub(crate) mod vendored_bwrap;
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

        // If bubblewrap is not available, apply landlock/seccomp in the forked
        // child process before exec.  pre_exec runs after fork() but before
        // exec(), which is exactly the right place for per-process sandboxing.
        if request.sandbox == sandboxing::SandboxType::LinuxLandlock {
            if let Some(policy) = request.sandbox_policy {
                let cwd_for_sandbox = request.cwd.clone();
                unsafe {
                    cmd.pre_exec(move || {
                        crate::landlock::apply_sandbox_policy_to_current_thread(
                            &policy,
                            &cwd_for_sandbox,
                        )
                        .map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
                        })
                    });
                }
            }
        }

        cmd.spawn().map_err(|e| ColossalErr::Io(e))
    }
    #[cfg(target_os = "windows")]
    {
        use tokio::process::Command;

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

        // On Windows, use inline sandboxing via windows_sandbox::spawn_sandboxed_command
        if request.sandbox == sandboxing::SandboxType::WindowsRestrictedToken {
            if let Some(profile) = request.windows_profile {
                return tokio::task::spawn_blocking(move || {
                    crate::windows_sandbox::spawn_sandboxed_command(
                        &request.program,
                        &request.args,
                        &request.cwd,
                        &request.env,
                        &profile,
                    )
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))
                })
                .await
                .map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?
                .map_err(|e| ColossalErr::Io(e));
            }
        }

        // Fallback: spawn normally (shouldn't happen, but handle it)
        let mut cmd = Command::new(&request.program);
        cmd.args(&request.args)
            .current_dir(&request.cwd)
            .envs(&request.env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped());
        cmd.spawn().map_err(|e| ColossalErr::Io(e))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
