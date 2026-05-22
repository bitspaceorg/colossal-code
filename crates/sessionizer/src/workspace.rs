use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceSessionId(pub String);

#[derive(Debug)]
pub struct SessionWorkspace {
    base_workspace: PathBuf,
    current_source_root: PathBuf,
    session_root: PathBuf,
    checkpoints_root: PathBuf,
    private_workspace: PathBuf,
    managed_tmp: PathBuf,
    baseline_manifest: FsManifest,
    current_checkpoint: Option<FsCheckpoint>,
    backend: WorkspaceBackend,
    audit_log: Vec<WorkspaceAuditEvent>,
    #[cfg(target_os = "linux")]
    audit_watcher: Option<RecommendedWatcher>,
    #[cfg(target_os = "linux")]
    audit_rx: Option<std::sync::mpsc::Receiver<notify::Result<Event>>>,
}

#[derive(Clone, Debug)]
enum WorkspaceBackend {
    Copy,
    #[cfg(target_os = "macos")]
    Clone,
    #[cfg(target_os = "windows")]
    WindowsCopy,
    #[cfg(target_os = "linux")]
    Overlay {
        upperdir: PathBuf,
        workdir: PathBuf,
        mount_program: PathBuf,
        mounted: bool,
    },
}

impl WorkspaceBackend {
    fn name(&self) -> &'static str {
        match self {
            Self::Copy => "copy",
            #[cfg(target_os = "macos")]
            Self::Clone => "clone",
            #[cfg(target_os = "windows")]
            Self::WindowsCopy => "windows-copy",
            #[cfg(target_os = "linux")]
            Self::Overlay { .. } => "overlay",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FsCheckpointId(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsCheckpoint {
    pub id: FsCheckpointId,
    pub manifest: FsManifest,
    pub created_at: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsManifest {
    pub entries: BTreeMap<PathBuf, FsEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsEntry {
    pub kind: FsEntryKind,
    pub size: u64,
    pub hash: Option<String>,
    pub modified: Option<SystemTime>,
    pub readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsEntryKind {
    File,
    Directory,
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsDelta {
    pub changes: Vec<FsChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FsChange {
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

#[derive(Clone, Debug)]
pub struct ExecutionReviewEntry {
    pub path: PathBuf,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceAuditEventKind {
    WorkspacePrepared,
    BackendSelected,
    CheckpointCreated,
    CommitApplied,
    CommitBlockedByConflicts,
    Discarded,
    FileEventObserved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAuditEvent {
    pub at: SystemTime,
    pub kind: WorkspaceAuditEventKind,
    pub message: String,
}

pub fn isolated_execution_enabled() -> bool {
    std::env::var("NITE_ISOLATED_EXECUTION_ROOT")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn workspace_env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

impl SessionWorkspace {
    pub fn initialize(base_workspace: PathBuf) -> Result<Self> {
        let base_workspace = base_workspace
            .canonicalize()
            .with_context(|| format!("resolve workspace root {}", base_workspace.display()))?;
        let baseline_manifest = FsManifest::scan(&base_workspace)?;

        #[cfg(target_os = "linux")]
        if !overlay_backend_forced_off() {
            if let Some(env) =
                Self::initialize_linux_overlay(base_workspace.clone(), baseline_manifest.clone())?
            {
                return Ok(env);
            }
        }

        #[cfg(target_os = "macos")]
        if !clone_backend_forced_off() {
            if let Some(env) =
                Self::initialize_macos_clone(base_workspace.clone(), baseline_manifest.clone())?
            {
                return Ok(env);
            }
        }

        #[cfg(target_os = "windows")]
        if !windows_copy_backend_forced_off() {
            return Self::initialize_windows_copy(base_workspace, baseline_manifest);
        }

        let session_root =
            std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let checkpoints_root =
            std::env::temp_dir().join(format!("nite-exec-checkpoints-{}", uuid::Uuid::new_v4()));
        let private_workspace = session_root.join("workspace");
        let managed_tmp = session_root.join("tmp");
        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        copy_tree(&base_workspace, &private_workspace)?;

        let mut env = Self {
            base_workspace: base_workspace.clone(),
            current_source_root: base_workspace.clone(),
            session_root,
            checkpoints_root,
            private_workspace,
            managed_tmp,
            baseline_manifest,
            current_checkpoint: None,
            backend: WorkspaceBackend::Copy,
            audit_log: Vec::new(),
            #[cfg(target_os = "linux")]
            audit_watcher: None,
            #[cfg(target_os = "linux")]
            audit_rx: None,
        };
        #[cfg(target_os = "linux")]
        env.start_linux_audit_watcher()?;
        env.record_audit(
            WorkspaceAuditEventKind::BackendSelected,
            format!("Selected workspace backend: {}", env.backend_name()),
        );
        env.record_audit(
            WorkspaceAuditEventKind::WorkspacePrepared,
            format!(
                "Prepared isolated workspace at {}",
                env.private_workspace.display()
            ),
        );
        let checkpoint = env.checkpoint_agent_fs()?;
        env.current_checkpoint = Some(checkpoint);
        Ok(env)
    }

    #[cfg(target_os = "windows")]
    fn initialize_windows_copy(
        base_workspace: PathBuf,
        baseline_manifest: FsManifest,
    ) -> Result<Self> {
        let session_root =
            std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let checkpoints_root =
            std::env::temp_dir().join(format!("nite-exec-checkpoints-{}", uuid::Uuid::new_v4()));
        let private_workspace = session_root.join("workspace");
        let managed_tmp = session_root.join("tmp");
        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        copy_tree(&base_workspace, &private_workspace)?;

        let mut env = Self {
            base_workspace: base_workspace.clone(),
            current_source_root: base_workspace.clone(),
            session_root,
            checkpoints_root,
            private_workspace,
            managed_tmp,
            baseline_manifest,
            current_checkpoint: None,
            backend: WorkspaceBackend::WindowsCopy,
            audit_log: Vec::new(),
            #[cfg(target_os = "linux")]
            audit_watcher: None,
            #[cfg(target_os = "linux")]
            audit_rx: None,
        };
        #[cfg(target_os = "linux")]
        env.start_linux_audit_watcher()?;
        env.record_audit(
            WorkspaceAuditEventKind::BackendSelected,
            format!("Selected workspace backend: {}", env.backend_name()),
        );
        env.record_audit(
            WorkspaceAuditEventKind::WorkspacePrepared,
            format!(
                "Prepared isolated workspace at {}",
                env.private_workspace.display()
            ),
        );
        let checkpoint = env.checkpoint_agent_fs()?;
        env.current_checkpoint = Some(checkpoint);
        Ok(env)
    }

    #[cfg(target_os = "macos")]
    fn initialize_macos_clone(
        base_workspace: PathBuf,
        baseline_manifest: FsManifest,
    ) -> Result<Option<Self>> {
        let session_root =
            std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let checkpoints_root =
            std::env::temp_dir().join(format!("nite-exec-checkpoints-{}", uuid::Uuid::new_v4()));
        let private_workspace = session_root.join("workspace");
        let managed_tmp = session_root.join("tmp");
        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        copy_tree_with_clone_fallback(&base_workspace, &private_workspace)?;

        let mut env = Self {
            base_workspace: base_workspace.clone(),
            current_source_root: base_workspace.clone(),
            session_root,
            checkpoints_root,
            private_workspace,
            managed_tmp,
            baseline_manifest,
            current_checkpoint: None,
            backend: WorkspaceBackend::Clone,
            audit_log: Vec::new(),
            #[cfg(target_os = "linux")]
            audit_watcher: None,
            #[cfg(target_os = "linux")]
            audit_rx: None,
        };
        #[cfg(target_os = "linux")]
        env.start_linux_audit_watcher()?;
        env.record_audit(
            WorkspaceAuditEventKind::BackendSelected,
            format!("Selected workspace backend: {}", env.backend_name()),
        );
        env.record_audit(
            WorkspaceAuditEventKind::WorkspacePrepared,
            format!(
                "Prepared isolated workspace at {}",
                env.private_workspace.display()
            ),
        );
        let checkpoint = env.checkpoint_agent_fs()?;
        env.current_checkpoint = Some(checkpoint);
        Ok(Some(env))
    }

    #[cfg(target_os = "linux")]
    fn initialize_linux_overlay(
        base_workspace: PathBuf,
        baseline_manifest: FsManifest,
    ) -> Result<Option<Self>> {
        let Some(mount_program) = find_overlay_mount_program() else {
            return Ok(None);
        };

        let session_root =
            std::env::temp_dir().join(format!("nite-exec-root-{}", uuid::Uuid::new_v4()));
        let checkpoints_root =
            std::env::temp_dir().join(format!("nite-exec-checkpoints-{}", uuid::Uuid::new_v4()));
        let private_workspace = session_root.join("workspace");
        let managed_tmp = session_root.join("tmp");
        let upperdir = session_root.join("upper");
        let workdir = session_root.join("work");

        std::fs::create_dir_all(&private_workspace)
            .with_context(|| format!("create private workspace {}", private_workspace.display()))?;
        std::fs::create_dir_all(&managed_tmp)
            .with_context(|| format!("create managed tmp {}", managed_tmp.display()))?;
        std::fs::create_dir_all(&upperdir)
            .with_context(|| format!("create upperdir {}", upperdir.display()))?;
        std::fs::create_dir_all(&workdir)
            .with_context(|| format!("create workdir {}", workdir.display()))?;

        if let Err(error) = mount_overlay(
            &mount_program,
            &base_workspace,
            &upperdir,
            &workdir,
            &private_workspace,
        ) {
            let _ = std::fs::remove_dir_all(&session_root);
            eprintln!(
                "overlay workspace setup failed, falling back to copy backend: {}",
                error
            );
            return Ok(None);
        }

        let mut env = Self {
            base_workspace: base_workspace.clone(),
            current_source_root: base_workspace.clone(),
            session_root,
            checkpoints_root,
            private_workspace,
            managed_tmp,
            baseline_manifest,
            current_checkpoint: None,
            backend: WorkspaceBackend::Overlay {
                upperdir,
                workdir,
                mount_program,
                mounted: true,
            },
            audit_log: Vec::new(),
            #[cfg(target_os = "linux")]
            audit_watcher: None,
            #[cfg(target_os = "linux")]
            audit_rx: None,
        };
        #[cfg(target_os = "linux")]
        env.start_linux_audit_watcher()?;
        env.record_audit(
            WorkspaceAuditEventKind::BackendSelected,
            format!("Selected workspace backend: {}", env.backend_name()),
        );
        env.record_audit(
            WorkspaceAuditEventKind::WorkspacePrepared,
            format!(
                "Prepared isolated workspace at {}",
                env.private_workspace.display()
            ),
        );
        let checkpoint = env.checkpoint_agent_fs()?;
        env.current_checkpoint = Some(checkpoint);
        Ok(Some(env))
    }

    pub fn private_workspace(&self) -> &Path {
        &self.private_workspace
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub fn audit_log(&mut self) -> &[WorkspaceAuditEvent] {
        self.drain_audit_events();
        &self.audit_log
    }

    pub fn record_audit(&mut self, kind: WorkspaceAuditEventKind, message: String) {
        self.audit_log.push(WorkspaceAuditEvent {
            at: SystemTime::now(),
            kind,
            message,
        });
    }

    pub fn destroy(mut self) -> Result<()> {
        self.teardown_backend()?;
        if self.checkpoints_root.exists() {
            std::fs::remove_dir_all(&self.checkpoints_root).with_context(|| {
                format!(
                    "remove checkpoints root {}",
                    self.checkpoints_root.display()
                )
            })?;
        }
        Ok(())
    }

    pub fn remap_workspace_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute()
            && let Ok(relative) = path.strip_prefix(&self.base_workspace)
        {
            return self.private_workspace.join(relative);
        }
        path.to_path_buf()
    }

    pub fn env_overrides(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        let workspace = self.private_workspace.to_string_lossy().to_string();
        let tmp = self.managed_tmp.to_string_lossy().to_string();
        env.insert("NITE_WORKSPACE_ROOT".to_string(), workspace);
        env.insert("TMPDIR".to_string(), tmp.clone());
        env.insert("TMP".to_string(), tmp.clone());
        env.insert("TEMP".to_string(), tmp);
        env
    }

    pub fn checkpoint_agent_fs(&mut self) -> Result<FsCheckpoint> {
        self.drain_audit_events();
        let manifest = FsManifest::scan(&self.private_workspace)?;
        let checkpoint = FsCheckpoint {
            id: FsCheckpointId(uuid::Uuid::new_v4().to_string()),
            manifest,
            created_at: SystemTime::now(),
        };
        let snapshot_root = self.checkpoint_snapshot_root(&checkpoint.id);
        if snapshot_root.exists() {
            std::fs::remove_dir_all(&snapshot_root)
                .with_context(|| format!("remove old checkpoint {}", snapshot_root.display()))?;
        }
        std::fs::create_dir_all(&snapshot_root)
            .with_context(|| format!("create checkpoint root {}", snapshot_root.display()))?;
        copy_tree(&self.private_workspace, &snapshot_root)?;
        std::fs::write(
            self.checkpoint_metadata_path(&checkpoint.id),
            serde_json::to_vec(&checkpoint)?,
        )
        .with_context(|| format!("write checkpoint metadata {}", checkpoint.id.0))?;
        self.current_checkpoint = Some(checkpoint.clone());
        self.record_audit(
            WorkspaceAuditEventKind::CheckpointCreated,
            format!("Created checkpoint {}", checkpoint.id.0),
        );
        Ok(checkpoint)
    }

    pub fn current_checkpoint(&self) -> Option<&FsCheckpoint> {
        self.current_checkpoint.as_ref()
    }

    pub fn pending_change_count(&mut self) -> Result<usize> {
        self.drain_audit_events();
        let private_manifest = FsManifest::scan(&self.private_workspace)?;
        Ok(self.baseline_manifest.diff(&private_manifest).changes.len())
    }

    pub fn review_entries(&mut self) -> Result<Vec<ExecutionReviewEntry>> {
        self.drain_audit_events();
        let private_manifest = FsManifest::scan(&self.private_workspace)?;
        let delta = self.baseline_manifest.diff(&private_manifest);
        let mut entries = Vec::new();

        for change in &delta.changes {
            entries.push(ExecutionReviewEntry {
                path: change.path().to_path_buf(),
                old_string: entry_snapshot_for_change(
                    change.before_entry(),
                    &self.base_workspace,
                    change.path(),
                )?,
                new_string: entry_snapshot_for_change(
                    change.after_entry(),
                    &self.private_workspace,
                    change.path(),
                )?,
            });
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    pub fn apply_to_real_workspace(&mut self) -> Result<ApplyResult> {
        self.drain_audit_events();
        let private_manifest = FsManifest::scan(&self.private_workspace)?;
        let real_manifest = FsManifest::scan(&self.base_workspace)?;
        let agent_delta = self.baseline_manifest.diff(&private_manifest);
        let real_delta = self.baseline_manifest.diff(&real_manifest);
        let conflicts = detect_conflicts(&agent_delta, &real_delta);
        if !conflicts.is_empty() {
            self.record_audit(
                WorkspaceAuditEventKind::CommitBlockedByConflicts,
                format!("Blocked commit due to {} conflict(s)", conflicts.len()),
            );
            return Ok(ApplyResult {
                applied_paths: Vec::new(),
                conflicts,
            });
        }

        let mut applied_paths = Vec::new();
        for change in &agent_delta.changes {
            apply_change(change, &self.private_workspace, &self.base_workspace)?;
            applied_paths.push(change.path().to_path_buf());
        }

        self.baseline_manifest = private_manifest.clone();
        self.current_checkpoint = Some(FsCheckpoint {
            id: FsCheckpointId(uuid::Uuid::new_v4().to_string()),
            manifest: private_manifest,
            created_at: SystemTime::now(),
        });
        self.record_audit(
            WorkspaceAuditEventKind::CommitApplied,
            format!("Applied {} path(s) to base workspace", applied_paths.len()),
        );

        Ok(ApplyResult {
            applied_paths,
            conflicts: Vec::new(),
        })
    }

    pub fn discard_changes(&mut self) -> Result<()> {
        self.drain_audit_events();
        let refreshed_baseline = FsManifest::scan(&self.base_workspace)?;
        self.current_source_root = self.base_workspace.clone();
        self.rebuild_private_workspace()?;
        self.baseline_manifest = refreshed_baseline;
        self.checkpoint_agent_fs()?;
        self.record_audit(
            WorkspaceAuditEventKind::Discarded,
            "Discarded isolated workspace changes".to_string(),
        );
        Ok(())
    }

    pub fn restore_checkpoint(&mut self, checkpoint_id: &FsCheckpointId) -> Result<FsCheckpoint> {
        self.drain_audit_events();
        let metadata = self.read_checkpoint_metadata(checkpoint_id)?;
        let snapshot_root = self.checkpoint_snapshot_root(checkpoint_id);
        if !snapshot_root.exists() {
            return Err(anyhow::anyhow!(
                "checkpoint snapshot {} does not exist",
                checkpoint_id.0
            ));
        }

        self.current_source_root = snapshot_root;
        self.rebuild_private_workspace()?;
        self.baseline_manifest = FsManifest::scan(&self.base_workspace)?;
        self.current_checkpoint = Some(metadata.clone());
        self.record_audit(
            WorkspaceAuditEventKind::CheckpointCreated,
            format!("Restored checkpoint {}", checkpoint_id.0),
        );
        Ok(metadata)
    }

    fn rebuild_private_workspace(&mut self) -> Result<()> {
        self.teardown_backend()?;

        std::fs::create_dir_all(&self.managed_tmp)
            .with_context(|| format!("recreate managed tmp {}", self.managed_tmp.display()))?;

        match &mut self.backend {
            WorkspaceBackend::Copy => {
                std::fs::create_dir_all(&self.private_workspace).with_context(|| {
                    format!(
                        "recreate private workspace {}",
                        self.private_workspace.display()
                    )
                })?;
                copy_tree(&self.current_source_root, &self.private_workspace)?;
            }
            #[cfg(target_os = "windows")]
            WorkspaceBackend::WindowsCopy => {
                std::fs::create_dir_all(&self.private_workspace).with_context(|| {
                    format!(
                        "recreate private workspace {}",
                        self.private_workspace.display()
                    )
                })?;
                copy_tree(&self.current_source_root, &self.private_workspace)?;
            }
            #[cfg(target_os = "macos")]
            WorkspaceBackend::Clone => {
                std::fs::create_dir_all(&self.private_workspace).with_context(|| {
                    format!(
                        "recreate private workspace {}",
                        self.private_workspace.display()
                    )
                })?;
                copy_tree_with_clone_fallback(&self.current_source_root, &self.private_workspace)?;
            }
            #[cfg(target_os = "linux")]
            WorkspaceBackend::Overlay {
                upperdir,
                workdir,
                mount_program,
                mounted,
            } => {
                std::fs::create_dir_all(&self.private_workspace).with_context(|| {
                    format!(
                        "recreate private workspace {}",
                        self.private_workspace.display()
                    )
                })?;
                std::fs::create_dir_all(&*upperdir)
                    .with_context(|| format!("recreate upperdir {}", upperdir.display()))?;
                std::fs::create_dir_all(&*workdir)
                    .with_context(|| format!("recreate workdir {}", workdir.display()))?;
                mount_overlay(
                    mount_program,
                    &self.current_source_root,
                    upperdir,
                    workdir,
                    &self.private_workspace,
                )?;
                *mounted = true;
            }
        }

        #[cfg(target_os = "linux")]
        self.start_linux_audit_watcher()?;

        Ok(())
    }

    fn checkpoints_root(&self) -> PathBuf {
        self.checkpoints_root.clone()
    }

    fn checkpoint_snapshot_root(&self, checkpoint_id: &FsCheckpointId) -> PathBuf {
        self.checkpoints_root()
            .join(&checkpoint_id.0)
            .join("workspace")
    }

    fn checkpoint_metadata_path(&self, checkpoint_id: &FsCheckpointId) -> PathBuf {
        self.checkpoints_root()
            .join(&checkpoint_id.0)
            .join("checkpoint.json")
    }

    fn read_checkpoint_metadata(&self, checkpoint_id: &FsCheckpointId) -> Result<FsCheckpoint> {
        let bytes = std::fs::read(self.checkpoint_metadata_path(checkpoint_id))
            .with_context(|| format!("read checkpoint metadata {}", checkpoint_id.0))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse checkpoint metadata {}", checkpoint_id.0))
    }

    fn teardown_backend(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.audit_watcher = None;
            self.audit_rx = None;
        }
        match &mut self.backend {
            WorkspaceBackend::Copy => {
                if self.private_workspace.exists() {
                    std::fs::remove_dir_all(&self.private_workspace).with_context(|| {
                        format!(
                            "remove private workspace {}",
                            self.private_workspace.display()
                        )
                    })?;
                }
                if self.session_root.exists() {
                    std::fs::remove_dir_all(&self.session_root).with_context(|| {
                        format!("remove session root {}", self.session_root.display())
                    })?;
                }
            }
            #[cfg(target_os = "windows")]
            WorkspaceBackend::WindowsCopy => {
                if self.private_workspace.exists() {
                    std::fs::remove_dir_all(&self.private_workspace).with_context(|| {
                        format!(
                            "remove private workspace {}",
                            self.private_workspace.display()
                        )
                    })?;
                }
                if self.session_root.exists() {
                    std::fs::remove_dir_all(&self.session_root).with_context(|| {
                        format!("remove session root {}", self.session_root.display())
                    })?;
                }
            }
            #[cfg(target_os = "macos")]
            WorkspaceBackend::Clone => {
                if self.private_workspace.exists() {
                    std::fs::remove_dir_all(&self.private_workspace).with_context(|| {
                        format!(
                            "remove private workspace {}",
                            self.private_workspace.display()
                        )
                    })?;
                }
                if self.session_root.exists() {
                    std::fs::remove_dir_all(&self.session_root).with_context(|| {
                        format!("remove session root {}", self.session_root.display())
                    })?;
                }
            }
            #[cfg(target_os = "linux")]
            WorkspaceBackend::Overlay {
                mount_program,
                mounted,
                ..
            } => {
                if *mounted {
                    unmount_overlay(mount_program, &self.private_workspace)?;
                    *mounted = false;
                }
                if self.session_root.exists() {
                    std::fs::remove_dir_all(&self.session_root).with_context(|| {
                        format!("remove session root {}", self.session_root.display())
                    })?;
                }
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn start_linux_audit_watcher(&mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())
            .with_context(|| format!("create watcher for {}", self.private_workspace.display()))?;
        watcher
            .watch(&self.private_workspace, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", self.private_workspace.display()))?;
        self.audit_watcher = Some(watcher);
        self.audit_rx = Some(rx);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn drain_audit_events(&mut self) {
        let Some(rx) = self.audit_rx.as_ref() else {
            return;
        };

        let mut pending_messages = Vec::new();

        while let Ok(event_result) = rx.try_recv() {
            match event_result {
                Ok(event) => {
                    let paths = event
                        .paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    pending_messages.push(format!("{:?}: {}", event.kind, paths));
                }
                Err(error) => {
                    pending_messages.push(format!("watch error: {}", error));
                }
            }
        }

        for message in pending_messages {
            self.record_audit(WorkspaceAuditEventKind::FileEventObserved, message);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn drain_audit_events(&mut self) {}
}

impl FsManifest {
    pub fn scan(root: &Path) -> Result<Self> {
        let mut entries = BTreeMap::new();
        scan_path(root, root, &mut entries)?;
        Ok(Self { entries })
    }

    pub fn diff(&self, other: &Self) -> FsDelta {
        let mut changes = Vec::new();
        for (path, before) in &self.entries {
            match other.entries.get(path) {
                Some(after) if before.kind != after.kind => changes.push(FsChange::TypeChanged {
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }),
                Some(after) if before.content_changed(after) => changes.push(FsChange::Modified {
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }),
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

    fn before_entry(&self) -> Option<&FsEntry> {
        match self {
            FsChange::Created { .. } => None,
            FsChange::Modified { before, .. }
            | FsChange::Deleted { before, .. }
            | FsChange::TypeChanged { before, .. } => Some(before),
        }
    }

    fn after_entry(&self) -> Option<&FsEntry> {
        match self {
            FsChange::Deleted { .. } => None,
            FsChange::Created { entry, .. } => Some(entry),
            FsChange::Modified { after, .. } | FsChange::TypeChanged { after, .. } => Some(after),
        }
    }
}

fn entry_snapshot_for_change(
    entry: Option<&FsEntry>,
    root: &Path,
    relative: &Path,
) -> Result<String> {
    match entry {
        None => Ok(String::new()),
        Some(entry) => render_entry_snapshot(entry, &root.join(relative)),
    }
}

fn render_entry_snapshot(entry: &FsEntry, path: &Path) -> Result<String> {
    match &entry.kind {
        FsEntryKind::File => Ok(std::fs::read_to_string(path).unwrap_or_else(|_| {
            String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
        })),
        FsEntryKind::Directory => Ok("[directory]\n".to_string()),
        FsEntryKind::Symlink { target } => Ok(format!("[symlink -> {}]\n", target.display())),
    }
}

fn detect_conflicts(agent_delta: &FsDelta, real_delta: &FsDelta) -> Vec<ApplyConflict> {
    let mut conflict_paths = std::collections::BTreeSet::new();
    for agent_change in &agent_delta.changes {
        for real_change in &real_delta.changes {
            if paths_overlap(agent_change.path(), real_change.path()) {
                conflict_paths.insert(overlapping_path(agent_change.path(), real_change.path()));
            }
        }
    }
    let mut minimized = Vec::new();
    for path in conflict_paths {
        if minimized
            .iter()
            .any(|existing: &PathBuf| path.starts_with(existing))
        {
            continue;
        }
        minimized.retain(|existing| !existing.starts_with(&path));
        minimized.push(path);
    }
    minimized
        .into_iter()
        .map(|path| ApplyConflict { path })
        .collect()
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

#[cfg(target_os = "macos")]
fn clone_backend_forced_off() -> bool {
    std::env::var("NITE_WORKSPACE_BACKEND")
        .map(|value| value.eq_ignore_ascii_case("copy"))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn windows_copy_backend_forced_off() -> bool {
    std::env::var("NITE_WORKSPACE_BACKEND")
        .map(|value| value.eq_ignore_ascii_case("copy"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn copy_tree_with_clone_fallback(src: &Path, dst: &Path) -> Result<()> {
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
            if let Err(error) = clone_file(path, &target) {
                let unsupported = error.downcast_ref::<std::io::Error>().is_some_and(|io| {
                    matches!(io.raw_os_error(), Some(libc::ENOTSUP | libc::EXDEV))
                });
                if !unsupported {
                    eprintln!(
                        "clonefile failed for {} -> {}, falling back to copy: {}",
                        path.display(),
                        target.display(),
                        error
                    );
                }
                std::fs::copy(path, &target)
                    .with_context(|| format!("copy {} to {}", path.display(), target.display()))?;
            }
        } else if file_type.is_symlink() {
            let link_target = std::fs::read_link(path)
                .with_context(|| format!("read symlink {}", path.display()))?;
            create_symlink(&link_target, &target)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src = CString::new(src.as_os_str().as_bytes())
        .with_context(|| format!("convert source {} to cstring", src.display()))?;
    let dst = CString::new(dst.as_os_str().as_bytes())
        .with_context(|| format!("convert target {} to cstring", dst.display()))?;
    let rc = unsafe { clonefile(src.as_ptr(), dst.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(anyhow::Error::from(std::io::Error::last_os_error()))
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn clonefile(
        src: *const libc::c_char,
        dst: *const libc::c_char,
        flags: libc::c_int,
    ) -> libc::c_int;
}

#[cfg(target_os = "linux")]
fn find_overlay_mount_program() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("fuse-overlayfs");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn overlay_backend_forced_off() -> bool {
    std::env::var("NITE_WORKSPACE_BACKEND")
        .map(|value| value.eq_ignore_ascii_case("copy"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn mount_overlay(
    mount_program: &Path,
    lowerdir: &Path,
    upperdir: &Path,
    workdir: &Path,
    mountpoint: &Path,
) -> Result<()> {
    let lower = shell_escape::escape(lowerdir.to_string_lossy());
    let upper = shell_escape::escape(upperdir.to_string_lossy());
    let work = shell_escape::escape(workdir.to_string_lossy());
    let mount = shell_escape::escape(mountpoint.to_string_lossy());
    let options = format!("lowerdir={lower},upperdir={upper},workdir={work}");
    let status = std::process::Command::new(mount_program)
        .arg("-o")
        .arg(options)
        .arg(mount.to_string())
        .status()
        .with_context(|| format!("spawn {}", mount_program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} exited with status {}",
            mount_program.display(),
            status
        ))
    }
}

#[cfg(target_os = "linux")]
fn unmount_overlay(mount_program: &Path, mountpoint: &Path) -> Result<()> {
    let fusermount = ["/usr/bin/fusermount3", "/usr/bin/fusermount"]
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file());
    if let Some(program) = fusermount {
        let status = std::process::Command::new(&program)
            .arg("-u")
            .arg(mountpoint)
            .status()
            .with_context(|| format!("spawn {}", program.display()))?;
        if status.success() {
            return Ok(());
        }
    }

    let status = std::process::Command::new("umount")
        .arg(mountpoint)
        .status()
        .with_context(|| {
            format!(
                "unmount {} from {}",
                mountpoint.display(),
                mount_program.display()
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "failed to unmount {}",
            mountpoint.display()
        ))
    }
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

    fn file_entry(contents: &str) -> FsEntry {
        FsEntry {
            kind: FsEntryKind::File,
            size: contents.len() as u64,
            hash: Some(format!("{:x}", sha2::Sha256::digest(contents.as_bytes()))),
            modified: None,
            readonly: false,
        }
    }

    fn dir_entry() -> FsEntry {
        FsEntry {
            kind: FsEntryKind::Directory,
            size: 0,
            hash: None,
            modified: None,
            readonly: false,
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn make_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sessionizer-workspace-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn isolated_workspace_commit_creates_and_modifies_files() {
        let temp = make_test_dir("commit-create-modify");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("file.txt"), "after")
            .expect("modify private file");
        std::fs::write(workspace.private_workspace().join("created.txt"), "new")
            .expect("create private file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "after"
        );
        assert_eq!(
            std::fs::read_to_string(temp.join("created.txt")).unwrap(),
            "new"
        );
    }

    #[test]
    fn isolated_workspace_commit_blocks_on_base_workspace_drift() {
        let temp = make_test_dir("commit-conflict");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("file.txt"), "agent")
            .expect("modify private file");
        std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, PathBuf::from("file.txt"));
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "user"
        );
    }

    #[test]
    fn isolated_workspace_discard_restores_private_copy_from_base() {
        let temp = make_test_dir("discard");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("file.txt"), "agent")
            .expect("modify private file");
        std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

        workspace.discard_changes().expect("discard changes");

        assert_eq!(
            std::fs::read_to_string(workspace.private_workspace().join("file.txt")).unwrap(),
            "user"
        );
        assert_eq!(workspace.pending_change_count().unwrap(), 0);
    }

    #[test]
    fn isolated_workspace_remaps_absolute_paths_into_private_root() {
        let temp = make_test_dir("path-remap");
        let workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");
        let absolute = temp.join("pizza-pref.txt");

        let remapped = workspace.remap_workspace_path(&absolute);

        assert_eq!(
            remapped,
            workspace.private_workspace().join("pizza-pref.txt")
        );
    }

    #[test]
    fn isolated_workspace_can_force_copy_backend() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("force-copy-backend");
        let workspace = SessionWorkspace::initialize(temp).expect("init workspace");

        assert_eq!(workspace.backend_name(), "copy");
    }

    #[test]
    fn isolated_workspace_checkpoint_state_tracks_latest_snapshot() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("checkpoint-state");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        let initial_checkpoint = workspace
            .current_checkpoint
            .clone()
            .expect("initial checkpoint");
        assert!(
            initial_checkpoint
                .manifest
                .entries
                .contains_key(&PathBuf::from("file.txt"))
        );

        std::fs::write(workspace.private_workspace().join("file.txt"), "after")
            .expect("modify private file");
        let checkpoint = workspace
            .checkpoint_agent_fs()
            .expect("checkpoint private state");
        assert_ne!(checkpoint.id, initial_checkpoint.id);
        assert_eq!(
            workspace
                .current_checkpoint
                .as_ref()
                .expect("checkpoint state")
                .id,
            checkpoint.id
        );

        let apply = workspace.apply_to_real_workspace().expect("apply changes");
        assert!(apply.conflicts.is_empty());
        let applied_checkpoint = workspace
            .current_checkpoint
            .clone()
            .expect("checkpoint after apply");
        assert_ne!(applied_checkpoint.id, checkpoint.id);
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "after"
        );

        std::fs::write(temp.join("file.txt"), "real").expect("modify real file");
        workspace.discard_changes().expect("discard changes");
        let discarded_checkpoint = workspace
            .current_checkpoint
            .clone()
            .expect("checkpoint after discard");
        assert_ne!(discarded_checkpoint.id, applied_checkpoint.id);
        assert_eq!(
            std::fs::read_to_string(workspace.private_workspace().join("file.txt")).unwrap(),
            "real"
        );
    }

    #[test]
    fn isolated_workspace_restore_checkpoint_restores_historical_private_state() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("restore-checkpoint");
        std::fs::write(temp.join("file.txt"), "base").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("file.txt"), "first")
            .expect("first change");
        let first_checkpoint = workspace.checkpoint_agent_fs().expect("first checkpoint");

        std::fs::write(workspace.private_workspace().join("file.txt"), "second")
            .expect("second change");
        workspace.checkpoint_agent_fs().expect("second checkpoint");

        std::fs::write(temp.join("file.txt"), "real-now").expect("modify real file");
        let restored = workspace
            .restore_checkpoint(&first_checkpoint.id)
            .expect("restore first checkpoint");

        assert_eq!(restored.id, first_checkpoint.id);
        assert_eq!(
            std::fs::read_to_string(workspace.private_workspace().join("file.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "real-now"
        );
        assert_eq!(workspace.pending_change_count().unwrap(), 1);
        let review = workspace.review_entries().expect("review entries");
        assert_eq!(review[0].old_string, "real-now");
        assert_eq!(review[0].new_string, "first");
    }

    #[test]
    fn isolated_workspace_initial_snapshot_preserves_nested_content() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("initial-snapshot");
        std::fs::create_dir_all(temp.join("nested/inner")).expect("create nested dirs");
        std::fs::write(temp.join("nested/inner/file.txt"), "hello").expect("write nested file");

        #[cfg(unix)]
        std::os::unix::fs::symlink("nested/inner/file.txt", temp.join("linked.txt"))
            .expect("create symlink");

        let workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        assert_eq!(
            std::fs::read_to_string(workspace.private_workspace().join("nested/inner/file.txt"))
                .unwrap(),
            "hello"
        );

        #[cfg(unix)]
        {
            let linked = workspace.private_workspace().join("linked.txt");
            assert!(
                std::fs::symlink_metadata(&linked)
                    .expect("symlink metadata")
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_link(linked).unwrap(),
                PathBuf::from("nested/inner/file.txt")
            );
        }
    }

    #[test]
    fn isolated_workspace_commit_applies_delete_nested_create_and_type_change() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("commit-delete-create-type-change");
        std::fs::write(temp.join("delete.txt"), "remove me").expect("seed delete file");
        std::fs::write(temp.join("replace-me"), "before").expect("seed type change file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_file(workspace.private_workspace().join("delete.txt"))
            .expect("delete private file");
        std::fs::create_dir_all(workspace.private_workspace().join("deep/new"))
            .expect("create nested dir");
        std::fs::write(
            workspace.private_workspace().join("deep/new/file.txt"),
            "created",
        )
        .expect("create nested file");
        std::fs::remove_file(workspace.private_workspace().join("replace-me"))
            .expect("remove original file");
        std::fs::create_dir_all(workspace.private_workspace().join("replace-me/child"))
            .expect("create replacement dir");
        std::fs::write(
            workspace
                .private_workspace()
                .join("replace-me/child/value.txt"),
            "after",
        )
        .expect("write replacement child file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert!(!temp.join("delete.txt").exists());
        assert_eq!(
            std::fs::read_to_string(temp.join("deep/new/file.txt")).unwrap(),
            "created"
        );
        assert!(temp.join("replace-me").is_dir());
        assert_eq!(
            std::fs::read_to_string(temp.join("replace-me/child/value.txt")).unwrap(),
            "after"
        );
        assert_eq!(workspace.pending_change_count().unwrap(), 0);
    }

    #[test]
    fn isolated_workspace_review_entries_cover_create_modify_delete_and_binary() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("review-entries");
        std::fs::write(temp.join("modify.txt"), "before").expect("seed modify file");
        std::fs::write(temp.join("delete.txt"), "gone soon").expect("seed delete file");
        std::fs::write(temp.join("binary.bin"), [0_u8, 159, 146, 150]).expect("seed binary file");
        let mut workspace = SessionWorkspace::initialize(temp).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("modify.txt"), "after")
            .expect("modify file");
        std::fs::remove_file(workspace.private_workspace().join("delete.txt"))
            .expect("delete file");
        std::fs::write(workspace.private_workspace().join("create.txt"), "created")
            .expect("create file");
        std::fs::write(
            workspace.private_workspace().join("binary.bin"),
            [0_u8, 255, 10],
        )
        .expect("rewrite binary file");

        let entries = workspace.review_entries().expect("review entries");

        let create = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("create.txt"))
            .unwrap();
        assert_eq!(create.old_string, "");
        assert_eq!(create.new_string, "created");

        let modify = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("modify.txt"))
            .unwrap();
        assert_eq!(modify.old_string, "before");
        assert_eq!(modify.new_string, "after");

        let delete = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("delete.txt"))
            .unwrap();
        assert_eq!(delete.old_string, "gone soon");
        assert_eq!(delete.new_string, "");

        let binary = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("binary.bin"))
            .unwrap();
        assert!(!binary.old_string.is_empty());
        assert!(!binary.new_string.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn isolated_workspace_applies_symlink_changes() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("symlink-apply");
        std::fs::write(temp.join("target-a.txt"), "a").expect("seed target a");
        std::fs::write(temp.join("target-b.txt"), "b").expect("seed target b");
        std::os::unix::fs::symlink("target-a.txt", temp.join("linked.txt")).expect("seed link");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_file(workspace.private_workspace().join("linked.txt"))
            .expect("remove old link");
        std::os::unix::fs::symlink(
            "target-b.txt",
            workspace.private_workspace().join("linked.txt"),
        )
        .expect("replace link");

        let result = workspace
            .apply_to_real_workspace()
            .expect("apply symlink change");

        assert!(result.conflicts.is_empty());
        assert_eq!(
            std::fs::read_link(temp.join("linked.txt")).unwrap(),
            PathBuf::from("target-b.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn isolated_workspace_discard_restores_latest_real_symlink_target() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("symlink-discard");
        std::fs::write(temp.join("target-a.txt"), "a").expect("seed target a");
        std::fs::write(temp.join("target-b.txt"), "b").expect("seed target b");
        std::os::unix::fs::symlink("target-a.txt", temp.join("linked.txt")).expect("seed link");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_file(workspace.private_workspace().join("linked.txt"))
            .expect("remove private link");
        std::os::unix::fs::symlink(
            "target-b.txt",
            workspace.private_workspace().join("linked.txt"),
        )
        .expect("replace private link");
        std::fs::remove_file(temp.join("linked.txt")).expect("remove real link");
        std::os::unix::fs::symlink("target-b.txt", temp.join("linked.txt"))
            .expect("replace real link");

        workspace.discard_changes().expect("discard changes");

        assert_eq!(
            std::fs::read_link(workspace.private_workspace().join("linked.txt")).unwrap(),
            PathBuf::from("target-b.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn isolated_workspace_commit_blocks_on_symlink_target_conflict() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("symlink-conflict");
        std::fs::write(temp.join("target-a.txt"), "a").expect("seed target a");
        std::fs::write(temp.join("target-b.txt"), "b").expect("seed target b");
        std::fs::write(temp.join("target-c.txt"), "c").expect("seed target c");
        std::os::unix::fs::symlink("target-a.txt", temp.join("linked.txt")).expect("seed link");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_file(workspace.private_workspace().join("linked.txt"))
            .expect("remove private link");
        std::os::unix::fs::symlink(
            "target-b.txt",
            workspace.private_workspace().join("linked.txt"),
        )
        .expect("replace private link");
        std::fs::remove_file(temp.join("linked.txt")).expect("remove real link");
        std::os::unix::fs::symlink("target-c.txt", temp.join("linked.txt"))
            .expect("replace real link");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, PathBuf::from("linked.txt"));
        assert_eq!(
            std::fs::read_link(temp.join("linked.txt")).unwrap(),
            PathBuf::from("target-c.txt")
        );
    }

    #[test]
    fn isolated_workspace_applies_binary_file_updates_exactly() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("binary-apply");
        std::fs::write(temp.join("blob.bin"), [0_u8, 1, 2, 3]).expect("seed binary file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        let replacement = [9_u8, 8, 7, 6, 5, 4];
        std::fs::write(workspace.private_workspace().join("blob.bin"), replacement)
            .expect("rewrite private binary file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert!(result.conflicts.is_empty());
        assert_eq!(std::fs::read(temp.join("blob.bin")).unwrap(), replacement);
    }

    #[test]
    fn isolated_workspace_records_audit_events_for_lifecycle() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("audit-log");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        assert!(
            workspace
                .audit_log()
                .iter()
                .any(|event| event.kind == WorkspaceAuditEventKind::BackendSelected)
        );
        assert!(
            workspace
                .audit_log()
                .iter()
                .any(|event| event.kind == WorkspaceAuditEventKind::WorkspacePrepared)
        );
        assert!(
            workspace
                .audit_log()
                .iter()
                .any(|event| event.kind == WorkspaceAuditEventKind::CheckpointCreated)
        );

        std::fs::write(workspace.private_workspace().join("file.txt"), "after")
            .expect("modify private file");
        let result = workspace.apply_to_real_workspace().expect("apply changes");
        assert!(result.conflicts.is_empty());
        assert!(
            workspace
                .audit_log()
                .iter()
                .any(|event| event.kind == WorkspaceAuditEventKind::CommitApplied)
        );

        workspace.discard_changes().expect("discard changes");
        assert!(
            workspace
                .audit_log()
                .iter()
                .any(|event| event.kind == WorkspaceAuditEventKind::Discarded)
        );
    }

    #[test]
    fn isolated_workspace_records_conflict_audit_events() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("audit-conflict");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("file.txt"), "agent")
            .expect("modify private file");
        std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert!(workspace.audit_log().iter().any(|event| {
            event.kind == WorkspaceAuditEventKind::CommitBlockedByConflicts
                && event.message.contains("1 conflict")
        }));
    }

    #[test]
    fn detect_conflicts_deduplicates_parent_child_overlaps() {
        let agent_delta = FsDelta {
            changes: vec![FsChange::Created {
                path: PathBuf::from("nested/file.txt"),
                entry: file_entry("agent"),
            }],
        };
        let real_delta = FsDelta {
            changes: vec![
                FsChange::Created {
                    path: PathBuf::from("nested"),
                    entry: dir_entry(),
                },
                FsChange::Created {
                    path: PathBuf::from("nested/file.txt"),
                    entry: file_entry("real"),
                },
            ],
        };

        let conflicts = detect_conflicts(&agent_delta, &real_delta);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path, PathBuf::from("nested"));
    }

    #[test]
    fn isolated_workspace_commit_blocks_on_delete_vs_modify_conflict() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("delete-vs-modify-conflict");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_file(workspace.private_workspace().join("file.txt"))
            .expect("delete private file");
        std::fs::write(temp.join("file.txt"), "user").expect("modify real file");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, PathBuf::from("file.txt"));
        assert_eq!(
            std::fs::read_to_string(temp.join("file.txt")).unwrap(),
            "user"
        );
    }

    #[test]
    fn isolated_workspace_commit_blocks_on_type_change_vs_real_child_change() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("type-change-conflict");
        std::fs::create_dir_all(temp.join("node")).expect("seed directory");
        std::fs::write(temp.join("node/existing.txt"), "base").expect("seed nested file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::remove_dir_all(workspace.private_workspace().join("node"))
            .expect("remove private directory");
        std::fs::write(workspace.private_workspace().join("node"), "agent file")
            .expect("replace directory with file");
        std::fs::write(temp.join("node/real.txt"), "user").expect("modify real subtree");

        let result = workspace.apply_to_real_workspace().expect("apply changes");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, PathBuf::from("node"));
        assert!(temp.join("node").is_dir());
        assert_eq!(
            std::fs::read_to_string(temp.join("node/real.txt")).unwrap(),
            "user"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn isolated_workspace_linux_watcher_records_file_events() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("linux-audit-watcher");
        let mut workspace = SessionWorkspace::initialize(temp).expect("init workspace");

        std::fs::write(workspace.private_workspace().join("watch.txt"), "hello")
            .expect("write watched file");
        std::thread::sleep(std::time::Duration::from_millis(100));

        let _ = workspace.pending_change_count().expect("drain watcher");

        assert!(workspace.audit_log().iter().any(|event| {
            event.kind == WorkspaceAuditEventKind::FileEventObserved
                && event.message.contains("watch.txt")
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn isolated_workspace_linux_watcher_restarts_after_discard() {
        let _env_lock = workspace_env_test_lock();
        let _guard = EnvVarGuard::set("NITE_WORKSPACE_BACKEND", "copy");
        let temp = make_test_dir("linux-watcher-discard");
        std::fs::write(temp.join("file.txt"), "before").expect("seed file");
        let mut workspace = SessionWorkspace::initialize(temp.clone()).expect("init workspace");

        std::fs::write(
            workspace.private_workspace().join("before-discard.txt"),
            "hello",
        )
        .expect("write first watched file");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = workspace.pending_change_count().expect("drain watcher");

        workspace.discard_changes().expect("discard changes");
        assert_eq!(workspace.pending_change_count().unwrap(), 0);

        std::fs::write(
            workspace.private_workspace().join("after-discard.txt"),
            "world",
        )
        .expect("write second watched file");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = workspace
            .pending_change_count()
            .expect("drain restarted watcher");

        let audit_log = workspace.audit_log();
        assert!(audit_log.iter().any(|event| {
            event.kind == WorkspaceAuditEventKind::FileEventObserved
                && event.message.contains("before-discard.txt")
        }));
        assert!(audit_log.iter().any(|event| {
            event.kind == WorkspaceAuditEventKind::FileEventObserved
                && event.message.contains("after-discard.txt")
        }));
    }
}
