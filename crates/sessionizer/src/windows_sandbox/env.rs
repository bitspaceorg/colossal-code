use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn normalize_null_device_env(env_map: &mut HashMap<String, String>) {
    let keys: Vec<String> = env_map.keys().cloned().collect();
    for key in keys {
        if let Some(value) = env_map.get(&key).cloned() {
            let normalized = value.trim().to_ascii_lowercase();
            if normalized == "/dev/null" || normalized == "\\\\dev\\null" {
                env_map.insert(key, "NUL".to_string());
            }
        }
    }
}

pub fn ensure_non_interactive_pager(env_map: &mut HashMap<String, String>) {
    env_map
        .entry("GIT_PAGER".into())
        .or_insert_with(|| "more.com".into());
    env_map
        .entry("PAGER".into())
        .or_insert_with(|| "more.com".into());
    env_map.entry("LESS".into()).or_insert_with(String::new);
}

pub fn apply_no_network_to_env(env_map: &mut HashMap<String, String>) -> anyhow::Result<()> {
    env_map.insert("SBX_NONET_ACTIVE".into(), "1".into());
    env_map
        .entry("HTTP_PROXY".into())
        .or_insert_with(|| "http://127.0.0.1:9".into());
    env_map
        .entry("HTTPS_PROXY".into())
        .or_insert_with(|| "http://127.0.0.1:9".into());
    env_map
        .entry("ALL_PROXY".into())
        .or_insert_with(|| "http://127.0.0.1:9".into());
    env_map
        .entry("NO_PROXY".into())
        .or_insert_with(|| "localhost,127.0.0.1,::1".into());
    env_map
        .entry("PIP_NO_INDEX".into())
        .or_insert_with(|| "1".into());
    env_map
        .entry("PIP_DISABLE_PIP_VERSION_CHECK".into())
        .or_insert_with(|| "1".into());
    env_map
        .entry("NPM_CONFIG_OFFLINE".into())
        .or_insert_with(|| "true".into());
    env_map
        .entry("CARGO_NET_OFFLINE".into())
        .or_insert_with(|| "true".into());
    env_map
        .entry("GIT_HTTP_PROXY".into())
        .or_insert_with(|| "http://127.0.0.1:9".into());
    env_map
        .entry("GIT_HTTPS_PROXY".into())
        .or_insert_with(|| "http://127.0.0.1:9".into());
    env_map
        .entry("GIT_SSH_COMMAND".into())
        .or_insert_with(|| "cmd /c exit 1".into());
    env_map
        .entry("GIT_ALLOW_PROTOCOLS".into())
        .or_insert_with(String::new);

    let denybin = ensure_denybin(&["ssh", "scp"], None)?;
    prepend_path(env_map, &denybin.to_string_lossy());
    reorder_pathext_for_stubs(env_map);
    Ok(())
}

fn ensure_denybin(tools: &[&str], denybin_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let base = match denybin_dir {
        Some(path) => path.to_path_buf(),
        None => env::temp_dir().join("colossal-code").join("sbx-denybin"),
    };
    fs::create_dir_all(&base)?;
    for tool in tools {
        for ext in [".bat", ".cmd"] {
            let path = base.join(format!("{tool}{ext}"));
            if !path.exists() {
                let mut file = File::create(&path)?;
                file.write_all(b"@echo off\r\nexit /b 1\r\n")?;
            }
        }
    }
    Ok(base)
}

fn prepend_path(env_map: &mut HashMap<String, String>, prefix: &str) {
    let existing = env_map
        .get("PATH")
        .cloned()
        .or_else(|| env::var("PATH").ok())
        .unwrap_or_default();
    if existing
        .split(';')
        .next()
        .map(|part| part.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
    {
        return;
    }
    let mut updated = String::from(prefix);
    if !existing.is_empty() {
        updated.push(';');
        updated.push_str(&existing);
    }
    env_map.insert("PATH".into(), updated);
}

fn reorder_pathext_for_stubs(env_map: &mut HashMap<String, String>) {
    let default = env_map
        .get("PATHEXT")
        .cloned()
        .or_else(|| env::var("PATHEXT").ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    let exts: Vec<String> = default
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(ToString::to_string)
        .collect();
    let exts_upper: Vec<String> = exts.iter().map(|ext| ext.to_ascii_uppercase()).collect();
    let mut reordered = Vec::new();
    for wanted in [".BAT", ".CMD"] {
        if let Some(index) = exts_upper.iter().position(|ext| ext == wanted) {
            reordered.push(exts[index].clone());
        }
    }
    for (index, ext) in exts.into_iter().enumerate() {
        if exts_upper[index] != ".BAT" && exts_upper[index] != ".CMD" {
            reordered.push(ext);
        }
    }
    env_map.insert("PATHEXT".into(), reordered.join(";"));
}
