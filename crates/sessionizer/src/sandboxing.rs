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
    LinuxBubblewrap,
    LinuxLandlock,
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
    pub sandbox_policy: Option<SandboxPolicy>,
}

#[derive(Default)]
pub struct SandboxManager;

impl SandboxManager {
    pub fn new() -> Self {
        Self
    }

    pub fn prepare_spawn(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        if matches!(sandbox_policy, SandboxPolicy::DangerFullAccess) {
            return Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::None,
                sandbox_policy: None,
            });
        }

        #[cfg(target_os = "macos")]
        {
            self.prepare_macos_seatbelt(command, sandbox_policy)
        }
        #[cfg(target_os = "linux")]
        {
            self.prepare_linux_sandbox(command, sandbox_policy)
        }
        #[cfg(target_os = "windows")]
        {
            self.prepare_windows_restricted_token(command, sandbox_policy)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = sandbox_policy;
            Ok(SandboxExecRequest {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::None,
                sandbox_policy: None,
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn prepare_linux_sandbox(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        use crate::linux_sandbox::{create_bwrap_command_args, preferred_bwrap_launcher};

        // Prefer bubblewrap if available - it provides mount namespace isolation
        if let Some(bwrap) = preferred_bwrap_launcher(None) {
            let mut cmd_vec = vec![command.program.to_string_lossy().to_string()];
            cmd_vec.extend(command.args.iter().cloned());
            let bwrap_args = create_bwrap_command_args(sandbox_policy, &command.cwd, &cmd_vec);
            return Ok(SandboxExecRequest {
                program: bwrap.program().to_path_buf(),
                args: bwrap_args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::LinuxBubblewrap,
                sandbox_policy: None,
            });
        }

        // Fallback: return original command, caller applies landlock via pre_exec
        Ok(SandboxExecRequest {
            program: command.program,
            args: command.args,
            cwd: command.cwd,
            env: command.env,
            sandbox: SandboxType::LinuxLandlock,
            sandbox_policy: Some(sandbox_policy.clone()),
        })
    }

    #[cfg(target_os = "macos")]
    fn prepare_macos_seatbelt(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        let mut argv = vec![command.program.to_string_lossy().to_string()];
        argv.extend(command.args);
        Ok(SandboxExecRequest {
            program: PathBuf::from(seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE),
            args: seatbelt::create_seatbelt_command_args(argv, sandbox_policy, &command.cwd),
            cwd: command.cwd,
            env: command.env,
            sandbox: SandboxType::MacosSeatbelt,
            sandbox_policy: None,
        })
    }

    #[cfg(target_os = "windows")]
    fn prepare_windows_restricted_token(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        let profile = windows_sandbox::build_windows_sandbox_profile(sandbox_policy, &command.cwd);
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
            sandbox_policy: None,
        })
    }
}

/// Resolve the sandbox helper binary path.
/// Only needed on Windows where in-process sandboxing is not yet supported.
pub fn resolve_sandbox_helper_path() -> Result<PathBuf, ColossalErr> {
    // Check environment variable first
    if let Ok(path) = std::env::var("COLOSSAL_SANDBOX_HELPER") {
        let candidate = PathBuf::from(&path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

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
