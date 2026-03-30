use color_eyre::Result;

use crate::app::App;
use crate::app::connect::ConnectAuthMethod;
use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

impl App {
    pub(crate) fn start_selected_oauth_auth(&mut self) -> Result<()> {
        match self.connect.selected_auth_method {
            Some(ConnectAuthMethod::OpenAiSubscription) => self.start_openai_subscription_auth(),
            Some(ConnectAuthMethod::ClaudeCode) => self.start_claude_code_auth(),
            _ => Ok(()),
        }
    }

    pub(crate) fn poll_selected_oauth_auth(&mut self) -> Result<bool> {
        match self.connect.selected_auth_method {
            Some(ConnectAuthMethod::OpenAiSubscription) => self.poll_openai_subscription_auth(),
            Some(ConnectAuthMethod::ClaudeCode) => self.poll_claude_code_auth(),
            _ => Ok(false),
        }
    }

    pub(crate) fn oauth_state_from_connection(&mut self, connection: &StoredConnection) {
        self.connect.oauth_state.started = true;
        self.connect.oauth_state.account_id = connection.account_id.clone();
        self.connect.oauth_state.access_token = connection.access_token.clone();
        self.connect.oauth_state.refresh_token = connection.refresh_token.clone();
        self.connect.oauth_state.expires_at = connection.access_expires_at;
        self.connect.oauth_state.scopes = connection.oauth_scopes.clone();
        self.connect.oauth_state.subscription_type = connection.oauth_subscription_type.clone();
        self.connect.oauth_state.rate_limit_tier = connection.oauth_rate_limit_tier.clone();
        self.connect.oauth_state.organization_id = connection.organization_id.clone();
        self.connect.oauth_state.status = Some(
            "A saved OAuth-backed connection exists. You can re-authorize or continue.".to_string(),
        );
    }

    pub(crate) fn oauth_connection_ready(&self) -> bool {
        self.connect.oauth_state.access_token.is_some()
            && (self.connect.selected_auth_method != Some(ConnectAuthMethod::OpenAiSubscription)
                || self.connect.oauth_state.refresh_token.is_some())
    }

    pub(crate) fn stored_auth_kind_for_method(method: ConnectAuthMethod) -> StoredAuthKind {
        match method {
            ConnectAuthMethod::ApiKey => StoredAuthKind::ApiKey,
            ConnectAuthMethod::OpenAiSubscription => StoredAuthKind::OpenAiSubscription,
            ConnectAuthMethod::ClaudeCode => StoredAuthKind::ClaudeCode,
        }
    }
}
