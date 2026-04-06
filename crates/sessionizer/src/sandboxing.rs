use crate::error::ColossalErr;
#[cfg(test)]
use crate::protocol::NetworkAccess;
use crate::protocol::SandboxPolicy;
#[cfg(target_os = "macos")]
use crate::seatbelt;
#[cfg(target_os = "windows")]
use crate::windows_sandbox;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxType {
    None,
    MacosSeatbelt,
    LinuxHelper,
    WindowsRestrictedToken,
}

#[derive(Debug)]
pub struct SandboxCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

#[derive(Debug)]
pub struct SandboxExecRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub sandbox: SandboxType,
}

#[derive(Default)]
pub struct SandboxManager;

impl SandboxManager {
    pub fn new() -> Self {
        Self
    }

    pub fn get_platform_sandbox() -> Option<SandboxType> {
        if cfg!(target_os = "linux") {
            Some(SandboxType::LinuxHelper)
        } else if cfg!(target_os = "macos") {
            Some(SandboxType::MacosSeatbelt)
        } else if cfg!(target_os = "windows") {
            Some(SandboxType::WindowsRestrictedToken)
        } else {
            None
        }
    }

    pub fn prepare_spawn(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        let sandbox = if matches!(sandbox_policy, SandboxPolicy::DangerFullAccess) {
            SandboxType::None
        } else {
            Self::get_platform_sandbox().unwrap_or(SandboxType::None)
        };

        match sandbox {
            SandboxType::None => Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox,
            }),
            SandboxType::WindowsRestrictedToken => {
                self.prepare_windows_restricted_token(command, sandbox_policy)
            }
            SandboxType::LinuxHelper => self.prepare_linux_helper(command, sandbox_policy),
            SandboxType::MacosSeatbelt => self.prepare_macos_seatbelt(command, sandbox_policy),
        }
    }

    fn prepare_linux_helper(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        #[cfg(target_os = "linux")]
        {
            let helper = resolve_sandbox_helper_path()?;
            let policy = serde_json::to_string(sandbox_policy).map_err(|err| {
                ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
            })?;

            let mut args = vec![
                "--cwd".to_string(),
                command.cwd.to_string_lossy().to_string(),
                "--sandbox-policy".to_string(),
                policy,
                "--".to_string(),
                command.program.to_string_lossy().to_string(),
            ];
            args.extend(command.args);
            Ok(SandboxExecRequest {
                program: helper,
                args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::LinuxHelper,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = sandbox_policy;
            Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::None,
            })
        }
    }

    fn prepare_macos_seatbelt(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        #[cfg(target_os = "macos")]
        {
            let mut argv = vec![command.program.to_string_lossy().to_string()];
            argv.extend(command.args);
            Ok(SandboxExecRequest {
                program: PathBuf::from(seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE),
                args: seatbelt::create_seatbelt_command_args(argv, sandbox_policy, &command.cwd),
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::MacosSeatbelt,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sandbox_policy;
            Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::None,
            })
        }
    }

    fn prepare_windows_restricted_token(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        #[cfg(target_os = "windows")]
        {
            let profile =
                windows_sandbox::build_windows_sandbox_profile(sandbox_policy, &command.cwd);
            let mut env = command.env;
            env.insert(
                "COLOSSAL_WINDOWS_SANDBOX_PROFILE".to_string(),
                profile.serialized_policy(),
            );
            let helper = resolve_sandbox_helper_path()?;
            let mut args = vec![
                "--cwd".to_string(),
                command.cwd.to_string_lossy().to_string(),
                "--windows-sandbox-profile".to_string(),
                profile.serialized_policy(),
                "--".to_string(),
                command.program.to_string_lossy().to_string(),
            ];
            args.extend(command.args);
            Ok(SandboxExecRequest {
                program: helper,
                args,
                cwd: command.cwd,
                env,
                sandbox: SandboxType::WindowsRestrictedToken,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = sandbox_policy;
            Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::WindowsRestrictedToken,
            })
        }
    }
}

pub fn resolve_sandbox_helper_path() -> Result<PathBuf, ColossalErr> {
    let current_exe = std::env::current_exe().map_err(ColossalErr::Io)?;
    let Some(bin_dir) = current_exe.parent() else {
        return Err(ColossalErr::MissingSandboxHelper);
    };

    let search_dirs = [Some(bin_dir), bin_dir.parent()].into_iter().flatten();
    for dir in search_dirs {
        for candidate in sandbox_helper_candidates(dir) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(ColossalErr::MissingSandboxHelper)
}

fn sandbox_helper_candidates(bin_dir: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        vec![
            bin_dir.join("colossal-sandbox-helper.exe"),
            bin_dir.join("colossal-sandbox-helper"),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![bin_dir.join("colossal-sandbox-helper")]
    }
}

#[cfg(target_os = "linux")]
pub fn resolve_linux_helper_path() -> Result<PathBuf, ColossalErr> {
    resolve_sandbox_helper_path()
}

#[cfg(not(target_os = "linux"))]
pub fn resolve_linux_helper_path() -> Result<PathBuf, ColossalErr> {
    Err(ColossalErr::MissingSandboxHelper)
}

pub fn command_isolation_required(sandbox_policy: &SandboxPolicy) -> bool {
    !matches!(sandbox_policy, SandboxPolicy::DangerFullAccess)
}

pub fn normalize_command_program(program: &str, cwd: &Path) -> PathBuf {
    let candidate = PathBuf::from(program);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

#[cfg(test)]
fn windows_profile_marker(policy: &SandboxPolicy) -> String {
    match policy {
        SandboxPolicy::DangerFullAccess => "danger-full-access".to_string(),
        SandboxPolicy::ReadOnly => "read-only".to_string(),
        SandboxPolicy::WorkspaceWrite { network_access, .. } => format!(
            "workspace-write:{}",
            match network_access {
                NetworkAccess::Enabled => "network-enabled",
                NetworkAccess::Restricted => "network-restricted",
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{NetworkAccess, WritableRoot};

    #[test]
    fn danger_full_access_skips_platform_wrapper() {
        let request = SandboxManager::new()
            .prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["hello".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::DangerFullAccess,
            )
            .expect("prepare spawn");

        assert_eq!(request.sandbox, SandboxType::None);
        assert_eq!(request.program, PathBuf::from("/bin/echo"));
    }

    #[test]
    fn workspace_write_requires_isolation() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![WritableRoot {
                root: PathBuf::from("/tmp"),
                recursive: true,
                read_only_subpaths: vec![],
            }],
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        assert!(command_isolation_required(&policy));
    }

    #[test]
    fn windows_profile_marker_reflects_policy_shape() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        assert_eq!(
            windows_profile_marker(&policy),
            "workspace-write:network-restricted"
        );
    }
}
