use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Debug)]
pub(super) struct ClaudeCodeAuthState {
    access_token: Mutex<String>,
    access_expires_at: Mutex<Option<u64>>,
    organization_id: Mutex<Option<String>>,
    refresh_lock: Mutex<()>,
    cli_version: String,
    refresh_via_cli_allowed: bool,
}

impl ClaudeCodeAuthState {
    pub(super) fn from_env(initial_access_token: String) -> Option<Self> {
        let access_expires_at = std::env::var("NITE_HTTP_ACCESS_EXPIRES_AT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());
        let organization_id = std::env::var("NITE_HTTP_ORGANIZATION_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if initial_access_token.trim().is_empty() {
            return None;
        }

        Some(Self {
            access_token: Mutex::new(initial_access_token),
            access_expires_at: Mutex::new(access_expires_at),
            organization_id: Mutex::new(organization_id),
            refresh_lock: Mutex::new(()),
            cli_version: detected_claude_cli_version(),
            refresh_via_cli_allowed: access_expires_at.is_some(),
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

    pub(super) async fn organization_id(&self) -> Option<String> {
        self.organization_id.lock().await.clone()
    }

    pub(super) fn can_force_refresh(&self) -> bool {
        self.refresh_via_cli_allowed
    }

    pub(super) async fn ensure_fresh(&self, _client: &reqwest::Client, force: bool) -> Result<()> {
        if !force && !self.should_refresh().await {
            return Ok(());
        }

        let _guard = self.refresh_lock.lock().await;
        if !force && !self.should_refresh().await {
            return Ok(());
        }

        if !force && self.sync_from_claude_store().await? {
            return Ok(());
        }

        if !self.refresh_via_cli_allowed {
            return Err(anyhow::anyhow!(
                "Claude token refresh is not available for this connection. Run `claude setup-token`, reconnect the provider, and try again."
            ));
        }

        if let Err(err) = refresh_via_cli() {
            return Err(anyhow::anyhow!(
                "Claude CLI token refresh failed: {}. Run `claude auth login --claudeai` to re-authenticate.",
                err
            ));
        }
        let _ = self.sync_from_claude_store().await?;
        Ok(())
    }

    pub(super) fn apply_request_headers(
        &self,
        request: reqwest::RequestBuilder,
        model_id: &str,
    ) -> reqwest::RequestBuilder {
        let user_agent = std::env::var("ANTHROPIC_USER_AGENT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("claude-cli/{} (external, cli)", self.cli_version));
        request
            .header("anthropic-beta", anthropic_beta_header(model_id))
            .header("x-app", "cli")
            .header("user-agent", user_agent)
            .header(
                "x-anthropic-billing-header",
                format!(
                    "cc_version={}.{}; cc_entrypoint=cli; cch=00000;",
                    self.cli_version, model_id
                ),
            )
    }

    async fn should_refresh(&self) -> bool {
        let now = current_timestamp();
        let expires_at = *self.access_expires_at.lock().await;
        match expires_at {
            Some(value) => value <= now + 60,
            None => false,
        }
    }

    async fn sync_from_claude_store(&self) -> Result<bool> {
        let Some(credentials) = load_claude_credentials()? else {
            return Ok(false);
        };
        if credentials.access_token.trim().is_empty() {
            return Ok(false);
        }

        let expires_at = credentials.expires_at.unwrap_or(0);
        if expires_at <= current_timestamp() + 60 {
            return Ok(false);
        }

        *self.access_token.lock().await = credentials.access_token;
        *self.access_expires_at.lock().await = credentials.expires_at;
        if let Some(org_id) = read_claude_auth_status().and_then(|status| status.org_id) {
            *self.organization_id.lock().await = Some(org_id);
        }

        Ok(true)
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeCredentialStore {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeAiOauthCredentials>,
}

#[derive(Debug, Deserialize)]
struct ClaudeAiOauthCredentials {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ClaudeAuthStatus {
    #[serde(rename = "orgId")]
    org_id: Option<String>,
}

fn anthropic_beta_header(model_id: &str) -> String {
    let mut betas = vec![
        "claude-code-20250219",
        "oauth-2025-04-20",
        "interleaved-thinking-2025-05-14",
        "prompt-caching-scope-2026-01-05",
        "context-management-2025-06-27",
    ];
    let id_lower = model_id.to_ascii_lowercase();
    let supports_effort =
        id_lower.contains("4-5") || id_lower.contains("4-6") || id_lower.contains("4-7");
    if supports_effort {
        betas.push("effort-2025-11-24");
    }
    betas.join(",")
}

fn detected_claude_cli_version() -> String {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|stdout| stdout.split_whitespace().next().map(str::to_string))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "2.1.87".to_string())
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_claude_credentials() -> Result<Option<ClaudeAiOauthCredentials>> {
    let path = claude_credentials_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    let store: ClaudeCredentialStore = serde_json::from_str(&content)?;
    Ok(store.claude_ai_oauth)
}

fn read_claude_auth_status() -> Option<ClaudeAuthStatus> {
    let output = Command::new("claude")
        .args(["auth", "status", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<ClaudeAuthStatus>(&output.stdout).ok()
}

fn refresh_via_cli() -> anyhow::Result<()> {
    let status = Command::new("claude")
        .args(["-p", ".", "--model", "haiku"])
        .current_dir(std::env::temp_dir())
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| {
            anyhow::anyhow!(
                "could not run `claude` CLI (is it installed and on PATH?): {}",
                err
            )
        })?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "`claude` CLI exited with status {} during token refresh",
            status
        ));
    }
    Ok(())
}

fn claude_credentials_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join(".credentials.json");
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join(".credentials.json")
}
