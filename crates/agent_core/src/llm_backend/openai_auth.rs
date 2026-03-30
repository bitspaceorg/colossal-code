use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use super::HttpBackend;

const OPENAI_AUTH_ISSUER: &str = "https://auth.openai.com";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug)]
pub(super) struct OpenAiAuthState {
    access_token: Mutex<String>,
    refresh_token: Mutex<Option<String>>,
    access_expires_at: Mutex<Option<u64>>,
    account_id: Mutex<Option<String>>,
    auth_file: Option<PathBuf>,
    active_connection_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, serde::Serialize, Deserialize, Default)]
struct RuntimeAuthStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    active_connection_id: Option<String>,
    #[serde(default)]
    connections: Vec<RuntimeStoredConnection>,
}

#[derive(Debug, serde::Serialize, Deserialize, Default)]
struct RuntimeStoredConnection {
    id: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    access_expires_at: Option<u64>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl OpenAiAuthState {
    pub(super) fn from_env(initial_access_token: String) -> Option<Self> {
        let refresh_token = std::env::var("NITE_HTTP_REFRESH_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let account_id = std::env::var("NITE_HTTP_ACCOUNT_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let access_expires_at = std::env::var("NITE_HTTP_ACCESS_EXPIRES_AT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());
        let auth_file = std::env::var("NITE_AUTH_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| path.exists());
        let active_connection_id = std::env::var("NITE_ACTIVE_CONNECTION_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if refresh_token.is_none() && account_id.is_none() && access_expires_at.is_none() {
            return None;
        }

        Some(Self {
            access_token: Mutex::new(initial_access_token),
            refresh_token: Mutex::new(refresh_token),
            access_expires_at: Mutex::new(access_expires_at),
            account_id: Mutex::new(account_id),
            auth_file,
            active_connection_id,
        })
    }

    pub(super) async fn auth_header(&self) -> Option<String> {
        let token = self.access_token.lock().await.clone();
        if token.trim().is_empty() {
            None
        } else {
            Some(format!("Bearer {}", token))
        }
    }

    pub(super) async fn account_id(&self) -> Option<String> {
        self.account_id.lock().await.clone()
    }

    pub(super) async fn ensure_fresh(&self, client: &reqwest::Client) -> Result<()> {
        let now = current_timestamp();
        let expires_at = *self.access_expires_at.lock().await;
        if !expires_at.is_some_and(|value| value <= now + 60) {
            return Ok(());
        }

        let refresh_token = self.refresh_token.lock().await.clone();
        let Some(refresh_token) = refresh_token else {
            return Ok(());
        };

        let response = client
            .post(format!("{OPENAI_AUTH_ISSUER}/oauth/token"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", OPENAI_CLIENT_ID),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "OpenAI token refresh failed ({status}): {body}"
            ));
        }

        let refreshed: RefreshResponse = response.json().await?;
        let new_refresh = refreshed.refresh_token.clone().unwrap_or(refresh_token);
        let expires_at = now + refreshed.expires_in.unwrap_or(3600);

        *self.access_token.lock().await = refreshed.access_token.clone();
        *self.refresh_token.lock().await = Some(new_refresh.clone());
        *self.access_expires_at.lock().await = Some(expires_at);
        persist_refreshed_tokens(
            self.auth_file.clone(),
            self.active_connection_id.clone(),
            refreshed.access_token,
            new_refresh,
            expires_at,
            self.account_id.lock().await.clone(),
        )?;
        Ok(())
    }
}

impl HttpBackend {
    pub(super) async fn ensure_fresh_openai_auth(&self) -> Result<()> {
        if let Some(state) = &self.openai_auth {
            state.ensure_fresh(&self.client).await?;
        }
        Ok(())
    }

    pub(super) fn has_openai_auth(&self) -> bool {
        self.openai_auth.is_some()
    }

    pub(super) async fn openai_auth_header(&self) -> Option<String> {
        if let Some(state) = &self.openai_auth {
            return state.auth_header().await;
        }
        self.auth_header()
    }

    pub(super) async fn openai_account_id(&self) -> Option<String> {
        if let Some(state) = &self.openai_auth {
            return state.account_id().await;
        }
        self.chatgpt_account_id.clone()
    }
}

fn persist_refreshed_tokens(
    auth_file: Option<PathBuf>,
    active_connection_id: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    account_id: Option<String>,
) -> Result<()> {
    let Some(path) = auth_file else {
        return Ok(());
    };
    let Some(connection_id) = active_connection_id else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let mut store: RuntimeAuthStore = serde_json::from_str(&content).unwrap_or_default();
    let Some(connection) = store
        .connections
        .iter_mut()
        .find(|entry| entry.id == connection_id)
    else {
        return Ok(());
    };

    connection.access_token = Some(access_token);
    connection.refresh_token = Some(refresh_token);
    connection.access_expires_at = Some(expires_at);
    if account_id.is_some() {
        connection.account_id = account_id;
    }

    fs::write(&path, serde_json::to_string_pretty(&store)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
