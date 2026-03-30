use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use color_eyre::Result;
use serde::Deserialize;

use crate::app::App;

#[derive(Debug, Deserialize)]
struct ClaudeCredentialStore {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeAiOauthCredentials>,
}

#[derive(Debug, Deserialize)]
struct ClaudeAiOauthCredentials {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeAuthStatus {
    #[serde(rename = "orgId")]
    org_id: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

impl App {
    pub(crate) fn start_claude_code_auth(&mut self) -> Result<()> {
        let launch_command = "claude auth login --claudeai".to_string();
        let launch_result = Command::new("claude")
            .args(["auth", "login", "--claudeai"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        let status = match (launch_result, claude_code_session_available()) {
            (_, true) => "Claude Code already has a local login session. You can finish `claude auth login --claudeai` to refresh it, then press Enter here to import the account.".to_string(),
            (Ok(_), false) => {
                "Started Claude Code login in the background. If the browser does not open, run `claude auth login --claudeai` in another terminal, then press Enter here.".to_string()
            }
            (Err(_), false) => {
                "Run `claude auth login --claudeai` in another terminal, finish login, then press Enter here to import the session.".to_string()
            }
        };

        self.connect.oauth_state.started = true;
        self.connect.oauth_state.launch_command = Some(launch_command);
        self.connect.oauth_state.status = Some(status);

        Ok(())
    }

    pub(crate) fn poll_claude_code_auth(&mut self) -> Result<bool> {
        let Some(credentials) = load_claude_credentials()? else {
            self.connect.oauth_state.status = Some(
                "Claude Code login not found yet. Finish `claude auth login --claudeai`, then press Enter again.".to_string(),
            );
            return Ok(false);
        };

        if credentials.access_token.trim().is_empty() {
            self.connect.oauth_state.status = Some(
                "Claude Code credentials were found, but the access token was empty.".to_string(),
            );
            return Ok(false);
        }

        let status = read_claude_auth_status();

        self.connect.oauth_state.access_token = Some(credentials.access_token);
        self.connect.oauth_state.refresh_token = credentials.refresh_token;
        self.connect.oauth_state.expires_at = credentials.expires_at;
        self.connect.oauth_state.scopes = credentials.scopes;
        self.connect.oauth_state.subscription_type = credentials.subscription_type.or_else(|| {
            status
                .as_ref()
                .and_then(|entry| entry.subscription_type.clone())
        });
        self.connect.oauth_state.rate_limit_tier = credentials.rate_limit_tier;
        self.connect.oauth_state.organization_id = status.and_then(|entry| entry.org_id);
        self.connect.oauth_state.status = Some(
            "Imported Claude Code login. Press Enter to continue to model selection.".to_string(),
        );

        Ok(true)
    }
}

pub(crate) fn claude_code_session_available() -> bool {
    load_claude_credentials()
        .ok()
        .flatten()
        .is_some_and(|credentials| !credentials.access_token.trim().is_empty())
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

fn claude_credentials_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join(".credentials.json");
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join(".credentials.json")
}
