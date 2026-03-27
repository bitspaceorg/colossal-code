use color_eyre::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::persistence::config::config_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum StoredAuthKind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "openai_subscription")]
    OpenAiSubscription,
}

impl Default for StoredAuthKind {
    fn default() -> Self {
        Self::ApiKey
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredConnection {
    pub(crate) id: String,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    #[serde(default)]
    pub(crate) auth_kind: StoredAuthKind,
    #[serde(default)]
    pub(crate) api_key: Option<String>,
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) completions_path: Option<String>,
    #[serde(default)]
    pub(crate) account_id: Option<String>,
    #[serde(default)]
    pub(crate) access_token: Option<String>,
    #[serde(default)]
    pub(crate) refresh_token: Option<String>,
    #[serde(default)]
    pub(crate) access_expires_at: Option<u64>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AuthStore {
    #[serde(default = "default_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) active_connection_id: Option<String>,
    #[serde(default)]
    pub(crate) connections: Vec<StoredConnection>,
}

const AUTH_FILE_NAME: &str = "auth.json";

fn default_version() -> u32 {
    1
}

pub(crate) fn auth_file_path() -> Result<PathBuf> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    Ok(dir.join(AUTH_FILE_NAME))
}

pub(crate) fn load_auth_store() -> Result<AuthStore> {
    let path = auth_file_path()?;
    if !path.exists() {
        return Ok(AuthStore::default());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content).unwrap_or_default())
}

pub(crate) fn save_auth_store(store: &AuthStore) -> Result<()> {
    let path = auth_file_path()?;
    let content = serde_json::to_string_pretty(store)?;
    fs::write(&path, content)?;
    set_restricted_permissions(&path)?;
    Ok(())
}

pub(crate) fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn set_restricted_permissions(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_restricted_permissions(_path: &PathBuf) -> Result<()> {
    Ok(())
}

impl AuthStore {
    pub(crate) fn upsert_connection(&mut self, connection: StoredConnection) {
        if let Some(existing) = self
            .connections
            .iter_mut()
            .find(|existing| existing.id == connection.id)
        {
            *existing = connection;
            return;
        }

        self.connections.push(connection);
        self.connections
            .sort_by(|a, b| a.provider_name.cmp(&b.provider_name));
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthStore, StoredAuthKind, StoredConnection, current_unix_timestamp};

    #[test]
    fn upsert_connection_replaces_matching_id() {
        let mut store = AuthStore::default();
        let now = current_unix_timestamp();
        store.upsert_connection(StoredConnection {
            id: "openai".to_string(),
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            auth_kind: StoredAuthKind::ApiKey,
            api_key: Some("one".to_string()),
            model: Some("gpt-5".to_string()),
            base_url: None,
            completions_path: None,
            account_id: None,
            access_token: None,
            refresh_token: None,
            access_expires_at: None,
            created_at: now,
            updated_at: now,
        });
        store.upsert_connection(StoredConnection {
            id: "openai".to_string(),
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            auth_kind: StoredAuthKind::ApiKey,
            api_key: Some("two".to_string()),
            model: Some("gpt-5-mini".to_string()),
            base_url: None,
            completions_path: None,
            account_id: None,
            access_token: None,
            refresh_token: None,
            access_expires_at: None,
            created_at: now,
            updated_at: now + 1,
        });

        assert_eq!(store.connections.len(), 1);
        assert_eq!(store.connections[0].api_key.as_deref(), Some("two"));
        assert_eq!(store.connections[0].model.as_deref(), Some("gpt-5-mini"));
    }
}
