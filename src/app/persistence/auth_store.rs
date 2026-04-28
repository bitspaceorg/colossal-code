use color_eyre::Result;
use keyring::{Entry, Error as KeyringError};
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
    #[serde(rename = "claude_code")]
    ClaudeCode,
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
    #[serde(default)]
    pub(crate) oauth_scopes: Vec<String>,
    #[serde(default)]
    pub(crate) oauth_subscription_type: Option<String>,
    #[serde(default)]
    pub(crate) oauth_rate_limit_tier: Option<String>,
    #[serde(default)]
    pub(crate) organization_id: Option<String>,
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
const KEYRING_SERVICE: &str = "cocode";

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
    let mut store = serde_json::from_str(&content).unwrap_or_default();
    hydrate_store_secrets(&mut store);
    Ok(store)
}

pub(crate) fn save_auth_store(store: &AuthStore) -> Result<()> {
    let path = auth_file_path()?;
    persist_store_secrets(store)?;
    let mut scrubbed = store.clone();
    scrub_store_secrets(&mut scrubbed);
    let content = serde_json::to_string_pretty(&scrubbed)?;
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

fn secret_field_names() -> [&'static str; 3] {
    ["api_key", "access_token", "refresh_token"]
}

fn keyring_entry(connection_id: &str, field: &str) -> Result<Entry> {
    Ok(Entry::new(
        KEYRING_SERVICE,
        &format!("connection:{connection_id}:{field}"),
    )?)
}

fn persist_store_secrets(store: &AuthStore) -> Result<()> {
    for connection in &store.connections {
        persist_connection_secret(connection, "api_key", connection.api_key.as_deref())?;
        persist_connection_secret(
            connection,
            "access_token",
            connection.access_token.as_deref(),
        )?;
        persist_connection_secret(
            connection,
            "refresh_token",
            connection.refresh_token.as_deref(),
        )?;
    }
    Ok(())
}

fn persist_connection_secret(
    connection: &StoredConnection,
    field: &str,
    value: Option<&str>,
) -> Result<()> {
    let entry = keyring_entry(&connection.id, field)?;
    match value.filter(|value| !value.trim().is_empty()) {
        Some(secret) => entry.set_password(secret)?,
        None => match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(err) => return Err(err.into()),
        },
    }
    Ok(())
}

fn hydrate_store_secrets(store: &mut AuthStore) {
    for connection in &mut store.connections {
        hydrate_connection_secret(&connection.id, "api_key", &mut connection.api_key);
        hydrate_connection_secret(&connection.id, "access_token", &mut connection.access_token);
        hydrate_connection_secret(
            &connection.id,
            "refresh_token",
            &mut connection.refresh_token,
        );
    }
}

fn hydrate_connection_secret(connection_id: &str, field: &str, slot: &mut Option<String>) {
    let Ok(entry) = keyring_entry(connection_id, field) else {
        return;
    };
    match entry.get_password() {
        Ok(secret) if !secret.trim().is_empty() => *slot = Some(secret),
        Ok(_) | Err(KeyringError::NoEntry) => {}
        Err(_) => {}
    }
}

fn scrub_store_secrets(store: &mut AuthStore) {
    for connection in &mut store.connections {
        for field in secret_field_names() {
            match field {
                "api_key" => connection.api_key = None,
                "access_token" => connection.access_token = None,
                "refresh_token" => {
                    if connection.auth_kind != StoredAuthKind::OpenAiSubscription {
                        connection.refresh_token = None;
                    }
                }
                _ => {}
            }
        }
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
            oauth_scopes: Vec::new(),
            oauth_subscription_type: None,
            oauth_rate_limit_tier: None,
            organization_id: None,
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
            oauth_scopes: Vec::new(),
            oauth_subscription_type: None,
            oauth_rate_limit_tier: None,
            organization_id: None,
            created_at: now,
            updated_at: now + 1,
        });

        assert_eq!(store.connections.len(), 1);
        assert_eq!(store.connections[0].api_key.as_deref(), Some("two"));
        assert_eq!(store.connections[0].model.as_deref(), Some("gpt-5-mini"));
    }
}
