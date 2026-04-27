use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub(crate) struct ExecutionEnvironment {
    real_workspace: PathBuf,
    private_workspace: PathBuf,
    managed_tmp: PathBuf,
    baseline_manifest: FsManifest,
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
pub(crate) struct FsDelta {
    pub(crate) changes: Vec<FsChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug)]
pub struct ApplyConflict {
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ApplyResult {
    pub applied_paths: Vec<PathBuf>,
    pub conflicts: Vec<ApplyConflict>,
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
        let baseline_manifest = FsManifest::scan(&real_workspace)?;
        let base = std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let private_workspace = base.join("workspace");
        let managed_tmp = base.join("tmp");
        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        copy_tree(&real_workspace, &private_workspace)?;

        let mut env = Self {
            real_workspace,
            private_workspace,
            managed_tmp,
            baseline_manifest,
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

    pub(crate) fn pending_change_count(&self) -> Result<usize> {
        let private_manifest = FsManifest::scan(&self.private_workspace)?;
        Ok(self.baseline_manifest.diff(&private_manifest).changes.len())
    }

    pub(crate) fn apply_to_real_workspace(&mut self) -> Result<ApplyResult> {
        let private_manifest = FsManifest::scan(&self.private_workspace)?;
        let real_manifest = FsManifest::scan(&self.real_workspace)?;
        let agent_delta = self.baseline_manifest.diff(&private_manifest);
        let real_delta = self.baseline_manifest.diff(&real_manifest);
        let conflicts = detect_conflicts(&agent_delta, &real_delta);
        if !conflicts.is_empty() {
            return Ok(ApplyResult {
                applied_paths: Vec::new(),
                conflicts,
            });
        }

        let mut applied_paths = Vec::new();
        for change in &agent_delta.changes {
            apply_change(change, &self.private_workspace, &self.real_workspace)?;
            applied_paths.push(change.path().to_path_buf());
        }

        self.baseline_manifest = private_manifest.clone();
        self.current_checkpoint = Some(FsCheckpoint {
            id: FsCheckpointId(uuid::Uuid::new_v4().to_string()),
            manifest: private_manifest,
            created_at: SystemTime::now(),
        });

        Ok(ApplyResult {
            applied_paths,
            conflicts: Vec::new(),
        })
    }

    pub(crate) fn discard_changes(&mut self) -> Result<()> {
        let refreshed_baseline = FsManifest::scan(&self.real_workspace)?;
        if self.private_workspace.exists() {
            std::fs::remove_dir_all(&self.private_workspace).with_context(|| {
                format!(
                    "remove private workspace {}",
                    self.private_workspace.display()
                )
            })?;
        }
        std::fs::create_dir_all(&self.private_workspace).with_context(|| {
            format!(
                "recreate private workspace {}",
                self.private_workspace.display()
            )
        })?;
        copy_tree(&self.real_workspace, &self.private_workspace)?;
        self.baseline_manifest = refreshed_baseline;
        self.checkpoint_agent_fs()?;
        Ok(())
    }
}

impl FsManifest {
    pub(crate) fn scan(root: &Path) -> Result<Self> {
        let mut entries = BTreeMap::new();
        scan_path(root, root, &mut entries)?;
        Ok(Self { entries })
    }

    pub(crate) fn diff(&self, other: &Self) -> FsDelta {
        let mut changes = Vec::new();
        for (path, before) in &self.entries {
            match other.entries.get(path) {
                Some(after) if before.kind != after.kind => changes.push(FsChange::TypeChanged {
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }),
                Some(after) if before.content_changed(after) => {
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

impl FsEntry {
    fn content_changed(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (FsEntryKind::File, FsEntryKind::File) => {
                self.hash != other.hash
                    || self.size != other.size
                    || self.readonly != other.readonly
            }
            (FsEntryKind::Symlink { target: left }, FsEntryKind::Symlink { target: right }) => {
                left != right || self.readonly != other.readonly
            }
            (FsEntryKind::Directory, FsEntryKind::Directory) => self.readonly != other.readonly,
            _ => true,
        }
    }
}

impl FsChange {
    fn path(&self) -> &Path {
        match self {
            FsChange::Created { path, .. }
            | FsChange::Modified { path, .. }
            | FsChange::Deleted { path, .. }
            | FsChange::TypeChanged { path, .. } => path,
        }
    }
}

fn detect_conflicts(agent_delta: &FsDelta, real_delta: &FsDelta) -> Vec<ApplyConflict> {
    let mut conflicts = Vec::new();
    for agent_change in &agent_delta.changes {
        for real_change in &real_delta.changes {
            if paths_overlap(agent_change.path(), real_change.path()) {
                conflicts.push(ApplyConflict {
                    path: overlapping_path(agent_change.path(), real_change.path()),
                });
            }
        }
    }
    conflicts
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn overlapping_path(left: &Path, right: &Path) -> PathBuf {
    if left.starts_with(right) {
        right.to_path_buf()
    } else {
        left.to_path_buf()
    }
}

fn apply_change(change: &FsChange, src_root: &Path, dst_root: &Path) -> Result<()> {
    match change {
        FsChange::Created { path, .. } | FsChange::Modified { path, .. } => {
            copy_path(src_root, dst_root, path)
        }
        FsChange::Deleted { path, .. } => remove_path(dst_root, path),
        FsChange::TypeChanged { path, .. } => {
            remove_path(dst_root, path)?;
            copy_path(src_root, dst_root, path)
        }
    }
}

fn copy_path(src_root: &Path, dst_root: &Path, relative: &Path) -> Result<()> {
    let src = src_root.join(relative);
    let dst = dst_root.join(relative);
    let metadata = std::fs::symlink_metadata(&src)
        .with_context(|| format!("inspect source {}", src.display()))?;
    let file_type = metadata.file_type();

    if file_type.is_dir() {
        std::fs::create_dir_all(&dst)
            .with_context(|| format!("create directory {}", dst.display()))?;
        return Ok(());
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent {}", parent.display()))?;
    }

    if file_type.is_symlink() {
        if dst.exists() {
            remove_existing_path(&dst)?;
        }
        let link_target =
            std::fs::read_link(&src).with_context(|| format!("read symlink {}", src.display()))?;
        create_symlink(&link_target, &dst)
    } else {
        std::fs::copy(&src, &dst)
            .with_context(|| format!("copy {} to {}", src.display(), dst.display()))?;
        Ok(())
    }
}

fn remove_path(root: &Path, relative: &Path) -> Result<()> {
    let target = root.join(relative);
    if !target.exists() && std::fs::symlink_metadata(&target).is_err() {
        return Ok(());
    }
    remove_existing_path(&target)
}

fn remove_existing_path(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect path {}", path.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_dir() && !file_type.is_symlink() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("remove directory {}", path.display()))?;
    } else {
        std::fs::remove_file(path).with_context(|| format!("remove file {}", path.display()))?;
    }
    Ok(())
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

    #[test]
    fn apply_to_real_workspace_creates_files() {
        let temp = make_test_dir("apply-create");
        let mut env = ExecutionEnvironment::initialize(temp.clone()).expect("init env");

        std::fs::write(env.private_workspace().join("created.txt"), "hello")
            .expect("create private file");

        let result = env.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(temp.join("created.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn apply_to_real_workspace_modifies_files() {
        let temp = make_test_dir("apply-modify");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut env = ExecutionEnvironment::initialize(temp.clone()).expect("init env");

        std::fs::write(env.private_workspace().join("file.txt"), "after")
            .expect("modify private file");

        let result = env.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "after"
        );
    }

    #[test]
    fn apply_to_real_workspace_deletes_files() {
        let temp = make_test_dir("apply-delete");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut env = ExecutionEnvironment::initialize(temp.clone()).expect("init env");

        std::fs::remove_file(env.private_workspace().join("file.txt"))
            .expect("delete private file");

        let result = env.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert!(!temp.join("file.txt").exists());
    }

    #[test]
    fn apply_to_real_workspace_blocks_on_real_workspace_drift() {
        let temp = make_test_dir("apply-conflict");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut env = ExecutionEnvironment::initialize(temp.clone()).expect("init env");

        std::fs::write(env.private_workspace().join("file.txt"), "agent")
            .expect("modify private file");
        std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

        let result = env.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, PathBuf::from("file.txt"));
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "user"
        );
    }

    #[test]
    fn apply_to_real_workspace_allows_non_conflicting_parallel_changes() {
        let temp = make_test_dir("apply-non-conflict");
        std::fs::write(temp.join("agent.txt"), "before-agent").expect("seed agent file");
        std::fs::write(temp.join("user.txt"), "before-user").expect("seed user file");
        let mut env = ExecutionEnvironment::initialize(temp.clone()).expect("init env");

        std::fs::write(env.private_workspace().join("agent.txt"), "after-agent")
            .expect("modify private file");
        std::fs::write(temp.join("user.txt"), "after-user").expect("modify real file");

        let result = env.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(temp.join("agent.txt")).unwrap(),
            "after-agent"
        );
        assert_eq!(
            std::fs::read_to_string(temp.join("user.txt")).unwrap(),
            "after-user"
        );
    }
}
