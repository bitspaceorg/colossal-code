use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    /// The sandbox is disabled and the agent has full access to the system.
    DangerFullAccess,
    /// The sandbox is enabled and the agent has read-only access to the workspace.
    ReadOnly,
    /// The agent has write access to the workspace.
    WorkspaceWrite,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkAccess {
    /// The agent has full network access.
    Enabled,
    /// The agent has restricted network access.
    Restricted,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WritableRoot {
    /// The root path that the agent has write access to.
    pub root: PathBuf,
    /// Whether the agent has write access to the subdirectories of the root.
    pub recursive: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxPolicy {
    /// The sandbox is disabled and the agent has full access to the system.
    DangerFullAccess,
    /// The sandbox is enabled and the agent has read-only access to the workspace.
    ReadOnly,
    /// The agent has write access to the workspace.
    WorkspaceWrite {
        writable_roots: Vec<WritableRoot>,
        network_access: NetworkAccess,
        #[serde(default)]
        exclude_tmpdir_env_var: bool,
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

impl SandboxPolicy {
    pub fn new_read_only_policy() -> Self {
        Self::ReadOnly
    }

    pub fn new_workspace_write_policy() -> Self {
        Self::WorkspaceWrite {
            writable_roots: vec![],
            network_access: NetworkAccess::Restricted,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }
    }

    pub fn has_full_network_access(&self) -> bool {
        matches!(
            self,
            Self::DangerFullAccess
                | Self::WorkspaceWrite {
                    network_access: NetworkAccess::Enabled,
                    ..
                }
        )
    }

    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }

    pub fn has_full_disk_read_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }

    pub fn get_writable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<WritableRoot> {
        match self {
            Self::DangerFullAccess => {
                vec![WritableRoot {
                    root: "/".into(),
                    recursive: true,
                }]
            }
            Self::ReadOnly => vec![],
            Self::WorkspaceWrite { writable_roots, .. } => {
                let mut roots = writable_roots.clone();
                roots.push(WritableRoot {
                    root: cwd.to_path_buf(),
                    recursive: true,
                });
                roots
            }
        }
    }

    pub fn get_readable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<PathBuf> {
        match self {
            Self::DangerFullAccess => vec!["/".into()],
            Self::ReadOnly | Self::WorkspaceWrite { .. } => {
                vec![cwd.to_path_buf()]
            }
        }
    }
}
