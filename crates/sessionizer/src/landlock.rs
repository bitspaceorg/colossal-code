use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use std::path::Path;

pub fn apply_sandbox_policy_to_current_thread(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Result<(), ColossalErr> {
    #[cfg(target_os = "linux")]
    {
        apply_linux_landlock_policy(sandbox_policy, cwd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        {
            apply_macos_sandbox_policy(sandbox_policy, cwd)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // On other platforms, simply return Ok without applying sandboxing
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
fn landlock_bypass_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var("DISABLE_LANDLOCK").unwrap_or_default() == "1"
        && std::env::var("ALLOW_UNSAFE_SANDBOX_BYPASS").unwrap_or_default() == "1"
}

#[cfg(target_os = "linux")]
fn apply_linux_landlock_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Result<(), ColossalErr> {
    use landlock::{Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI};

    match sandbox_policy {
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access: _, // Landlock doesn't handle network yet
            exclude_tmpdir_env_var,
            exclude_slash_tmp,
        } => {
            // Try to use the highest ABI version available
            let abi = ABI::V3;

            // Create ruleset with file system access controls
            let mut ruleset = Ruleset::default()
                .handle_access(AccessFs::from_all(abi))
                .map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to create Landlock ruleset: {}", e),
                    ))
                })?
                .create()
                .map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to create Landlock ruleset: {}", e),
                    ))
                })?;

            // Add read-only access to system directories needed for basic operation
            for sys_path in &["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
                if std::path::Path::new(sys_path).exists() {
                    ruleset = ruleset
                        .add_rule(landlock::PathBeneath::new(
                            landlock::PathFd::new(sys_path).map_err(|e| {
                                ColossalErr::Io(std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    format!("Failed to open {}: {}", sys_path, e),
                                ))
                            })?,
                            AccessFs::from_read(abi),
                        ))
                        .map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!("Failed to add rule for {}: {}", sys_path, e),
                            ))
                        })?;
                }
            }

            // Add read/write access to /dev for things like /dev/null
            if std::path::Path::new("/dev").exists() {
                ruleset = ruleset
                    .add_rule(landlock::PathBeneath::new(
                        landlock::PathFd::new("/dev").map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("Failed to open /dev: {}", e),
                            ))
                        })?,
                        AccessFs::from_all(abi),
                    ))
                    .map_err(|e| {
                        ColossalErr::Io(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to add rule for /dev: {}", e),
                        ))
                    })?;
            }

            // Add full access to writable roots
            for writable_root in writable_roots {
                let root_path = &writable_root.root;
                if root_path.exists() {
                    let path_fd = landlock::PathFd::new(root_path).map_err(|e| {
                        ColossalErr::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("Failed to open {}: {}", root_path.display(), e),
                        ))
                    })?;

                    ruleset = ruleset
                        .add_rule(landlock::PathBeneath::new(
                            path_fd,
                            AccessFs::from_all(abi), // Full read/write access
                        ))
                        .map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!("Failed to add rule for {}: {}", root_path.display(), e),
                            ))
                        })?;
                }
            }

            // Add /tmp access if not excluded
            if !exclude_slash_tmp && std::path::Path::new("/tmp").exists() {
                ruleset = ruleset
                    .add_rule(landlock::PathBeneath::new(
                        landlock::PathFd::new("/tmp").map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("Failed to open /tmp: {}", e),
                            ))
                        })?,
                        AccessFs::from_all(abi),
                    ))
                    .map_err(|e| {
                        ColossalErr::Io(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to add rule for /tmp: {}", e),
                        ))
                    })?;
            }

            // Add TMPDIR access if not excluded
            if !exclude_tmpdir_env_var {
                if let Ok(tmpdir) = std::env::var("TMPDIR") {
                    let tmpdir_path = std::path::Path::new(&tmpdir);
                    if tmpdir_path.exists() {
                        ruleset = ruleset
                            .add_rule(landlock::PathBeneath::new(
                                landlock::PathFd::new(tmpdir_path).map_err(|e| {
                                    ColossalErr::Io(std::io::Error::new(
                                        std::io::ErrorKind::NotFound,
                                        format!("Failed to open TMPDIR {}: {}", tmpdir, e),
                                    ))
                                })?,
                                AccessFs::from_all(abi),
                            ))
                            .map_err(|e| {
                                ColossalErr::Io(std::io::Error::new(
                                    std::io::ErrorKind::PermissionDenied,
                                    format!("Failed to add rule for TMPDIR {}: {}", tmpdir, e),
                                ))
                            })?;
                    }
                }
            }

            if landlock_bypass_enabled() {
                eprintln!(
                    "[LANDLOCK] Sandbox rules configured but NOT activated (unsafe debug bypass enabled)"
                );
                Ok(())
            } else {
                ruleset.restrict_self().map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to restrict thread with Landlock: {}", e),
                    ))
                })?;
                Ok(())
            }
        }
        SandboxPolicy::DangerFullAccess => {
            // Don't apply any restrictions
            Ok(())
        }
        SandboxPolicy::ReadOnly => {
            // Read-only mode: allow reads to system paths and cwd, but NO writes
            let abi = ABI::V3;

            let mut ruleset = Ruleset::default()
                .handle_access(AccessFs::from_all(abi))
                .map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to create Landlock ruleset: {}", e),
                    ))
                })?
                .create()
                .map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to create Landlock ruleset: {}", e),
                    ))
                })?;

            // Add read-only access to system directories
            for sys_path in &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
                if std::path::Path::new(sys_path).exists() {
                    ruleset = ruleset
                        .add_rule(landlock::PathBeneath::new(
                            landlock::PathFd::new(sys_path).map_err(|e| {
                                ColossalErr::Io(std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    format!("Failed to open {}: {}", sys_path, e),
                                ))
                            })?,
                            AccessFs::from_read(abi),
                        ))
                        .map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!("Failed to add rule for {}: {}", sys_path, e),
                            ))
                        })?;
                }
            }

            // Add read-only access to cwd (workspace)
            if cwd.exists() {
                ruleset = ruleset
                    .add_rule(landlock::PathBeneath::new(
                        landlock::PathFd::new(cwd).map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("Failed to open cwd {}: {}", cwd.display(), e),
                            ))
                        })?,
                        AccessFs::from_read(abi),
                    ))
                    .map_err(|e| {
                        ColossalErr::Io(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to add rule for cwd {}: {}", cwd.display(), e),
                        ))
                    })?;
            }

            // Allow access to /dev for /dev/null etc (read/write needed for output)
            if std::path::Path::new("/dev").exists() {
                ruleset = ruleset
                    .add_rule(landlock::PathBeneath::new(
                        landlock::PathFd::new("/dev").map_err(|e| {
                            ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("Failed to open /dev: {}", e),
                            ))
                        })?,
                        AccessFs::from_all(abi),
                    ))
                    .map_err(|e| {
                        ColossalErr::Io(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to add rule for /dev: {}", e),
                        ))
                    })?;
            }

            if landlock_bypass_enabled() {
                eprintln!(
                    "[LANDLOCK] ReadOnly sandbox rules configured but NOT activated (unsafe debug bypass enabled)"
                );
                Ok(())
            } else {
                ruleset.restrict_self().map_err(|e| {
                    ColossalErr::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("Failed to restrict thread with Landlock (ReadOnly): {}", e),
                    ))
                })?;
                Ok(())
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_macos_sandbox_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Result<(), ColossalErr> {
    use std::io::Write;
    use std::process::Command;
    use tempfile::NamedTempFile;

    // Create a temporary seatbelt profile
    let mut profile = String::from("(version 1)\n");

    match sandbox_policy {
        crate::protocol::SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            exclude_tmpdir_env_var,
            exclude_slash_tmp,
        } => {
            // Allow reading the current working directory and its subdirectories
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}\"))\n",
                cwd.display()
            ));

            // Allow reading the current working directory itself
            profile.push_str(&format!(
                "(allow file-read-metadata (subpath \"{}\"))\n",
                cwd.display()
            ));

            // Add rules for writable roots
            for writable_root in writable_roots {
                if writable_root.recursive {
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        writable_root.root.display()
                    ));
                    profile.push_str(&format!(
                        "(allow file-read-metadata file-write-data (subpath \"{}\"))\n",
                        writable_root.root.display()
                    ));
                } else {
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (literal \"{}\"))\n",
                        writable_root.root.display()
                    ));
                    profile.push_str(&format!(
                        "(allow file-read-metadata file-write-data (literal \"{}\"))\n",
                        writable_root.root.display()
                    ));
                }

                // Add read-only subpath restrictions
                for read_only_path in &writable_root.read_only_subpaths {
                    profile.push_str(&format!(
                        "(allow file-read* (subpath \"{}\"))\n",
                        read_only_path.display()
                    ));
                    profile.push_str(&format!(
                        "(allow file-read-metadata (subpath \"{}\"))\n",
                        read_only_path.display()
                    ));
                }
            }

            // Handle /tmp access
            if !exclude_slash_tmp {
                profile.push_str("(allow file-read* file-write* (subpath \"/tmp\"))\n");
                profile.push_str("(allow file-read-metadata file-write-data (subpath \"/tmp\"))\n");
            }

            // Handle TMPDIR access
            if !exclude_tmpdir_env_var {
                if let Ok(tmpdir) = std::env::var("TMPDIR") {
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        tmpdir
                    ));
                    profile.push_str(&format!(
                        "(allow file-read-metadata file-write-data (subpath \"{}\"))\n",
                        tmpdir
                    ));
                }
            }

            // Network access
            if matches!(network_access, crate::protocol::NetworkAccess::Enabled) {
                profile.push_str("(allow network*)\n");
            } else {
                profile.push_str("(deny network*)\n");
            }
        }
        crate::protocol::SandboxPolicy::DangerFullAccess => {
            // For DangerFullAccess, allow everything (not recommended but possible)
            profile.push_str("(allow default)\n");
        }
        crate::protocol::SandboxPolicy::ReadOnly => {
            // Read-only mode: allow reading cwd and system paths, but NO writes
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}\"))\n",
                cwd.display()
            ));
            profile.push_str(&format!(
                "(allow file-read-metadata (subpath \"{}\"))\n",
                cwd.display()
            ));
            // Deny all writes
            profile.push_str("(deny file-write*)\n");
            // Deny network
            profile.push_str("(deny network*)\n");
        }
    }

    // Add basic system permissions needed for normal operation
    profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
    profile.push_str("(allow file-read* (subpath \"/bin\"))\n");
    profile.push_str("(allow file-read* (subpath \"/lib\"))\n");
    profile.push_str("(allow file-read* (subpath \"/System\"))\n");
    profile.push_str("(allow process-exec)\n");

    // Create a temporary file for the profile
    let mut temp_file = NamedTempFile::new().map_err(|e| {
        ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to create temp profile file: {}", e),
        ))
    })?;

    temp_file.write_all(profile.as_bytes()).map_err(|e| {
        ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to write profile: {}", e),
        ))
    })?;

    // Apply the sandbox profile using sandbox-exec
    let output = Command::new("sandbox-exec")
        .arg("-f")
        .arg(temp_file.path())
        .arg("true") // Just test if the sandbox is valid
        .output()
        .map_err(|e| {
            ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to execute sandbox-exec: {}", e),
            ))
        })?;

    if !output.status.success() {
        return Err(ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "Sandbox profile validation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )));
    }

    Ok(())
}
