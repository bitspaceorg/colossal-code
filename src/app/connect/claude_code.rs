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
pub(crate) struct ClaudeAuthStatus {
    #[serde(rename = "orgId")]
    pub org_id: Option<String>,
    #[serde(rename = "subscriptionType")]
    pub subscription_type: Option<String>,
}

impl App {
    pub(crate) fn start_claude_code_auth(&mut self) -> Result<()> {
        let launch_command = "claude auth login --claudeai".to_string();

        if claude_code_session_available() {
            self.connect.oauth_state.started = true;
            self.connect.oauth_state.launch_command = Some(launch_command);
            self.connect.oauth_state.status = Some(
                "Claude Code already has a login session. Press Enter here to import the account."
                    .to_string(),
            );
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("script")
                .args(["-q", "/dev/null", "claude", "auth", "login", "--claudeai"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }

        #[cfg(target_os = "linux")]
        {
            let _ = Command::new("script")
                .args(["-q", "-c", "claude auth login --claudeai", "/dev/null"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("claude")
                .args(["auth", "login", "--claudeai"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }

        self.connect.oauth_state.started = true;
        self.connect.oauth_state.launch_command = Some(launch_command);
        self.connect.oauth_state.status = Some(
            "Claude Code login started. If the browser didn't open, run `claude auth login --claudeai` in another terminal, then press Enter here.".to_string(),
        );

        Ok(())
    }

    pub(crate) fn poll_claude_code_auth(&mut self) -> Result<bool> {
        let credentials = load_claude_credentials()?;
        let auth_status = read_claude_auth_status();

        let has_token = credentials
            .as_ref()
            .is_some_and(|c| !c.access_token.trim().is_empty());

        let has_claude_ai = auth_status
            .as_ref()
            .and_then(|s| s.subscription_type.clone())
            .is_some();

        if !has_token && !has_claude_ai {
            self.connect.oauth_state.status = Some(
                "Claude Code login not found yet. Finish `claude auth login --claudeai`, then press Enter again.".to_string(),
            );
            return Ok(false);
        }

        let access_token = credentials.as_ref().and_then(|c| {
            if c.access_token.trim().is_empty() {
                None
            } else {
                Some(c.access_token.clone())
            }
        });

        self.connect.oauth_state.access_token = access_token;
        if let Some(cred) = &credentials {
            self.connect.oauth_state.refresh_token = cred.refresh_token.clone();
            self.connect.oauth_state.expires_at = cred.expires_at;
            self.connect.oauth_state.scopes = cred.scopes.clone();
            self.connect.oauth_state.rate_limit_tier = cred.rate_limit_tier.clone();
        }
        self.connect.oauth_state.subscription_type = credentials
            .as_ref()
            .and_then(|c| c.subscription_type.clone())
            .or_else(|| {
                auth_status
                    .as_ref()
                    .and_then(|s| s.subscription_type.clone())
            });
        self.connect.oauth_state.organization_id = auth_status.and_then(|s| s.org_id);
        self.connect.oauth_state.status = Some(
            "Imported Claude Code login. Press Enter to continue to model selection.".to_string(),
        );

        Ok(true)
    }
}

pub(crate) fn claude_code_session_available() -> bool {
    if let Some(credentials) = load_claude_credentials().ok().flatten() {
        if !credentials.access_token.trim().is_empty() {
            return true;
        }
    }
    read_claude_auth_status().is_some_and(|status| status.subscription_type.is_some())
}

fn load_claude_credentials() -> Result<Option<ClaudeAiOauthCredentials>> {
    if let Some(creds) = load_from_keychain()? {
        return Ok(Some(creds));
    }

    let path = claude_credentials_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    let store: ClaudeCredentialStore = serde_json::from_str(&content)?;
    Ok(store.claude_ai_oauth)
}

fn load_from_keychain() -> Result<Option<ClaudeAiOauthCredentials>> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()?;

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let store: ClaudeCredentialStore = serde_json::from_str(&json_str)?;
    Ok(store.claude_ai_oauth)
}

pub(crate) fn read_claude_auth_status() -> Option<ClaudeAuthStatus> {
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
