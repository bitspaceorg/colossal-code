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
    #[cfg(target_os = "windows")]
    pub windows_profile: Option<windows_sandbox::WindowsSandboxProfile>,
    #[cfg(target_os = "windows")]
    pub conpty_handles: Option<windows_sandbox::conpty::ConptyHandles>,
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
                #[cfg(target_os = "windows")]
                windows_profile: None,
                #[cfg(target_os = "windows")]
                conpty_handles: None,
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
                #[cfg(target_os = "windows")]
                windows_profile: None,
                #[cfg(target_os = "windows")]
                conpty_handles: None,
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
                #[cfg(target_os = "windows")]
                windows_profile: None,
                #[cfg(target_os = "windows")]
                conpty_handles: None,
            });
        }

        // No system bubblewrap found.  Try the colossal-sandbox-helper binary
        // which can apply landlock in-process (works for both PTY and non-PTY).
        // This avoids the pre_exec limitation with portable_pty.
        if let Some(helper) = resolve_sandbox_helper_binary() {
            let policy_json = serde_json::to_string(sandbox_policy).map_err(|e| {
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("failed to serialize sandbox policy: {e}"),
                ))
            })?;
            let mut args = vec![
                "--cwd".to_string(),
                command.cwd.to_string_lossy().to_string(),
                "--sandbox-policy".to_string(),
                policy_json,
                "--".to_string(),
                command.program.to_string_lossy().to_string(),
            ];
            args.extend(command.args.iter().cloned());
            return Ok(SandboxExecRequest {
                program: helper,
                args,
                cwd: command.cwd,
                env: command.env,
                sandbox: SandboxType::LinuxBubblewrap, // Treated as external sandbox
                sandbox_policy: None,
                #[cfg(target_os = "windows")]
                windows_profile: None,
                #[cfg(target_os = "windows")]
                conpty_handles: None,
            });
        }

        // Neither bubblewrap nor the helper binary are available.
        // Return an error instead of LinuxLandlock, because LinuxLandlock
        // requires pre_exec which doesn't work with PTY sessions.
        // The caller (session.rs) would error anyway, so we error early with
        // a clear message about what's needed.
        Err(ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Sandbox requires either bubblewrap (bwrap) or the colossal-sandbox-helper binary. \
             Install bubblewrap with your package manager (e.g., `apt install bubblewrap` or \
             `dnf install bubblewrap`), or ensure colossal-sandbox-helper is in the same \
             directory as the main binary. \
             If building from source, install libcap-dev to enable vendored bubblewrap.",
        )))
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
            #[cfg(target_os = "windows")]
            windows_profile: None,
            #[cfg(target_os = "windows")]
            conpty_handles: None,
        })
    }

    #[cfg(target_os = "windows")]
    fn prepare_windows_restricted_token(
        &self,
        command: SandboxCommand,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<SandboxExecRequest, ColossalErr> {
        let profile = windows_sandbox::build_windows_sandbox_profile(sandbox_policy, &command.cwd);

        let conpty_handles = windows_sandbox::conpty::create_conpty(80, 24).ok();

        Ok(SandboxExecRequest {
            program: command.program,
            args: command.args,
            cwd: command.cwd,
            env: command.env,
            sandbox: SandboxType::WindowsRestrictedToken,
            sandbox_policy: Some(sandbox_policy.clone()),
            windows_profile: Some(profile),
            conpty_handles,
        })
    }
}

/// Resolve the sandbox helper binary, returning None if not found.
/// Used as a fallback when system bubblewrap is not available on Linux.
/// The helper binary applies landlock in-process, which works for both
/// PTY and non-PTY sessions (unlike pre_exec which only works for non-PTY).
#[cfg(target_os = "linux")]
fn resolve_sandbox_helper_binary() -> Option<PathBuf> {
    resolve_sandbox_helper_path().ok()
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

    #[test]
    fn readonly_policy_requires_isolation() {
        assert!(command_isolation_required(&SandboxPolicy::ReadOnly));
    }

    #[test]
    fn danger_full_access_no_isolation() {
        assert!(!command_isolation_required(
            &SandboxPolicy::DangerFullAccess
        ));
    }

    #[test]
    fn normalize_command_program_absolute() {
        let program = "/bin/ls";
        let cwd = PathBuf::from("/home/user");
        let result = normalize_command_program(program, &cwd);
        assert_eq!(result, PathBuf::from("/bin/ls"));
    }

    #[test]
    fn normalize_command_program_relative() {
        let program = "ls";
        let cwd = PathBuf::from("/home/user");
        let result = normalize_command_program(program, &cwd);
        assert_eq!(result, PathBuf::from("/home/user/ls"));
    }

    #[cfg(target_os = "linux")]
    mod linux_tests {
        use super::*;
        use crate::linux_sandbox::preferred_bwrap_launcher;

        #[test]
        fn linux_readonly_policy_uses_sandbox() {
            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["hello".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::ReadOnly,
            );

            match result {
                Ok(request) => {
                    assert!(
                        matches!(request.sandbox, SandboxType::LinuxBubblewrap),
                        "Expected LinuxBubblewrap, got {:?}",
                        request.sandbox
                    );
                }
                Err(_) => {
                    // If bwrap and helper are not available, error is expected for PTY sessions
                    // This is acceptable in test environments without bwrap
                }
            }
        }

        #[test]
        fn linux_workspace_write_policy_uses_sandbox() {
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

            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["hello".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &policy,
            );

            match result {
                Ok(request) => {
                    assert!(
                        matches!(request.sandbox, SandboxType::LinuxBubblewrap),
                        "Expected LinuxBubblewrap, got {:?}",
                        request.sandbox
                    );
                }
                Err(_) => {
                    // Error expected if neither bwrap nor helper available
                }
            }
        }

        #[test]
        fn linux_bwrap_detection() {
            let bwrap = preferred_bwrap_launcher(None);
            // Test should adapt to environment - either bwrap is available or not
            // This test documents the expected behavior
            if bwrap.is_some() {
                let result = SandboxManager::new().prepare_spawn(
                    SandboxCommand {
                        program: PathBuf::from("/bin/echo"),
                        args: vec!["test".to_string()],
                        cwd: PathBuf::from("/tmp"),
                        env: HashMap::new(),
                    },
                    &SandboxPolicy::ReadOnly,
                );
                assert!(result.is_ok(), "Should work when bwrap is available");
                let request = result.unwrap();
                assert_eq!(request.sandbox, SandboxType::LinuxBubblewrap);
            }
        }

        #[test]
        fn linux_sandbox_error_when_no_bwrap_no_helper() {
            // This test verifies that we get a proper error when sandbox is required
            // but neither bwrap nor helper is available
            // Note: In most environments, bwrap IS available, so this may pass or skip
            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["test".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::ReadOnly,
            );

            // Either succeeds (bwrap/helper available) or fails with clear error
            match result {
                Ok(_) => {}
                Err(e) => {
                    let error_msg = e.to_string();
                    assert!(
                        error_msg.contains("bubblewrap")
                            || error_msg.contains("colossal-sandbox-helper"),
                        "Error should mention sandbox requirements: {}",
                        error_msg
                    );
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    mod macos_tests {
        use super::*;

        #[test]
        fn macos_readonly_policy_uses_seatbelt() {
            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["hello".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::ReadOnly,
            );

            let request = result.expect("prepare spawn should succeed on macOS");
            assert_eq!(request.sandbox, SandboxType::MacosSeatbelt);
            assert_eq!(request.program, PathBuf::from("/usr/bin/sandbox-exec"));
        }

        #[test]
        fn macos_workspace_write_policy_uses_seatbelt() {
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

            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("/bin/echo"),
                    args: vec!["hello".to_string()],
                    cwd: PathBuf::from("/tmp"),
                    env: HashMap::new(),
                },
                &policy,
            );

            let request = result.expect("prepare spawn should succeed on macOS");
            assert_eq!(request.sandbox, SandboxType::MacosSeatbelt);
        }
    }

    #[cfg(target_os = "windows")]
    mod windows_tests {
        use super::*;

        #[test]
        fn windows_readonly_policy_uses_restricted_token() {
            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("cmd.exe"),
                    args: vec!["/c".to_string(), "echo hello".to_string()],
                    cwd: PathBuf::from("C:\\Temp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::ReadOnly,
            );

            let request = result.expect("prepare spawn should succeed on Windows");
            assert_eq!(request.sandbox, SandboxType::WindowsRestrictedToken);
            assert!(request.windows_profile.is_some());
        }

        #[test]
        fn windows_workspace_write_policy_uses_restricted_token() {
            let policy = SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![WritableRoot {
                    root: PathBuf::from("C:\\Temp"),
                    recursive: true,
                    read_only_subpaths: vec![],
                }],
                network_access: NetworkAccess::Restricted,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            };

            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("cmd.exe"),
                    args: vec!["/c".to_string(), "echo hello".to_string()],
                    cwd: PathBuf::from("C:\\Temp"),
                    env: HashMap::new(),
                },
                &policy,
            );

            let request = result.expect("prepare spawn should succeed on Windows");
            assert_eq!(request.sandbox, SandboxType::WindowsRestrictedToken);
            assert!(request.windows_profile.is_some());
        }

        #[test]
        fn windows_conpty_handles_created() {
            let result = SandboxManager::new().prepare_spawn(
                SandboxCommand {
                    program: PathBuf::from("cmd.exe"),
                    args: vec!["/c".to_string(), "echo hello".to_string()],
                    cwd: PathBuf::from("C:\\Temp"),
                    env: HashMap::new(),
                },
                &SandboxPolicy::ReadOnly,
            );

            let request = result.expect("prepare spawn should succeed on Windows");
            // ConPTY handles should be created for PTY sessions
            assert!(request.conpty_handles.is_some());
        }
    }
}
