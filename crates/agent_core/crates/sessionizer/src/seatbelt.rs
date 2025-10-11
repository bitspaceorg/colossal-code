use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Child;
use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use crate::spawn::{COLOSSAL_SANDBOX_ENV_VAR, StdioPolicy, spawn_child_async};
use std::process::Command;
use tempfile::NamedTempFile;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;

const MACOS_SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
const MACOS_PATH_TO_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

pub async fn spawn_command_under_seatbelt(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
    stdio_policy: StdioPolicy,
    mut env: HashMap<String, String>,
) -> Result<Child, ColossalErr> {
    let args = create_seatbelt_command_args(command, sandbox_policy, &cwd);
    // eprintln!("Seatbelt args: {:?}", args);
    env.insert(COLOSSAL_SANDBOX_ENV_VAR.to_string(), "seatbelt".to_string());
    spawn_child_async(
        PathBuf::from(MACOS_PATH_TO_SEATBELT_EXECUTABLE),
        args,
        None,
        cwd,
        sandbox_policy,
        stdio_policy,
        env,
    )
    .await
    .map_err(|e| ColossalErr::Io(e))
}

pub fn apply_sandbox_policy(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Result<(), ColossalErr> {
    use std::process::Command;
    use std::fs::File;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use std::os::unix::ffi::OsStrExt;

    // Create a temporary seatbelt profile
    let mut profile = String::from("(version 1)\n");
    
    match sandbox_policy {
        crate::protocol::SandboxPolicy::WorkspaceWrite { 
            writable_roots, 
            network_access, 
            exclude_tmpdir_env_var, 
            exclude_slash_tmp 
        } => {
            // Allow reading the current working directory and its subdirectories
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", cwd.display()));
            
            // Allow reading the current working directory itself
            profile.push_str(&format!("(allow file-read-metadata (subpath \"{}\"))\n", cwd.display()));
            
            // Add rules for writable roots
            for writable_root in writable_roots {
                if writable_root.recursive {
                    profile.push_str(&format!("(allow file-read* file-write* (subpath \"{}\"))\n", writable_root.root.display()));
                    profile.push_str(&format!("(allow file-read-metadata file-write-data (subpath \"{}\"))\n", writable_root.root.display()));
                } else {
                    profile.push_str(&format!("(allow file-read* file-write* (literal \"{}\"))\n", writable_root.root.display()));
                    profile.push_str(&format!("(allow file-read-metadata file-write-data (literal \"{}\"))\n", writable_root.root.display()));
                }
                
                // Add read-only subpath restrictions
                for read_only_path in &writable_root.read_only_subpaths {
                    profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", read_only_path.display()));
                    profile.push_str(&format!("(allow file-read-metadata (subpath \"{}\"))\n", read_only_path.display()));
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
                    profile.push_str(&format!("(allow file-read* file-write* (subpath \"{}\"))\n", tmpdir));
                    profile.push_str(&format!("(allow file-read-metadata file-write-data (subpath \"{}\"))\n", tmpdir));
                }
            }
            
            // Network access
            if matches!(network_access, crate::protocol::NetworkAccess::Enabled) {
                profile.push_str("(allow network*)\n");
            } else {
                profile.push_str("(deny network*)\n");
            }
        },
        crate::protocol::SandboxPolicy::DangerFullAccess => {
            // For DangerFullAccess, allow everything (not recommended but possible)
            profile.push_str("(allow default)\n");
        }
    }
    
    // Add basic system permissions needed for normal operation
    profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
    profile.push_str("(allow file-read* (subpath \"/bin\"))\n");
    profile.push_str("(allow file-read* (subpath \"/lib\"))\n");
    profile.push_str("(allow file-read* (subpath \"/System\"))\n");
    profile.push_str("(allow process-exec)\n");
    
    // Create a temporary file for the profile
    let mut temp_file = NamedTempFile::new()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to create temp profile file: {}", e)
        )))?;
    
    temp_file.write_all(profile.as_bytes())
        .map_err(|e| ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to write profile: {}", e)
        )))?;
    
    // Apply the sandbox profile using sandbox-exec to 'true' to test if the sandbox is valid
    let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
        .arg("-f")
        .arg(temp_file.path())
        .arg("true") // Just test if the sandbox is valid
        .output()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to execute sandbox-exec: {}", e)
        )))?;
    
    if !output.status.success() {
        return Err(ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Sandbox profile validation failed: {}", String::from_utf8_lossy(&output.stderr))
        )));
    }

    Ok(())
}

fn create_seatbelt_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Vec<String> {
    let (file_write_policy, extra_cli_args) = {
        if sandbox_policy.has_full_disk_write_access() {
            (
                r#"(allow file-write* (regex #"^/"))"#.to_string(),
                Vec::<String>::new(),
            )
        } else {
            let writable_roots = sandbox_policy.get_writable_roots_with_cwd(cwd);
            let mut writable_folder_policies: Vec<String> = Vec::new();
            let mut cli_args: Vec<String> = Vec::new();
            for (index, wr) in writable_roots.iter().enumerate() {
                let canonical_root = wr.root.canonicalize().unwrap_or_else(|_| wr.root.clone());
                let root_param = format!("WRITABLE_ROOT_{index}");
                cli_args.push(format!(
                    "-D{root_param}={}",
                    canonical_root.to_string_lossy()
                ));
                if wr.read_only_subpaths.is_empty() {
                    writable_folder_policies.push(format!("(subpath (param \"{root_param}\"))"));
                } else {
                    let mut require_parts: Vec<String> = Vec::new();
                    require_parts.push(format!("(subpath (param \"{root_param}\"))"));
                    for (subpath_index, ro) in wr.read_only_subpaths.iter().enumerate() {
                        let canonical_ro = ro.canonicalize().unwrap_or_else(|_| ro.clone());
                        let ro_param = format!("WRITABLE_ROOT_{index}_RO_{subpath_index}");
                        cli_args.push(format!("-D{ro_param}={}", canonical_ro.to_string_lossy()));
                        require_parts
                            .push(format!("(require-not (subpath (param \"{ro_param}\")))"));
                    }
                    let policy_component = format!("(require-all {} )", require_parts.join(" "));
                    writable_folder_policies.push(policy_component);
                }
            }
            if writable_folder_policies.is_empty() {
                ("".to_string(), Vec::<String>::new())
            } else {
                let file_write_policy = format!(
                    "(allow file-write*\n{}\n)",
                    writable_folder_policies.join(" ")
                );
                (file_write_policy, cli_args)
            }
        }
    };
    let file_read_policy = if sandbox_policy.has_full_disk_read_access() {
        "; allow read-only file operations\n(allow file-read*)"
    } else {
        ""
    };
    let network_policy = if sandbox_policy.has_full_network_access() {
        "(allow network-outbound)\n(allow network-inbound)\n(allow system-socket)"
    } else {
        ""
    };
    let full_policy = format!(
        "{MACOS_SEATBELT_BASE_POLICY}\n{file_read_policy}\n{file_write_policy}\n{network_policy}"
    );
    // eprintln!("Generated SBPL policy:\n{}", full_policy);
    let mut seatbelt_args: Vec<String> = vec!["-p".to_string(), full_policy];
    seatbelt_args.extend(extra_cli_args);
    seatbelt_args.push("--".to_string());
    seatbelt_args.extend(command);
    seatbelt_args
}

#[cfg(test)]
mod tests {
    use super::{MACOS_SEATBELT_BASE_POLICY, create_seatbelt_command_args};
    use crate::protocol::{SandboxPolicy, WritableRoot};
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn create_seatbelt_args_with_read_only_git_subpath() {
        if cfg!(target_os = "windows") {
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        let PopulatedTmp {
            root_with_git,
            root_without_git,
            root_with_git_canon,
            root_with_git_git_canon,
            root_without_git_canon,
        } = populate_tmpdir(tmp.path());
        let cwd = tmp.path().join("cwd");
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                WritableRoot {
                    root: root_with_git.clone(),
                    recursive: true,
                    read_only_subpaths: vec![root_with_git.join(".git")],
                },
                WritableRoot {
                    root: root_without_git.clone(),
                    recursive: true,
                    read_only_subpaths: vec![],
                },
            ],
            network_access: crate::protocol::NetworkAccess::Restricted,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };
        let args = create_seatbelt_command_args(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            &policy,
            &cwd,
        );
        let expected_policy = format!(
            r#"{MACOS_SEATBELT_BASE_POLICY}
; allow read-only file operations
(allow file-read*)
(allow file-write*
(require-all (subpath (param "WRITABLE_ROOT_0")) (require-not (subpath (param "WRITABLE_ROOT_0_RO_0"))) ) (subpath (param "WRITABLE_ROOT_1")) (subpath (param "WRITABLE_ROOT_2"))
)
"#,
        );
        let mut expected_args = vec![
            "-p".to_string(),
            expected_policy,
            format!(
                "-DWRITABLE_ROOT_0={}",
                root_with_git_canon.to_string_lossy()
            ),
            format!(
                "-DWRITABLE_ROOT_0_RO_0={}",
                root_with_git_git_canon.to_string_lossy()
            ),
            format!(
                "-DWRITABLE_ROOT_1={}",
                root_without_git_canon.to_string_lossy()
            ),
            format!("-DWRITABLE_ROOT_2={}", cwd.to_string_lossy()),
        ];
        expected_args.extend(vec![
            "--".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ]);
        assert_eq!(expected_args, args);
    }

    #[test]
    fn create_seatbelt_args_for_cwd_as_git_repo() {
        if cfg!(target_os = "windows") {
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        let PopulatedTmp {
            root_with_git,
            root_with_git_canon,
            root_with_git_git_canon,
            ..
        } = populate_tmpdir(tmp.path());
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: crate::protocol::NetworkAccess::Restricted,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let args = create_seatbelt_command_args(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            &policy,
            root_with_git.as_path(),
        );
        let tmpdir_env_var = std::env::var("TMPDIR")
            .ok()
            .map(PathBuf::from)
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.to_string_lossy().to_string());
        let tempdir_policy_entry = if tmpdir_env_var.is_some() {
            r#" (subpath (param "WRITABLE_ROOT_2"))"#
        } else {
            ""
        };
        let expected_policy = format!(
            r#"{MACOS_SEATBELT_BASE_POLICY}
; allow read-only file operations
(allow file-read*)
(allow file-write*
(require-all (subpath (param "WRITABLE_ROOT_0")) (require-not (subpath (param "WRITABLE_ROOT_0_RO_0"))) ) (subpath (param "WRITABLE_ROOT_1")){tempdir_policy_entry}
)
"#,
        );
        let mut expected_args = vec![
            "-p".to_string(),
            expected_policy,
            format!(
                "-DWRITABLE_ROOT_0={}",
                root_with_git_canon.to_string_lossy()
            ),
            format!(
                "-DWRITABLE_ROOT_0_RO_0={}",
                root_with_git_git_canon.to_string_lossy()
            ),
            format!(
                "-DWRITABLE_ROOT_1={}",
                PathBuf::from("/tmp")
                    .canonicalize()
                    .expect("canonicalize /tmp")
                    .to_string_lossy()
            ),
        ];
        if let Some(p) = tmpdir_env_var {
            expected_args.push(format!("-DWRITABLE_ROOT_2={p}"));
        }
        expected_args.extend(vec![
            "--".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ]);
        assert_eq!(expected_args, args);
    }

    struct PopulatedTmp {
        root_with_git: PathBuf,
        root_without_git: PathBuf,
        root_with_git_canon: PathBuf,
        root_with_git_git_canon: PathBuf,
        root_without_git_canon: PathBuf,
    }

    fn populate_tmpdir(tmp: &Path) -> PopulatedTmp {
        let root_with_git = tmp.join("with_git");
        let root_without_git = tmp.join("no_git");
        fs::create_dir_all(&root_with_git).expect("create with_git");
        fs::create_dir_all(&root_without_git).expect("create no_git");
        fs::create_dir_all(root_with_git.join(".git")).expect("create .git");
        let root_with_git_canon = root_with_git.canonicalize().expect("canonicalize with_git");
        let root_with_git_git_canon = root_with_git_canon.join(".git");
        let root_without_git_canon = root_without_git
            .canonicalize()
            .expect("canonicalize no_git");
        PopulatedTmp {
            root_with_git,
            root_without_git,
            root_with_git_canon,
            root_with_git_git_canon,
            root_without_git_canon,
        }
    }
}
