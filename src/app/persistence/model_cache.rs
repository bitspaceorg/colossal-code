use color_eyre::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::app::persistence::auth_store::{
    StoredAuthKind, StoredConnection, current_unix_timestamp,
};
use crate::app::persistence::config::config_dir;

const MODEL_CACHE_FILE_NAME: &str = "model_cache.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ModelCache {
    #[serde(default = "default_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ModelCacheEntry {
    pub(crate) connection_id: String,
    pub(crate) provider_id: String,
    pub(crate) auth_kind: StoredAuthKind,
    pub(crate) models: Vec<String>,
    pub(crate) updated_at: u64,
}

fn default_version() -> u32 {
    1
}

pub(crate) fn model_cache_file_path() -> Result<PathBuf> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    Ok(dir.join(MODEL_CACHE_FILE_NAME))
}

pub(crate) fn load_model_cache() -> Result<ModelCache> {
    let path = model_cache_file_path()?;
    if !path.exists() {
        return Ok(ModelCache::default());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content).unwrap_or_default())
}

pub(crate) fn save_model_cache(cache: &ModelCache) -> Result<()> {
    let path = model_cache_file_path()?;
    let content = serde_json::to_string_pretty(cache)?;
    fs::write(&path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl ModelCache {
    pub(crate) fn models_for(&self, connection: &StoredConnection) -> Option<Vec<String>> {
        self.entries
            .iter()
            .find(|entry| {
                entry.connection_id == connection.id
                    && entry.provider_id == connection.provider_id
                    && entry.auth_kind == connection.auth_kind
            })
            .map(|entry| entry.models.clone())
            .filter(|models| !models.is_empty())
    }

    pub(crate) fn upsert_models(&mut self, connection: &StoredConnection, models: Vec<String>) {
        if models.is_empty() {
            return;
        }

        let updated_at = current_unix_timestamp();
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.connection_id == connection.id
                && entry.provider_id == connection.provider_id
                && entry.auth_kind == connection.auth_kind
        }) {
            entry.models = models;
            entry.updated_at = updated_at;
            return;
        }

        self.entries.push(ModelCacheEntry {
            connection_id: connection.id.clone(),
            provider_id: connection.provider_id.clone(),
            auth_kind: connection.auth_kind.clone(),
            models,
            updated_at,
        });
    }
}
