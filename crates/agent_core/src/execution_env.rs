use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub(crate) struct ExecutionEnvironment {
    private_workspace: PathBuf,
    managed_tmp: PathBuf,
    current_checkpoint: Option<FsCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct FsCheckpointId(pub(crate) String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FsCheckpoint {
    pub(crate) id: FsCheckpointId,
    pub(crate) manifest: FsManifest,
    pub(crate) created_at: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FsManifest {
    pub(crate) entries: BTreeMap<PathBuf, FsEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FsEntry {
    pub(crate) kind: FsEntryKind,
    pub(crate) size: u64,
    pub(crate) hash: Option<String>,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FsEntryKind {
    File,
    Directory,
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct FsDelta {
    pub(crate) changes: Vec<FsChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) enum FsChange {
    Created {
        path: PathBuf,
        entry: FsEntry,
    },
    Modified {
        path: PathBuf,
        before: FsEntry,
        after: FsEntry,
    },
    Deleted {
        path: PathBuf,
        before: FsEntry,
    },
    TypeChanged {
        path: PathBuf,
        before: FsEntry,
        after: FsEntry,
    },
}

pub(crate) fn isolated_execution_enabled() -> bool {
    std::env::var("NITE_ISOLATED_EXECUTION_ROOT")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

impl ExecutionEnvironment {
    pub(crate) fn initialize(real_workspace: PathBuf) -> Result<Self> {
        let real_workspace = real_workspace
            .canonicalize()
            .with_context(|| format!("resolve workspace root {}", real_workspace.display()))?;
        let base = std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let private_workspace = base.join("workspace");
        let managed_tmp = base.join("tmp");
        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        copy_tree(&real_workspace, &private_workspace)?;

        let mut env = Self {
            private_workspace,
            managed_tmp,
            current_checkpoint: None,
        };
        let checkpoint = env.checkpoint_agent_fs()?;
        env.current_checkpoint = Some(checkpoint);
        Ok(env)
    }

    pub(crate) fn private_workspace(&self) -> &Path {
        &self.private_workspace
    }

    pub(crate) fn env_overrides(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        let workspace = self.private_workspace.to_string_lossy().to_string();
        let tmp = self.managed_tmp.to_string_lossy().to_string();
        env.insert("NITE_WORKSPACE_ROOT".to_string(), workspace);
        env.insert("TMPDIR".to_string(), tmp.clone());
        env.insert("TMP".to_string(), tmp.clone());
        env.insert("TEMP".to_string(), tmp);
        env
    }

    pub(crate) fn checkpoint_agent_fs(&mut self) -> Result<FsCheckpoint> {
        let manifest = FsManifest::scan(&self.private_workspace)?;
        let checkpoint = FsCheckpoint {
            id: FsCheckpointId(uuid::Uuid::new_v4().to_string()),
            manifest,
            created_at: SystemTime::now(),
        };
        self.current_checkpoint = Some(checkpoint.clone());
        Ok(checkpoint)
    }
}

impl FsManifest {
    pub(crate) fn scan(root: &Path) -> Result<Self> {
        let mut entries = BTreeMap::new();
        scan_path(root, root, &mut entries)?;
        Ok(Self { entries })
    }

    #[allow(dead_code)]
    pub(crate) fn diff(&self, other: &Self) -> FsDelta {
        let mut changes = Vec::new();
        for (path, before) in &self.entries {
            match other.entries.get(path) {
                Some(after) if before.kind != after.kind => changes.push(FsChange::TypeChanged {
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }),
                Some(after) if before.hash != after.hash || before.size != after.size => {
                    changes.push(FsChange::Modified {
                        path: path.clone(),
                        before: before.clone(),
                        after: after.clone(),
                    });
                }
                Some(_) => {}
                None => changes.push(FsChange::Deleted {
                    path: path.clone(),
                    before: before.clone(),
                }),
            }
        }
        for (path, entry) in &other.entries {
            if !self.entries.contains_key(path) {
                changes.push(FsChange::Created {
                    path: path.clone(),
                    entry: entry.clone(),
                });
            }
        }
        FsDelta { changes }
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(src)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(relative);
        let file_type = entry.file_type();
        if file_type.is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("create directory {}", target.display()))?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(path, &target)
                .with_context(|| format!("copy {} to {}", path.display(), target.display()))?;
        } else if file_type.is_symlink() {
            let link_target = std::fs::read_link(path)
                .with_context(|| format!("read symlink {}", path.display()))?;
            create_symlink(&link_target, &target)?;
        }
    }
    Ok(())
}

fn scan_path(root: &Path, current: &Path, entries: &mut BTreeMap<PathBuf, FsEntry>) -> Result<()> {
    for entry in std::fs::read_dir(current)
        .with_context(|| format!("read directory {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?.to_path_buf();
        let metadata = std::fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            FsEntryKind::Symlink {
                target: std::fs::read_link(&path)?,
            }
        } else if file_type.is_dir() {
            FsEntryKind::Directory
        } else {
            FsEntryKind::File
        };
        let hash = if matches!(kind, FsEntryKind::File) {
            Some(hash_file(&path)?)
        } else {
            None
        };
        entries.insert(
            relative,
            FsEntry {
                kind: kind.clone(),
                size: metadata.len(),
                hash,
                modified: metadata.modified().ok(),
                readonly: metadata.permissions().readonly(),
            },
        );
        if matches!(kind, FsEntryKind::Directory) {
            scan_path(root, &path, entries)?;
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("hash file {}", path.display()))?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("create symlink {}", link.display()))
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| format!("create symlink {}", link.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-core-exec-env-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn manifest_diff_detects_created_modified_deleted_files() {
        let temp = make_test_dir("manifest-diff");
        std::fs::write(temp.join("modified.txt"), "before").expect("write modified");
        std::fs::write(temp.join("deleted.txt"), "delete me").expect("write deleted");

        let env = ExecutionEnvironment::initialize(temp).expect("init env");
        let before = FsManifest::scan(env.private_workspace()).expect("scan before");
        std::fs::write(env.private_workspace().join("modified.txt"), "after").expect("modify file");
        std::fs::remove_file(env.private_workspace().join("deleted.txt")).expect("delete file");
        std::fs::write(env.private_workspace().join("created.txt"), "new").expect("create file");
        let after = FsManifest::scan(env.private_workspace()).expect("scan after");

        let delta = before.diff(&after);
        assert!(delta.changes.iter().any(|change| matches!(change, FsChange::Created { path, .. } if path == Path::new("created.txt"))));
        assert!(delta.changes.iter().any(|change| matches!(change, FsChange::Modified { path, .. } if path == Path::new("modified.txt"))));
        assert!(delta.changes.iter().any(|change| matches!(change, FsChange::Deleted { path, .. } if path == Path::new("deleted.txt"))));
    }
}
