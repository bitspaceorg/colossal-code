use crate::safety_config;
use anyhow::Result;
use colossal_linux_sandbox::protocol::{SandboxPolicy, WritableRoot};
use once_cell::sync::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static WORKSPACE_ROOT_OVERRIDE: OnceCell<Mutex<Option<PathBuf>>> = OnceCell::new();

pub fn workspace_root_override() -> Option<PathBuf> {
    WORKSPACE_ROOT_OVERRIDE
        .get_or_init(|| {
            Mutex::new(std::env::var("NITE_WORKSPACE_ROOT").ok().and_then(|raw| {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let candidate = PathBuf::from(trimmed);
                if candidate.is_absolute() {
                    Some(candidate)
                } else {
                    std::env::current_dir().ok().map(|cwd| cwd.join(candidate))
                }
            }))
        })
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(None)
}

pub fn set_workspace_root_override(path: impl AsRef<Path>) {
    let absolute = if path.as_ref().is_absolute() {
        path.as_ref().to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path.as_ref())
    };
    let slot = WORKSPACE_ROOT_OVERRIDE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(absolute);
    }
}

pub fn resolve_workspace_root() -> PathBuf {
    workspace_root_override()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn resolve_tools_binary_path_for_runtime() -> Result<PathBuf> {
    colossal_linux_sandbox::resolve_tools_binary_path()
        .map_err(|err| anyhow::anyhow!(err.to_string()))
}

fn push_writable_root_unique(writable_roots: &mut Vec<WritableRoot>, root: PathBuf) {
    if writable_roots.iter().any(|existing| existing.root == root) {
        return;
    }
    writable_roots.push(WritableRoot {
        root,
        recursive: true,
        read_only_subpaths: vec![],
    });
}

pub fn sandbox_policy_from_config_with_workspace(
    safety_config: &safety_config::SafetyConfig,
    workspace_path: PathBuf,
) -> SandboxPolicy {
    let mut writable_roots = Vec::new();
    push_writable_root_unique(&mut writable_roots, workspace_path.clone());

    if let Some(parent) = workspace_path.parent() {
        push_writable_root_unique(&mut writable_roots, parent.to_path_buf());
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_parent) = exe_path.parent().and_then(|p| p.parent()) {
            push_writable_root_unique(&mut writable_roots, exe_parent.to_path_buf());
        }
    }

    if let Ok(tools_path) = resolve_tools_binary_path_for_runtime()
        && let Some(tools_parent) = tools_path.parent()
    {
        push_writable_root_unique(&mut writable_roots, tools_parent.to_path_buf());
    }

    if let Ok(extra_roots) = std::env::var("SANDBOX_EXTRA_ROOTS") {
        for root_path in extra_roots.split(':') {
            if !root_path.is_empty() {
                writable_roots.push(WritableRoot {
                    root: PathBuf::from(root_path),
                    recursive: true,
                    read_only_subpaths: vec![],
                });
            }
        }
    }

    match safety_config.mode {
        safety_config::SafetyMode::ReadOnly => SandboxPolicy::ReadOnly,
        safety_config::SafetyMode::Regular => {
            if safety_config.sandbox_enabled || std::env::var("SAFE_MODE").is_ok() {
                SandboxPolicy::WorkspaceWrite {
                    writable_roots,
                    network_access: colossal_linux_sandbox::protocol::NetworkAccess::Enabled,
                    exclude_tmpdir_env_var: false,
                    exclude_slash_tmp: false,
                }
            } else {
                SandboxPolicy::DangerFullAccess
            }
        }
        safety_config::SafetyMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

pub fn sandbox_policy_from_config(safety_config: &safety_config::SafetyConfig) -> SandboxPolicy {
    sandbox_policy_from_config_with_workspace(safety_config, resolve_workspace_root())
}
