use crate::protocol::SandboxPolicy;
use std::env;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const SYSTEM_PATHS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc",
    "/nix/store",
    "/run/current-system/sw",
];

pub fn find_system_bwrap_in_path() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let current_dir = env::current_dir().ok();
    for dir in env::split_paths(&path) {
        let candidate = dir.join("bwrap");
        if !candidate.is_file() {
            continue;
        }
        if let Some(cwd) = &current_dir {
            if candidate.starts_with(cwd) {
                continue;
            }
        }
        return Some(candidate);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemBwrap {
    pub program: PathBuf,
    pub supports_argv0: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BubblewrapLauncher {
    System(SystemBwrap),
    Packaged(PathBuf),
}

impl BubblewrapLauncher {
    pub fn program(&self) -> &Path {
        match self {
            Self::System(system) => system.program.as_path(),
            Self::Packaged(path) => path.as_path(),
        }
    }

    pub fn supports_argv0(&self) -> bool {
        match self {
            Self::System(system) => system.supports_argv0,
            Self::Packaged(_) => true,
        }
    }
}

pub fn preferred_system_bwrap() -> Option<SystemBwrap> {
    static BWRAP: OnceLock<Option<SystemBwrap>> = OnceLock::new();
    BWRAP
        .get_or_init(|| {
            let path = find_system_bwrap_in_path()?;
            preferred_system_bwrap_for_path(
                &path,
                system_bwrap_responds,
                system_bwrap_supports_argv0,
            )
        })
        .clone()
}

pub fn preferred_bwrap_launcher(current_exe: Option<&Path>) -> Option<BubblewrapLauncher> {
    if let Some(system) = preferred_system_bwrap() {
        return Some(BubblewrapLauncher::System(system));
    }

    resolve_packaged_bwrap(current_exe).map(BubblewrapLauncher::Packaged)
}

fn system_bwrap_responds(system_bwrap_path: &Path) -> bool {
    Command::new(system_bwrap_path)
        .arg("--help")
        .output()
        .is_ok()
}

fn system_bwrap_supports_argv0(system_bwrap_path: &Path) -> bool {
    let output = match Command::new(system_bwrap_path).arg("--help").output() {
        Ok(output) => output,
        Err(_) => return false,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("--argv0") || stderr.contains("--argv0")
}

fn resolve_packaged_bwrap(current_exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = env::var_os("COLOSSAL_BWRAP_PATH") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let Some(current_exe) = current_exe else {
        return None;
    };
    let bin_dir = current_exe.parent()?;
    let candidates = [
        bin_dir.join("colossal-bwrap"),
        bin_dir.join("bwrap"),
        bin_dir.join("libexec").join("colossal-bwrap"),
        bin_dir.join("libexec").join("bwrap"),
    ];
    candidates.into_iter().find(|candidate| candidate.is_file())
}

pub fn create_bwrap_command_args(
    sandbox_policy: &SandboxPolicy,
    cwd: &Path,
    command: &[String],
) -> Vec<String> {
    let full_filesystem = sandbox_policy.has_full_disk_read_access();
    let mut args = vec![
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-user".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
    ];
    if !sandbox_policy.has_full_network_access() {
        args.push("--unshare-net".to_string());
    }
    if full_filesystem {
        args.push("--ro-bind".to_string());
        args.push("/".to_string());
        args.push("/".to_string());
    } else {
        args.push("--tmpfs".to_string());
        args.push("/".to_string());
    }
    if matches!(sandbox_policy, SandboxPolicy::ReadOnly) {
        for dev in &[
            "/dev/null",
            "/dev/zero",
            "/dev/urandom",
            "/dev/random",
            "/dev/tty",
        ] {
            if Path::new(dev).exists() {
                args.push("--dev-bind".to_string());
                args.push(dev.to_string());
                args.push(dev.to_string());
            }
        }
    } else {
        args.extend(["--dev".to_string(), "/dev".to_string()]);
    }
    // Nushell and other tools expect procfs to exist even in read-only sandboxes.
    // Bubblewrap's --proc mount is read-only enough for this use and restores
    // behavior that regressed when ReadOnly stopped mounting /proc entirely.
    args.extend(["--proc".to_string(), "/proc".to_string()]);

    for system_path in SYSTEM_PATHS {
        let path = Path::new(system_path);
        if path.exists() {
            args.push("--ro-bind".to_string());
            args.push(system_path.to_string());
            args.push(system_path.to_string());
        }
    }

    add_read_only_bind(args.as_mut(), Path::new(&command[0]));
    ensure_path_parents_visible(&mut args, Path::new(&command[0]), false);

    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => {
            args.clear();
            args.extend([
                "--die-with-parent".to_string(),
                "--new-session".to_string(),
                "--bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                "--unshare-user".to_string(),
                "--unshare-pid".to_string(),
                "--proc".to_string(),
                "/proc".to_string(),
            ]);
        }
        SandboxPolicy::ReadOnly => {
            ensure_path_parents_visible(&mut args, cwd, true);
            add_read_only_bind(&mut args, cwd);
            args.push("--remount-ro".to_string());
            args.push("/".to_string());
        }
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            exclude_tmpdir_env_var,
            exclude_slash_tmp,
            ..
        } => {
            add_read_write_bind(&mut args, cwd);
            for root in writable_roots {
                add_read_write_bind(&mut args, &root.root);
                for subpath in &root.read_only_subpaths {
                    add_read_only_bind(&mut args, subpath);
                }
            }
            if !exclude_slash_tmp {
                add_read_write_bind(&mut args, Path::new("/tmp"));
            }
            if !exclude_tmpdir_env_var {
                if let Ok(tmpdir) = env::var("TMPDIR") {
                    add_read_write_bind(&mut args, Path::new(&tmpdir));
                }
            }
        }
    }

    args.push("--chdir".to_string());
    args.push(cwd.to_string_lossy().to_string());
    args.push("--".to_string());
    args.extend(command.iter().cloned());
    args
}

fn ensure_path_parents_visible(args: &mut Vec<String>, path: &Path, lock_down: bool) {
    if !path.is_absolute() {
        return;
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(component),
            Component::Normal(part) => {
                current.push(part);
                if current == path {
                    break;
                }
                // Already covered transitively by a system-path bind, or is an ancestor
                // of one that bwrap will create implicitly — skip to avoid exposing siblings.
                if SYSTEM_PATHS
                    .iter()
                    .any(|sp| current.starts_with(sp) || Path::new(sp).starts_with(&current))
                {
                    continue;
                }
                // Use --dir to create an empty traversable node rather than --ro-bind,
                // which would expose the directory's contents (sibling files/dirs).
                args.push("--dir".to_string());
                args.push(current.to_string_lossy().to_string());
                if lock_down {
                    args.push("--chmod".to_string());
                    args.push("0555".to_string());
                    args.push(current.to_string_lossy().to_string());
                }
            }
            _ => {}
        }
    }
}

fn add_read_only_bind(args: &mut Vec<String>, path: &Path) {
    if !path.exists() {
        return;
    }
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    let rendered = canonical.to_string_lossy().to_string();
    args.push("--ro-bind".to_string());
    args.push(rendered.clone());
    args.push(rendered);
}

fn add_read_write_bind(args: &mut Vec<String>, path: &Path) {
    if !path.exists() {
        return;
    }
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    let rendered = canonical.to_string_lossy().to_string();
    args.push("--bind".to_string());
    args.push(rendered.clone());
    args.push(rendered);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{NetworkAccess, WritableRoot};

    #[test]
    fn readonly_policy_uses_ro_bind_for_cwd() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(&SandboxPolicy::ReadOnly, temp.path(), &command);

        let rendered = temp.path().to_string_lossy().to_string();
        assert!(
            args.windows(3)
                .any(|window| window == ["--ro-bind", rendered.as_str(), rendered.as_str()])
        );
    }

    #[test]
    fn readonly_policy_locks_down_synthetic_cwd_ancestors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("nested").join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(&SandboxPolicy::ReadOnly, &workspace, &command);

        let parent = workspace
            .parent()
            .expect("workspace parent")
            .canonicalize()
            .expect("canonical parent");
        let rendered = parent.to_string_lossy().to_string();

        assert!(
            args.windows(2)
                .any(|window| window == ["--dir", rendered.as_str()]),
            "expected readonly cwd ancestor to be created as traversable dir"
        );
        assert!(
            args.windows(3)
                .any(|window| window == ["--chmod", "0555", rendered.as_str()]),
            "expected readonly cwd ancestor to be non-writable"
        );
    }

    #[test]
    fn workspace_write_policy_binds_writable_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let writable = temp.path().join("workspace");
        std::fs::create_dir(&writable).expect("workspace dir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(
            &SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![WritableRoot {
                    root: writable.clone(),
                    recursive: true,
                    read_only_subpaths: vec![],
                }],
                network_access: NetworkAccess::Restricted,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            },
            temp.path(),
            &command,
        );

        let rendered = writable.to_string_lossy().to_string();
        assert!(
            args.windows(3)
                .any(|window| window == ["--bind", rendered.as_str(), rendered.as_str()])
        );
    }

    #[test]
    fn missing_bwrap_path_falls_back_cleanly() {
        assert!(
            preferred_system_bwrap_for_path(
                Path::new("/definitely/not/a/bwrap"),
                |_| true,
                |_| true,
            )
            .is_none()
        );
    }

    #[test]
    fn captures_argv0_support_from_probe() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        let selected = preferred_system_bwrap_for_path(temp.path(), |_| true, |_| false)
            .expect("system bwrap");
        assert!(!selected.supports_argv0);
    }

    #[test]
    fn falls_back_to_packaged_bwrap_next_to_helper() {
        let temp = tempfile::tempdir().expect("tempdir");
        let helper = temp.path().join("colossal-sandbox-helper");
        let packaged = temp.path().join("colossal-bwrap");
        std::fs::write(&helper, "").expect("helper stub");
        std::fs::write(&packaged, "").expect("packaged bwrap stub");

        let launcher = preferred_bwrap_launcher_for_paths(None, Some(helper.as_path()));
        assert_eq!(launcher, Some(BubblewrapLauncher::Packaged(packaged)));
    }

    #[test]
    fn readonly_policy_adds_tmpfs_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(&SandboxPolicy::ReadOnly, temp.path(), &command);

        assert!(args.contains(&"--tmpfs".to_string()));
        assert!(args.contains(&"/".to_string()));
    }

    #[test]
    fn readonly_policy_remounts_root_readonly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(&SandboxPolicy::ReadOnly, temp.path(), &command);

        assert!(
            args.windows(2)
                .any(|window| window == ["--remount-ro", "/"]),
            "expected readonly sandbox to remount synthetic root readonly"
        );
    }

    #[test]
    fn danger_full_access_policy_uses_bind_not_tmpfs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args =
            create_bwrap_command_args(&SandboxPolicy::DangerFullAccess, temp.path(), &command);

        assert!(!args.contains(&"--tmpfs".to_string()));
        assert!(args.contains(&"--bind".to_string()));
    }

    #[test]
    fn workspace_write_includes_protected_subpaths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let protected = workspace.join(".git");
        std::fs::create_dir_all(&protected).expect("create protected dir");

        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(
            &SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![WritableRoot {
                    root: workspace.clone(),
                    recursive: true,
                    read_only_subpaths: vec![protected.clone()],
                }],
                network_access: NetworkAccess::Restricted,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            },
            temp.path(),
            &command,
        );

        let rendered = protected.to_string_lossy().to_string();
        assert!(
            args.windows(3)
                .any(|window| window == ["--ro-bind", rendered.as_str(), rendered.as_str()]),
            "Protected subpath should be read-only"
        );
    }

    #[test]
    fn network_restricted_includes_unshare_net() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(
            &SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![],
                network_access: NetworkAccess::Restricted,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            },
            temp.path(),
            &command,
        );

        assert!(args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn network_enabled_excludes_unshare_net() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = vec!["/bin/echo".to_string(), "hi".to_string()];
        let args = create_bwrap_command_args(
            &SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![],
                network_access: NetworkAccess::Enabled,
                exclude_tmpdir_env_var: true,
                exclude_slash_tmp: true,
            },
            temp.path(),
            &command,
        );

        assert!(!args.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn find_system_bwrap_returns_none_for_missing() {
        let result = find_system_bwrap_in_path();
        // Either finds bwrap or returns None - both are valid
        // This test ensures the function doesn't panic
        let _ = result;
    }

    #[test]
    fn preferred_bwrap_launcher_handles_none() {
        let result = preferred_bwrap_launcher(None);
        // Either finds bwrap or returns None - both are valid
        let _ = result;
    }
}

#[cfg(test)]
fn preferred_bwrap_launcher_for_paths(
    system_bwrap: Option<SystemBwrap>,
    current_exe: Option<&Path>,
) -> Option<BubblewrapLauncher> {
    if let Some(system) = system_bwrap {
        return Some(BubblewrapLauncher::System(system));
    }
    resolve_packaged_bwrap(current_exe).map(BubblewrapLauncher::Packaged)
}

fn preferred_system_bwrap_for_path(
    path: &Path,
    availability_probe: impl FnOnce(&Path) -> bool,
    argv0_probe: impl FnOnce(&Path) -> bool,
) -> Option<SystemBwrap> {
    if !path.is_file() {
        return None;
    }
    if !availability_probe(path) {
        return None;
    }
    Some(SystemBwrap {
        program: path.to_path_buf(),
        supports_argv0: argv0_probe(path),
    })
}
