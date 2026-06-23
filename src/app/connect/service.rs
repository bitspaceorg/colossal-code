use color_eyre::Result;

use crate::app::App;
use crate::app::connect::claude_code::read_claude_auth_status;
use crate::app::connect::model_discovery::{
    fallback_formatted_model_display_name, provider_model_metadata, resolve_provider_models,
};
use crate::app::connect::{
    ConnectAuthMethod, ConnectModalMode, ConnectProviderOption, ConnectSubscriptionState,
    built_in_providers,
};
use crate::app::persistence::auth_store::{
    AuthStore, StoredAuthKind, StoredConnection, current_unix_timestamp, save_auth_store,
};
use crate::app::persistence::provider_files::{
    ProviderFile, provider_id_from_name, save_provider_file,
};

fn require_claude_code_access_token(access_token: Option<&str>) -> Result<()> {
    if access_token.is_some_and(|token| !token.trim().is_empty()) {
        return Ok(());
    }

    Err(color_eyre::eyre::eyre!(
        "Claude Code authorization has not completed yet"
    ))
}

fn saved_openai_subscription_state(connection: &StoredConnection) -> ConnectSubscriptionState {
    ConnectSubscriptionState {
        started: false,
        verification_url: None,
        user_code: None,
        device_auth_id: None,
        account_id: connection.account_id.clone(),
        access_token: connection.access_token.clone(),
        refresh_token: connection.refresh_token.clone(),
        expires_at: connection.access_expires_at,
        status: Some(
            "A saved subscription connection exists. You can re-authorize or continue.".to_string(),
        ),
    }
}

impl App {
    pub(crate) fn current_model_provider_id(&self) -> Option<&str> {
        self.active_connection()
            .map(|connection| connection.provider_id.as_str())
    }

    pub(crate) fn current_model_metadata(&self) -> Option<&crate::app::ModelInfo> {
        let model_id = self.current_model.as_deref()?.trim();
        if model_id.is_empty() {
            return None;
        }

        let active_connection_id = self.connect.active_connection_id.as_deref();
        self.available_models.iter().find(|model| {
            model.filename == model_id
                && match (model.connection_id.as_deref(), active_connection_id) {
                    (Some(model_connection), Some(active_connection)) => {
                        model_connection == active_connection
                    }
                    (None, None) => true,
                    (None, Some(_)) => false,
                    (Some(_), None) => false,
                }
        })
    }

    pub(crate) fn current_model_supported_effort_levels(&self) -> Vec<String> {
        if let Some(model) = self.current_model_metadata() {
            return model.supported_effort_levels.clone();
        }

        let model_id = match self.current_model.as_deref().map(str::trim) {
            Some("") | None => return Vec::new(),
            Some(model_id) => model_id,
        };

        self.current_model_provider_id()
            .and_then(|provider_id| provider_model_metadata(provider_id, model_id))
            .map(|metadata| metadata.supported_effort_levels)
            .unwrap_or_default()
    }

    pub(crate) fn current_model_supports_reasoning(&self) -> bool {
        !self.current_model_supported_effort_levels().is_empty()
    }

    pub(crate) fn sanitized_connect_api_key(&self) -> String {
        self.connect.input.trim().to_string()
    }

    pub(crate) fn sanitized_connect_base_url(&self) -> String {
        self.connect
            .base_url_input
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    pub(crate) fn selected_provider_needs_base_url_input(&self) -> bool {
        self.connect
            .selected_provider
            .as_ref()
            .is_some_and(|provider| provider.id == "openai-compatible")
    }

    pub(crate) fn selected_provider_needs_name_input(&self) -> bool {
        self.selected_provider_needs_base_url_input()
    }

    pub(crate) fn sanitized_connect_provider_name(&self) -> String {
        self.connect.provider_name_input.trim().to_string()
    }

    pub(crate) fn select_connect_api_key_field(&mut self, selected_index: usize) {
        self.connect.selected_index =
            selected_index.min(if self.selected_provider_needs_name_input() {
                2
            } else if self.selected_provider_needs_base_url_input() {
                1
            } else {
                0
            });
        self.connect.input_cursor = match self.connect.selected_index {
            0 if self.selected_provider_needs_name_input() => {
                self.connect.provider_name_input.chars().count()
            }
            1 if self.selected_provider_needs_base_url_input() => {
                self.connect.base_url_input.chars().count()
            }
            _ => self.connect.input.chars().count(),
        };
    }

    pub(crate) fn active_connect_api_key_field_len(&self) -> usize {
        match self.connect.selected_index {
            0 if self.selected_provider_needs_name_input() => {
                self.connect.provider_name_input.chars().count()
            }
            1 if self.selected_provider_needs_base_url_input() => {
                self.connect.base_url_input.chars().count()
            }
            _ => self.connect.input.chars().count(),
        }
    }

    pub(crate) fn open_connect_modal(&mut self) {
        self.connect.show_connect_modal = true;
        self.connect.mode = ConnectModalMode::Providers;
        self.connect.provider_options = built_in_providers();
        self.connect.selected_index = 0;
        self.connect.filter.clear();
        self.connect.input.clear();
        self.connect.base_url_input.clear();
        self.connect.provider_name_input.clear();
        self.connect.input_cursor = 0;
        self.connect.selected_provider = None;
        self.connect.selected_auth_method = None;
        self.connect.subscription_state = ConnectSubscriptionState::default();
        self.connect.available_models.clear();
        self.connect.model_selected_index = 0;
        self.autocomplete_active = false;
        self.autocomplete_suggestions.clear();
        self.autocomplete_selected_index = 0;
    }

    pub(crate) fn close_connect_modal(&mut self) {
        self.connect.show_connect_modal = false;
        self.connect.mode = ConnectModalMode::Providers;
        self.connect.filter.clear();
        self.connect.input.clear();
        self.connect.base_url_input.clear();
        self.connect.provider_name_input.clear();
        self.connect.input_cursor = 0;
        self.connect.selected_provider = None;
        self.connect.selected_auth_method = None;
        self.connect.subscription_state = ConnectSubscriptionState::default();
        self.connect.available_models.clear();
        self.connect.selected_index = 0;
        self.connect.model_selected_index = 0;
    }

    pub(crate) fn filtered_connect_providers(&self) -> Vec<ConnectProviderOption> {
        let query = self.connect.filter.trim().to_lowercase();
        if query.is_empty() {
            return self.connect.provider_options.clone();
        }

        self.connect
            .provider_options
            .iter()
            .filter(|provider| {
                provider.name.to_lowercase().contains(&query)
                    || provider.id.to_lowercase().contains(&query)
                    || provider.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    pub(crate) fn active_connection(&self) -> Option<&StoredConnection> {
        self.connect.active_connection_id.as_deref().and_then(|id| {
            self.connect
                .saved_connections
                .iter()
                .find(|connection| connection.id == id)
        })
    }

    pub(crate) fn active_model_display_name(&self) -> Option<String> {
        let model_id = self.current_model.as_deref()?.trim();
        if model_id.is_empty() {
            return None;
        }

        let provider_id = self
            .active_connection()
            .map(|connection| connection.provider_id.as_str())
            .unwrap_or("local");
        Some(fallback_formatted_model_display_name(provider_id, model_id))
    }

    pub(crate) fn auth_methods_for_selected_provider(&self) -> Vec<ConnectAuthMethod> {
        self.connect
            .selected_provider
            .as_ref()
            .map(|provider| provider.auth_methods.clone())
            .unwrap_or_default()
    }

    pub(crate) fn begin_provider_auth_flow(&mut self, provider: ConnectProviderOption) {
        let saved = self
            .connect
            .saved_connections
            .iter()
            .find(|connection| connection.provider_id == provider.id)
            .cloned();

        self.connect.selected_provider = Some(provider.clone());
        self.connect.selected_auth_method = saved
            .as_ref()
            .map(|connection| ConnectAuthMethod::from(connection.auth_kind.clone()));
        self.connect.subscription_state = ConnectSubscriptionState::default();
        self.connect.oauth_state = Default::default();
        if let Some(connection) = saved.as_ref()
            && connection.auth_kind == StoredAuthKind::OpenAiSubscription
        {
            self.connect.subscription_state = saved_openai_subscription_state(connection);
        }
        if let Some(connection) = saved.as_ref()
            && connection.auth_kind == StoredAuthKind::ClaudeCode
        {
            self.oauth_state_from_connection(connection);
        }

        // Don't block on fetching models here - use provider.models as fallback
        // Models will be fetched when user runs /model if needed
        self.connect.available_models = provider.models.clone();
        self.connect.model_selected_index = saved
            .as_ref()
            .and_then(|connection| connection.model.as_ref())
            .and_then(|model| {
                self.connect
                    .available_models
                    .iter()
                    .position(|candidate| candidate == model)
            })
            .unwrap_or(0);
        self.connect.input = saved
            .as_ref()
            .and_then(|connection| connection.api_key.clone())
            .unwrap_or_default();
        self.connect.base_url_input = saved
            .as_ref()
            .and_then(|connection| connection.base_url.clone())
            .or_else(|| default_base_url_for_provider(&provider.id))
            .unwrap_or_default();
        self.connect.provider_name_input = if provider.id == "openai-compatible" {
            saved
                .as_ref()
                .map(|connection| connection.provider_name.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.connect.selected_index = 0;
        self.connect.input_cursor = if provider.id == "openai-compatible" {
            self.connect.provider_name_input.chars().count()
        } else {
            self.connect.input.chars().count()
        };

        self.connect.mode = if provider.auth_methods.len() > 1 {
            ConnectModalMode::AuthMethod
        } else {
            self.connect.selected_auth_method = provider.auth_methods.first().copied();
            ConnectModalMode::ApiKey
        };
    }

    pub(crate) fn save_connect_selection(&mut self) -> Result<StoredConnection> {
        let provider = self
            .connect
            .selected_provider
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("No provider selected"))?;
        let auth_method = self
            .connect
            .selected_auth_method
            .unwrap_or(ConnectAuthMethod::ApiKey);

        let now = current_unix_timestamp();
        let is_custom_provider = provider.id == "openai-compatible";
        let custom_name = if is_custom_provider {
            let name = self.sanitized_connect_provider_name();
            if name.is_empty() {
                return Err(color_eyre::eyre::eyre!("Provider name is empty"));
            }
            Some(name)
        } else {
            None
        };
        let id = custom_name
            .as_deref()
            .map(provider_id_from_name)
            .unwrap_or_else(|| provider.id.clone());
        let created_at = self
            .connect
            .saved_connections
            .iter()
            .find(|connection| connection.id == id)
            .map(|connection| connection.created_at)
            .unwrap_or(now);
        let model = self
            .connect
            .available_models
            .get(self.connect.model_selected_index)
            .cloned();

        let connection = match auth_method {
            ConnectAuthMethod::ApiKey => {
                let api_key = self.sanitized_connect_api_key();
                if api_key.is_empty() {
                    return Err(color_eyre::eyre::eyre!("API key is empty"));
                }
                let base_url = if is_custom_provider {
                    let base_url = self.sanitized_connect_base_url();
                    if base_url.is_empty() {
                        return Err(color_eyre::eyre::eyre!("Base URL is empty"));
                    }
                    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
                        return Err(color_eyre::eyre::eyre!(
                            "Base URL must start with http:// or https://"
                        ));
                    }
                    Some(base_url)
                } else {
                    default_base_url_for_provider(&provider.id)
                };
                StoredConnection {
                    id: id.clone(),
                    provider_id: id.clone(),
                    provider_name: custom_name.clone().unwrap_or(provider.name),
                    auth_kind: StoredAuthKind::ApiKey,
                    api_key: Some(api_key),
                    model,
                    base_url,
                    completions_path: default_completions_path_for_provider(&provider.id),
                    account_id: None,
                    access_token: None,
                    refresh_token: None,
                    access_expires_at: None,
                    oauth_scopes: Vec::new(),
                    oauth_subscription_type: None,
                    oauth_rate_limit_tier: None,
                    organization_id: None,
                    created_at,
                    updated_at: now,
                }
            }
            ConnectAuthMethod::OpenAiSubscription => {
                if self.connect.subscription_state.access_token.is_none()
                    || self.connect.subscription_state.refresh_token.is_none()
                {
                    return Err(color_eyre::eyre::eyre!(
                        "Subscription authorization has not completed yet"
                    ));
                }
                StoredConnection {
                    id: id.clone(),
                    provider_id: provider.id,
                    provider_name: provider.name,
                    auth_kind: StoredAuthKind::OpenAiSubscription,
                    api_key: None,
                    model,
                    base_url: Some("https://chatgpt.com".to_string()),
                    completions_path: Some("/backend-api/codex/responses".to_string()),
                    account_id: self.connect.subscription_state.account_id.clone(),
                    access_token: self.connect.subscription_state.access_token.clone(),
                    refresh_token: self.connect.subscription_state.refresh_token.clone(),
                    access_expires_at: self.connect.subscription_state.expires_at,
                    oauth_scopes: Vec::new(),
                    oauth_subscription_type: None,
                    oauth_rate_limit_tier: None,
                    organization_id: None,
                    created_at,
                    updated_at: now,
                }
            }
            ConnectAuthMethod::ClaudeCode => {
                require_claude_code_access_token(self.connect.oauth_state.access_token.as_deref())?;
                let auth_status = read_claude_auth_status();
                let subscription_type =
                    self.connect
                        .oauth_state
                        .subscription_type
                        .clone()
                        .or_else(|| {
                            auth_status
                                .as_ref()
                                .and_then(|s| s.subscription_type.clone())
                        });
                StoredConnection {
                    id: id.clone(),
                    provider_id: provider.id,
                    provider_name: provider.name,
                    auth_kind: StoredAuthKind::ClaudeCode,
                    api_key: None,
                    model,
                    base_url: Some("https://api.anthropic.com".to_string()),
                    completions_path: Some("/v1/messages".to_string()),
                    account_id: None,
                    access_token: self.connect.oauth_state.access_token.clone(),
                    refresh_token: self.connect.oauth_state.refresh_token.clone(),
                    access_expires_at: self.connect.oauth_state.expires_at,
                    oauth_scopes: self.connect.oauth_state.scopes.clone(),
                    oauth_subscription_type: subscription_type,
                    oauth_rate_limit_tier: self.connect.oauth_state.rate_limit_tier.clone(),
                    organization_id: self
                        .connect
                        .oauth_state
                        .organization_id
                        .clone()
                        .or_else(|| auth_status.and_then(|s| s.org_id)),
                    created_at,
                    updated_at: now,
                }
            }
        };

        let mut store = AuthStore {
            version: 1,
            active_connection_id: Some(id),
            connections: self.connect.saved_connections.clone(),
        };
        store.upsert_connection(connection.clone());
        save_auth_store(&store)?;

        if let (true, Some(name), Some(url_base)) = (
            is_custom_provider,
            custom_name.as_ref(),
            connection.base_url.clone(),
        ) {
            save_provider_file(&ProviderFile {
                name: name.clone(),
                url_base,
            })?;
        }

        self.connect.saved_connections = store.connections;
        self.connect.active_connection_id = store.active_connection_id;
        Ok(connection)
    }

    pub(crate) fn refresh_selected_provider_models(&mut self) -> Result<()> {
        let provider = self
            .connect
            .selected_provider
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("No provider selected"))?;
        let auth_method = self
            .connect
            .selected_auth_method
            .unwrap_or(ConnectAuthMethod::ApiKey);
        let auth_kind = Self::stored_auth_kind_for_method(auth_method);
        let api_key = self.sanitized_connect_api_key();

        let temp_saved = match auth_method {
            ConnectAuthMethod::ApiKey => None,
            ConnectAuthMethod::OpenAiSubscription => Some(StoredConnection {
                id: provider.id.clone(),
                provider_id: provider.id.clone(),
                provider_name: provider.name.clone(),
                auth_kind: StoredAuthKind::OpenAiSubscription,
                api_key: None,
                model: None,
                base_url: Some("https://chatgpt.com".to_string()),
                completions_path: Some("/backend-api/codex/responses".to_string()),
                account_id: self.connect.subscription_state.account_id.clone(),
                access_token: self.connect.subscription_state.access_token.clone(),
                refresh_token: self.connect.subscription_state.refresh_token.clone(),
                access_expires_at: self.connect.subscription_state.expires_at,
                oauth_scopes: Vec::new(),
                oauth_subscription_type: None,
                oauth_rate_limit_tier: None,
                organization_id: None,
                created_at: 0,
                updated_at: 0,
            }),
            ConnectAuthMethod::ClaudeCode => Some(StoredConnection {
                id: provider.id.clone(),
                provider_id: provider.id.clone(),
                provider_name: provider.name.clone(),
                auth_kind: StoredAuthKind::ClaudeCode,
                api_key: None,
                model: None,
                base_url: Some("https://api.anthropic.com".to_string()),
                completions_path: Some("/v1/messages".to_string()),
                account_id: None,
                access_token: self.connect.oauth_state.access_token.clone(),
                refresh_token: self.connect.oauth_state.refresh_token.clone(),
                access_expires_at: self.connect.oauth_state.expires_at,
                oauth_scopes: self.connect.oauth_state.scopes.clone(),
                oauth_subscription_type: self.connect.oauth_state.subscription_type.clone(),
                oauth_rate_limit_tier: self.connect.oauth_state.rate_limit_tier.clone(),
                organization_id: self.connect.oauth_state.organization_id.clone(),
                created_at: 0,
                updated_at: 0,
            }),
        };

        let resolved = resolve_provider_models(
            &provider,
            Some(auth_kind),
            Some(api_key.as_str()),
            temp_saved.as_ref(),
        )?;
        if !resolved.is_empty() {
            self.connect.available_models = resolved;
            self.connect.model_selected_index = 0;
        }
        Ok(())
    }

    pub(crate) fn handle_connect_paste(&mut self, data: String) {
        match self.connect.mode {
            ConnectModalMode::Providers => {
                Self::insert_at_char(&mut self.connect.filter, self.connect.input_cursor, &data);
                self.connect.input_cursor += data.chars().count();
                self.clamp_connect_provider_selection();
            }
            ConnectModalMode::ApiKey => {
                if self.connect.selected_index == 0 && self.selected_provider_needs_name_input() {
                    Self::insert_at_char(
                        &mut self.connect.provider_name_input,
                        self.connect.input_cursor,
                        &data,
                    );
                } else if self.connect.selected_index == 1
                    && self.selected_provider_needs_base_url_input()
                {
                    Self::insert_at_char(
                        &mut self.connect.base_url_input,
                        self.connect.input_cursor,
                        &data,
                    );
                } else {
                    Self::insert_at_char(&mut self.connect.input, self.connect.input_cursor, &data);
                }
                self.connect.input_cursor += data.chars().count();
            }
            ConnectModalMode::Subscription
                if self.connect.selected_auth_method
                    == Some(crate::app::connect::ConnectAuthMethod::ClaudeCode)
                    && !self.connect.oauth_state.access_token.is_some() =>
            {
                Self::insert_at_char(&mut self.connect.input, self.connect.input_cursor, &data);
                self.connect.input_cursor += data.chars().count();
            }
            ConnectModalMode::AuthMethod
            | ConnectModalMode::Subscription
            | ConnectModalMode::Models => {}
        }
    }

    pub(crate) fn insert_connect_char(&mut self, c: char) {
        match self.connect.mode {
            ConnectModalMode::Providers => {
                Self::insert_at_char(
                    &mut self.connect.filter,
                    self.connect.input_cursor,
                    &c.to_string(),
                );
                self.connect.input_cursor += 1;
                self.clamp_connect_provider_selection();
            }
            ConnectModalMode::ApiKey => {
                if self.connect.selected_index == 0 && self.selected_provider_needs_name_input() {
                    Self::insert_at_char(
                        &mut self.connect.provider_name_input,
                        self.connect.input_cursor,
                        &c.to_string(),
                    );
                } else if self.connect.selected_index == 1
                    && self.selected_provider_needs_base_url_input()
                {
                    Self::insert_at_char(
                        &mut self.connect.base_url_input,
                        self.connect.input_cursor,
                        &c.to_string(),
                    );
                } else {
                    Self::insert_at_char(
                        &mut self.connect.input,
                        self.connect.input_cursor,
                        &c.to_string(),
                    );
                }
                self.connect.input_cursor += 1;
            }
            ConnectModalMode::AuthMethod
            | ConnectModalMode::Subscription
            | ConnectModalMode::Models => {}
        }
    }

    pub(crate) fn backspace_connect_input(&mut self) {
        match self.connect.mode {
            ConnectModalMode::Providers => {
                if self.connect.input_cursor == 0 {
                    return;
                }
                Self::remove_at_char(&mut self.connect.filter, self.connect.input_cursor - 1);
                self.connect.input_cursor -= 1;
                self.clamp_connect_provider_selection();
            }
            ConnectModalMode::ApiKey => {
                if self.connect.input_cursor == 0 {
                    return;
                }
                if self.connect.selected_index == 0 && self.selected_provider_needs_name_input() {
                    Self::remove_at_char(
                        &mut self.connect.provider_name_input,
                        self.connect.input_cursor - 1,
                    );
                } else if self.connect.selected_index == 1
                    && self.selected_provider_needs_base_url_input()
                {
                    Self::remove_at_char(
                        &mut self.connect.base_url_input,
                        self.connect.input_cursor - 1,
                    );
                } else {
                    Self::remove_at_char(&mut self.connect.input, self.connect.input_cursor - 1);
                }
                self.connect.input_cursor -= 1;
            }
            ConnectModalMode::AuthMethod
            | ConnectModalMode::Subscription
            | ConnectModalMode::Models => {}
        }
    }

    pub(crate) fn clamp_connect_provider_selection(&mut self) {
        let len = self.filtered_connect_providers().len();
        if len == 0 {
            self.connect.selected_index = 0;
            return;
        }
        if self.connect.selected_index >= len {
            self.connect.selected_index = len - 1;
        }
    }

    fn insert_at_char(target: &mut String, char_idx: usize, text: &str) {
        let byte_idx = Self::char_to_byte_index(target, char_idx);
        target.insert_str(byte_idx, text);
    }

    fn remove_at_char(target: &mut String, char_idx: usize) {
        let start = Self::char_to_byte_index(target, char_idx);
        let end = Self::char_to_byte_index(target, char_idx + 1);
        target.replace_range(start..end, "");
    }

    fn char_to_byte_index(text: &str, char_idx: usize) -> usize {
        text.char_indices()
            .nth(char_idx)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len())
    }
}

fn default_base_url_for_provider(provider_id: &str) -> Option<String> {
    match provider_id {
        "openai" => Some("https://api.openai.com".to_string()),
        "anthropic" => Some("https://api.anthropic.com".to_string()),
        "openrouter" => Some("https://openrouter.ai/api".to_string()),
        "deepseek" => Some("https://api.deepseek.com".to_string()),
        "groq" => Some("https://api.groq.com/openai".to_string()),
        "xai" => Some("https://api.x.ai".to_string()),
        "mistral" => Some("https://api.mistral.ai".to_string()),
        "cerebras" => Some("https://api.cerebras.ai".to_string()),
        "togetherai" => Some("https://api.together.xyz".to_string()),
        "perplexity" => Some("https://api.perplexity.ai".to_string()),
        "moonshotai" => Some("https://api.moonshot.ai".to_string()),
        "alibaba" => Some("https://dashscope.aliyuncs.com/compatible-mode".to_string()),
        "nvidia" => Some("https://integrate.api.nvidia.com".to_string()),
        _ => None,
    }
}

fn default_completions_path_for_provider(provider_id: &str) -> Option<String> {
    match provider_id {
        "openai" => Some("/v1/chat/completions".to_string()),
        "anthropic" => Some("/v1/messages".to_string()),
        "openai-compatible" => Some("/v1/chat/completions".to_string()),
        "openrouter" | "deepseek" | "groq" | "xai" | "mistral" | "cerebras" | "togetherai"
        | "moonshotai" | "alibaba" | "nvidia" => Some("/v1/chat/completions".to_string()),
        "perplexity" => Some("/chat/completions".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_base_url_for_provider, default_completions_path_for_provider,
        require_claude_code_access_token, saved_openai_subscription_state,
    };
    use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

    #[test]
    fn claude_code_save_requires_non_empty_access_token() {
        assert!(require_claude_code_access_token(Some("token")).is_ok());
        assert!(require_claude_code_access_token(Some("   ")).is_err());
        assert!(require_claude_code_access_token(None).is_err());
    }

    #[test]
    fn saved_openai_subscription_does_not_mark_device_flow_started() {
        let connection = StoredConnection {
            id: "openai".to_string(),
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            auth_kind: StoredAuthKind::OpenAiSubscription,
            api_key: None,
            model: None,
            base_url: Some("https://chatgpt.com".to_string()),
            completions_path: Some("/backend-api/codex/responses".to_string()),
            account_id: Some("acct".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            access_expires_at: Some(123),
            oauth_scopes: Vec::new(),
            oauth_subscription_type: None,
            oauth_rate_limit_tier: None,
            organization_id: None,
            created_at: 1,
            updated_at: 1,
        };

        let state = saved_openai_subscription_state(&connection);
        assert!(!state.started);
        assert_eq!(state.account_id.as_deref(), Some("acct"));
        assert_eq!(state.user_code, None);
        assert_eq!(state.device_auth_id, None);
        assert_eq!(state.access_token.as_deref(), Some("access"));
        assert_eq!(state.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn api_key_provider_defaults_route_to_their_own_backends() {
        assert_eq!(
            default_base_url_for_provider("openai").as_deref(),
            Some("https://api.openai.com")
        );
        assert_eq!(
            default_completions_path_for_provider("openai").as_deref(),
            Some("/v1/chat/completions")
        );
        assert_eq!(
            default_base_url_for_provider("anthropic").as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            default_completions_path_for_provider("anthropic").as_deref(),
            Some("/v1/messages")
        );
        assert_eq!(default_base_url_for_provider("openai-compatible"), None);
        assert_eq!(
            default_completions_path_for_provider("openai-compatible").as_deref(),
            Some("/v1/chat/completions")
        );
    }
}
