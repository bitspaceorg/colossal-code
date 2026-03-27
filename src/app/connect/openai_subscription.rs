use color_eyre::Result;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::process::Command;

use crate::app::App;
use crate::app::connect::ConnectSubscriptionState;

const OPENAI_AUTH_ISSUER: &str = "https://auth.openai.com";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Deserialize)]
struct OpenAiDeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiDeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiTokenResponse {
    id_token: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiTokenClaims {
    chatgpt_account_id: Option<String>,
    organizations: Option<Vec<OpenAiOrganizationClaim>>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<OpenAiAuthClaim>,
}

#[derive(Debug, Deserialize)]
struct OpenAiOrganizationClaim {
    id: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiAuthClaim {
    chatgpt_account_id: Option<String>,
}

impl App {
    pub(crate) fn start_openai_subscription_auth(&mut self) -> Result<()> {
        let client = Client::new();
        let response = client
            .post(format!(
                "{OPENAI_AUTH_ISSUER}/api/accounts/deviceauth/usercode"
            ))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "client_id": OPENAI_CLIENT_ID }))
            .send()?;
        if !response.status().is_success() {
            return Err(color_eyre::eyre::eyre!(
                "Device authorization failed with status {}",
                response.status()
            ));
        }

        let data: OpenAiDeviceCodeResponse = response.json()?;
        let verification_url = format!("{OPENAI_AUTH_ISSUER}/codex/device");
        let _ = open_url(&verification_url);

        self.connect.subscription_state = ConnectSubscriptionState {
            started: true,
            verification_url: Some(verification_url),
            user_code: Some(data.user_code.clone()),
            device_auth_id: Some(data.device_auth_id),
            account_id: None,
            access_token: None,
            refresh_token: None,
            expires_at: None,
            status: Some(format!(
                "Open the browser page, enter code {}, then press Enter here.",
                data.user_code
            )),
        };
        Ok(())
    }

    pub(crate) fn poll_openai_subscription_auth(&mut self) -> Result<bool> {
        let device_auth_id = self
            .connect
            .subscription_state
            .device_auth_id
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("No device auth session in progress"))?;
        let user_code = self
            .connect
            .subscription_state
            .user_code
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("Missing device auth user code"))?;

        let client = Client::new();
        let response = client
            .post(format!(
                "{OPENAI_AUTH_ISSUER}/api/accounts/deviceauth/token"
            ))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_auth_id": device_auth_id,
                "user_code": user_code,
            }))
            .send()?;

        if response.status() == reqwest::StatusCode::NOT_FOUND
            || response.status() == reqwest::StatusCode::BAD_REQUEST
        {
            self.connect.subscription_state.status = Some(
                "Authorization still pending. Finish browser auth, then press Enter again."
                    .to_string(),
            );
            return Ok(false);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(color_eyre::eyre::eyre!(
                "Device token polling failed ({status}): {body}"
            ));
        }

        let device_data: OpenAiDeviceTokenResponse = response.json()?;
        let token_response = client
            .post(format!("{OPENAI_AUTH_ISSUER}/oauth/token"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", device_data.authorization_code.as_str()),
                ("client_id", OPENAI_CLIENT_ID),
                ("code_verifier", device_data.code_verifier.as_str()),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ])
            .send()?;
        if !token_response.status().is_success() {
            return Err(color_eyre::eyre::eyre!(
                "OpenAI token exchange failed with status {}",
                token_response.status()
            ));
        }

        let tokens: OpenAiTokenResponse = token_response.json()?;
        let account_id = tokens
            .id_token
            .as_deref()
            .and_then(extract_openai_account_id)
            .or_else(|| extract_openai_account_id(&tokens.access_token));

        self.connect.subscription_state.account_id = account_id;
        self.connect.subscription_state.access_token = Some(tokens.access_token);
        self.connect.subscription_state.refresh_token = Some(tokens.refresh_token);
        self.connect.subscription_state.expires_at = Some(
            crate::app::persistence::auth_store::current_unix_timestamp()
                + tokens.expires_in.unwrap_or(3600),
        );
        self.connect.subscription_state.status = Some(
            "Authorized. Press Enter to continue to model selection and save the subscription."
                .to_string(),
        );
        Ok(true)
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", url]).spawn()?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

fn extract_openai_account_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: OpenAiTokenClaims = serde_json::from_slice(&bytes).ok()?;
    claims
        .chatgpt_account_id
        .or_else(|| claims.auth.and_then(|auth| auth.chatgpt_account_id))
        .or_else(|| {
            claims
                .organizations
                .and_then(|mut organizations| organizations.pop().map(|org| org.id))
        })
}
