use super::path_normalization::canonical_path_key;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapSids {
    pub workspace: String,
    pub readonly: String,
    #[serde(default)]
    pub workspace_by_cwd: HashMap<String, String>,
}

pub fn cap_sid_file() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("colossal-code")
        .join("sandbox")
        .join("cap_sid.json")
}

pub fn load_or_create_cap_sids() -> anyhow::Result<CapSids> {
    let path = cap_sid_file();
    if path.exists() {
        let text = fs::read_to_string(&path)?;
        let trimmed = text.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            if let Ok(caps) = serde_json::from_str::<CapSids>(trimmed) {
                return Ok(caps);
            }
        } else if !trimmed.is_empty() {
            let caps = CapSids {
                workspace: trimmed.to_string(),
                readonly: make_random_cap_sid_string(),
                workspace_by_cwd: HashMap::new(),
            };
            persist_caps(&path, &caps)?;
            return Ok(caps);
        }
    }

    let caps = CapSids {
        workspace: make_random_cap_sid_string(),
        readonly: make_random_cap_sid_string(),
        workspace_by_cwd: HashMap::new(),
    };
    persist_caps(&path, &caps)?;
    Ok(caps)
}

pub fn workspace_cap_sid_for_cwd(cwd: &Path) -> anyhow::Result<String> {
    let path = cap_sid_file();
    let mut caps = load_or_create_cap_sids()?;
    let key = canonical_path_key(cwd);
    if let Some(sid) = caps.workspace_by_cwd.get(&key) {
        return Ok(sid.clone());
    }
    let sid = make_random_cap_sid_string();
    caps.workspace_by_cwd.insert(key, sid.clone());
    persist_caps(&path, &caps)?;
    Ok(sid)
}

fn persist_caps(path: &Path, caps: &CapSids) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string(caps)?)?;
    Ok(())
}

fn make_random_cap_sid_string() -> String {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let a = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let b = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let c = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let d = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    format!("S-1-5-21-{a}-{b}-{c}-{d}")
}
