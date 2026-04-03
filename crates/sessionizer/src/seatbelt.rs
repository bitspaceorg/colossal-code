use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use crate::spawn::{COLOSSAL_SANDBOX_ENV_VAR, StdioPolicy, spawn_child_async};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Child;

pub(crate) const MACOS_SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
const MACOS_RESTRICTED_READ_ONLY_PLATFORM_DEFAULTS: &str =
    include_str!("restricted_read_only_platform_defaults.sbpl");
pub(crate) const MACOS_PATH_TO_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Clone)]
struct SeatbeltAccessRoot {
    root: PathBuf,
    recursive: bool,
    excluded_subpaths: Vec<PathBuf>,
}

pub async fn spawn_command_under_seatbelt(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: PathBuf,
    stdio_policy: StdioPolicy,
    mut env: HashMap<String, String>,
) -> Result<Child, ColossalErr> {
    let args = create_seatbelt_command_args(command, sandbox_policy, &cwd);
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
    .map_err(ColossalErr::Io)
}

pub fn apply_sandbox_policy(sandbox_policy: &SandboxPolicy, cwd: &Path) -> Result<(), ColossalErr> {
    let output = std::process::Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
        .args(create_seatbelt_command_args(
            vec!["true".to_string()],
            sandbox_policy,
            cwd,
        ))
        .output()
        .map_err(|e| {
            ColossalErr::Io(std::io::Error::other(format!(
                "Failed to execute sandbox-exec: {e}"
            )))
        })?;

    if !output.status.success() {
        return Err(ColossalErr::Io(std::io::Error::other(format!(
            "Sandbox profile validation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))));
    }

    Ok(())
}

fn normalize_path_for_sandbox(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }

    path.canonicalize().ok().or(Some(path.to_path_buf()))
}

fn build_seatbelt_access_policy(
    action: &str,
    param_prefix: &str,
    roots: Vec<SeatbeltAccessRoot>,
) -> (String, Vec<(String, PathBuf)>) {
    let mut policy_components = Vec::new();
    let mut params = Vec::new();

    for (index, access_root) in roots.into_iter().enumerate() {
        let root =
            normalize_path_for_sandbox(access_root.root.as_path()).unwrap_or(access_root.root);
        let root_param = format!("{param_prefix}_{index}");
        params.push((root_param.clone(), root));

        let root_policy = if access_root.recursive {
            format!("(subpath (param \"{root_param}\"))")
        } else {
            format!("(literal (param \"{root_param}\"))")
        };

        if access_root.excluded_subpaths.is_empty() {
            policy_components.push(root_policy);
            continue;
        }

        let mut require_parts = vec![root_policy];
        for (excluded_index, excluded_subpath) in
            access_root.excluded_subpaths.into_iter().enumerate()
        {
            let excluded_subpath =
                normalize_path_for_sandbox(excluded_subpath.as_path()).unwrap_or(excluded_subpath);
            let excluded_param = format!("{param_prefix}_{index}_EXCLUDED_{excluded_index}");
            params.push((excluded_param.clone(), excluded_subpath));
            require_parts.push(format!(
                "(require-not (literal (param \"{excluded_param}\")))"
            ));
            require_parts.push(format!(
                "(require-not (subpath (param \"{excluded_param}\")))"
            ));
        }
        policy_components.push(format!("(require-all {} )", require_parts.join(" ")));
    }

    if policy_components.is_empty() {
        (String::new(), Vec::new())
    } else {
        (
            format!("(allow {action}\n{}\n)", policy_components.join(" ")),
            params,
        )
    }
}

#[cfg(target_os = "macos")]
fn confstr_path(name: libc::c_int) -> Option<PathBuf> {
    use std::ffi::CStr;

    let mut buf = vec![0_i8; (libc::PATH_MAX as usize) + 1];
    let len = unsafe { libc::confstr(name, buf.as_mut_ptr(), buf.len()) };
    if len == 0 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
    let path = PathBuf::from(cstr.to_str().ok()?);
    path.canonicalize().ok().or(Some(path))
}

#[cfg(target_os = "macos")]
fn macos_dir_params() -> Vec<(String, PathBuf)> {
    if let Some(path) = confstr_path(libc::_CS_DARWIN_USER_CACHE_DIR) {
        return vec![("DARWIN_USER_CACHE_DIR".to_string(), path)];
    }
    vec![]
}

#[cfg(not(target_os = "macos"))]
fn macos_dir_params() -> Vec<(String, PathBuf)> {
    vec![]
}

pub(crate) fn create_seatbelt_command_args(
    command: Vec<String>,
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
) -> Vec<String> {
    let (file_write_policy, file_write_dir_params) = if sandbox_policy.has_full_disk_write_access()
    {
        (
            r#"(allow file-write* (regex #"^/"))"#.to_string(),
            Vec::new(),
        )
    } else {
        build_seatbelt_access_policy(
            "file-write*",
            "WRITABLE_ROOT",
            sandbox_policy
                .get_writable_roots_with_cwd(cwd)
                .into_iter()
                .map(|root| SeatbeltAccessRoot {
                    root: root.root,
                    recursive: root.recursive,
                    excluded_subpaths: root.read_only_subpaths,
                })
                .collect(),
        )
    };

    let (file_read_policy, file_read_dir_params) = if sandbox_policy.has_full_disk_read_access() {
        (
            "; allow read-only file operations\n(allow file-read*)".to_string(),
            Vec::new(),
        )
    } else {
        let (policy, params) = build_seatbelt_access_policy(
            "file-read*",
            "READABLE_ROOT",
            sandbox_policy
                .get_readable_roots_with_cwd(cwd)
                .into_iter()
                .map(|root| SeatbeltAccessRoot {
                    root,
                    recursive: true,
                    excluded_subpaths: Vec::new(),
                })
                .collect(),
        );
        if policy.is_empty() {
            (String::new(), params)
        } else {
            (
                format!("; allow read-only file operations\n{policy}"),
                params,
            )
        }
    };

    let network_policy = if sandbox_policy.has_full_network_access() {
        "(allow network-outbound)\n(allow network-inbound)\n(allow system-socket)".to_string()
    } else {
        String::new()
    };

    let mut policy_sections = vec![
        MACOS_SEATBELT_BASE_POLICY.to_string(),
        file_read_policy,
        file_write_policy,
        network_policy,
    ];
    if !sandbox_policy.has_full_disk_read_access() {
        policy_sections.push(MACOS_RESTRICTED_READ_ONLY_PLATFORM_DEFAULTS.to_string());
    }
    let full_policy = policy_sections.join("\n");

    let dir_params = [
        file_read_dir_params,
        file_write_dir_params,
        macos_dir_params(),
    ]
    .concat();
    let mut seatbelt_args = vec!["-p".to_string(), full_policy];
    seatbelt_args.extend(
        dir_params
            .into_iter()
            .map(|(key, value)| format!("-D{key}={}", value.to_string_lossy())),
    );
    seatbelt_args.push("--".to_string());
    seatbelt_args.extend(command);
    seatbelt_args
}

#[cfg(test)]
mod tests {
    use super::create_seatbelt_command_args;
    use crate::protocol::{NetworkAccess, SandboxPolicy, WritableRoot};
    use std::fs;
    use std::path::{Path, PathBuf};
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
        fs::create_dir_all(cwd.join(".git")).expect("create cwd .git");
        let cwd_canon = cwd.canonicalize().expect("canonicalize cwd");
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
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };
        let args = create_seatbelt_command_args(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            &policy,
            &cwd,
        );
        let policy_text = &args[1];
        assert!(policy_text.contains("(allow file-write*"));
        assert!(policy_text.contains("(allow file-read*"));
        assert!(
            policy_text.contains("(require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_0\")))")
        );
        assert!(
            policy_text.contains("(require-not (subpath (param \"WRITABLE_ROOT_0_EXCLUDED_0\")))")
        );
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_0={}",
            root_with_git_canon.to_string_lossy()
        )));
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_0_EXCLUDED_0={}",
            root_with_git_git_canon.to_string_lossy()
        )));
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_1={}",
            root_without_git_canon.to_string_lossy()
        )));
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_2={}",
            cwd_canon.to_string_lossy()
        )));
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_2_EXCLUDED_0={}",
            cwd_canon.join(".git").to_string_lossy()
        )));
        assert_eq!(args[args.len() - 3], "--");
        assert_eq!(args[args.len() - 2], "/bin/echo");
        assert_eq!(args[args.len() - 1], "hello");
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
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let args = create_seatbelt_command_args(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            &policy,
            root_with_git.as_path(),
        );
        let policy_text = &args[1];
        assert!(policy_text.contains("(allow file-write*"));
        assert!(
            policy_text.contains("(require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_0\")))")
        );
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_0={}",
            root_with_git_canon.to_string_lossy()
        )));
        assert!(args.contains(&format!(
            "-DWRITABLE_ROOT_0_EXCLUDED_0={}",
            root_with_git_git_canon.to_string_lossy()
        )));
        assert!(args.iter().any(|arg| arg.starts_with("-DWRITABLE_ROOT_1=")));
        assert_eq!(args[args.len() - 3], "--");
        assert_eq!(args[args.len() - 2], "/bin/echo");
        assert_eq!(args[args.len() - 1], "hello");
    }

    #[test]
    fn create_seatbelt_args_for_full_access_policy() {
        if cfg!(target_os = "windows") {
            return;
        }
        let cwd = PathBuf::from("/");
        let args = create_seatbelt_command_args(
            vec!["/bin/echo".to_string(), "hello".to_string()],
            &SandboxPolicy::DangerFullAccess,
            &cwd,
        );

        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(allow file-read*)"));
        assert!(args[1].contains("(allow file-write* (regex #\"^/\"))"));
        assert!(args[1].contains("(allow network-outbound)"));
        assert!(args[1].contains("(allow network-inbound)"));
        assert!(args[1].contains("(allow system-socket)"));
        assert_eq!(args[args.len() - 3], "--");
        assert_eq!(args[args.len() - 2], "/bin/echo");
        assert_eq!(args[args.len() - 1], "hello");
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
