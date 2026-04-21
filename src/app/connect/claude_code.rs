use std::process::Command;

use color_eyre::Result;
use serde::Deserialize;

use crate::app::App;

#[derive(Debug, Deserialize)]
pub(crate) struct ClaudeAuthStatus {
    #[serde(rename = "orgId")]
    pub(crate) org_id: Option<String>,
    #[serde(rename = "subscriptionType")]
    pub(crate) subscription_type: Option<String>,
}

impl App {
    pub(crate) fn start_claude_code_auth(&mut self) -> Result<()> {
        self.connect.oauth_state.started = true;
        self.connect.oauth_state.launch_command = Some("claude setup-token".to_string());
        self.connect.oauth_state.status = Some(
            "Run `claude setup-token` in another terminal, then paste the generated token below and press Enter here to save it in your OS keyring."
                .to_string(),
        );
        Ok(())
    }

    pub(crate) fn poll_claude_code_auth(&mut self) -> Result<bool> {
        let token = normalize_claude_setup_token(&self.connect.input);
        if token.is_empty() {
            self.connect.oauth_state.status = Some(
                "Paste the token produced by `claude setup-token`, then press Enter again."
                    .to_string(),
            );
            return Ok(false);
        }

        let status = read_claude_auth_status();

        self.connect.oauth_state.access_token = Some(token.clone());
        self.connect.oauth_state.refresh_token = None;
        self.connect.oauth_state.expires_at = None;
        self.connect.oauth_state.scopes = Vec::new();
        self.connect.oauth_state.subscription_type = status
            .as_ref()
            .and_then(|entry| entry.subscription_type.clone());
        self.connect.oauth_state.rate_limit_tier = None;
        self.connect.oauth_state.organization_id = status.and_then(|entry| entry.org_id);
        self.connect.oauth_state.status = Some(format!(
            "Imported Claude token ({} chars). Press Enter to continue to model selection.",
            token.chars().count()
        ));

        Ok(true)
    }
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

fn normalize_claude_setup_token(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let candidate = trimmed
        .lines()
        .rev()
        .find_map(extract_claude_token_candidate)
        .unwrap_or(trimmed);

    candidate
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .trim_start_matches("Bearer ")
        .trim()
        .to_string()
}

fn extract_claude_token_candidate(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    for prefix in [
        "export CLAUDE_CODE_OAUTH_TOKEN=",
        "CLAUDE_CODE_OAUTH_TOKEN=",
        "set -x CLAUDE_CODE_OAUTH_TOKEN ",
    ] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return Some(value.trim());
        }
    }

    Some(trimmed)
}
