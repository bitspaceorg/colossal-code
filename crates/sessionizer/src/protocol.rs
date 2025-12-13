use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkAccess {
    Restricted,
    Enabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritableRoot {
    pub root: PathBuf,
    pub recursive: bool,
    pub read_only_subpaths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxPolicy {
    /// Read-only mode: NO write access anywhere, only read access to workspace
    ReadOnly,
    /// Workspace write: Can write to workspace and specified roots
    WorkspaceWrite {
        writable_roots: Vec<WritableRoot>,
        network_access: NetworkAccess,
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    },
    /// Full access: No sandbox restrictions
    DangerFullAccess,
}

impl SandboxPolicy {
    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, SandboxPolicy::DangerFullAccess)
    }

    pub fn has_full_disk_read_access(&self) -> bool {
        matches!(self, SandboxPolicy::DangerFullAccess)
    }

    pub fn has_full_network_access(&self) -> bool {
        match self {
            SandboxPolicy::ReadOnly => false, // Restrict network in read-only mode
            SandboxPolicy::WorkspaceWrite { network_access, .. } => matches!(network_access, NetworkAccess::Enabled),
            SandboxPolicy::DangerFullAccess => true,
        }
    }

    pub fn get_writable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<WritableRoot> {
        match self {
            SandboxPolicy::ReadOnly => {
                // No writable roots in read-only mode
                vec![]
            }
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                ..
            } => {
                let mut roots = writable_roots.clone();
                roots.push(WritableRoot {
                    root: cwd.to_path_buf(),
                    recursive: true,
                    read_only_subpaths: vec![cwd.join(".git")],
                });
                if !exclude_slash_tmp {
                    roots.push(WritableRoot {
                        root: PathBuf::from("/tmp"),
                        recursive: true,
                        read_only_subpaths: vec![],
                    });
                }
                if !exclude_tmpdir_env_var {
                    if let Ok(tmpdir) = std::env::var("TMPDIR") {
                        roots.push(WritableRoot {
                            root: PathBuf::from(tmpdir),
                            recursive: true,
                            read_only_subpaths: vec![],
                        });
                    }
                }
                roots
            }
            SandboxPolicy::DangerFullAccess => vec![],
        }
    }

    /// Get readable roots for the sandbox (used for read-only mode)
    pub fn get_readable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<PathBuf> {
        match self {
            SandboxPolicy::ReadOnly | SandboxPolicy::WorkspaceWrite { .. } => {
                // Read access to workspace and common paths
                vec![
                    cwd.to_path_buf(),
                    PathBuf::from("/usr"),
                    PathBuf::from("/lib"),
                    PathBuf::from("/lib64"),
                    PathBuf::from("/etc"),
                    PathBuf::from("/bin"),
                    PathBuf::from("/sbin"),
                ]
            }
            SandboxPolicy::DangerFullAccess => vec![PathBuf::from("/")],
        }
    }
}
