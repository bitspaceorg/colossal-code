use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectModalMode {
    Providers,
    AuthMethod,
    ApiKey,
    Subscription,
    Models,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectAuthMethod {
    ApiKey,
    OpenAiSubscription,
    ClaudeCode,
}

impl ConnectAuthMethod {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "API key",
            Self::OpenAiSubscription => "ChatGPT Plus/Pro",
            Self::ClaudeCode => "Claude Pro/Max",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::ApiKey => "Direct API billing with OpenAI-compatible requests",
            Self::OpenAiSubscription => {
                "Uses your ChatGPT subscription through the Codex auth flow"
            }
            Self::ClaudeCode => {
                "Uses a Claude subscription token generated with `claude setup-token`"
            }
        }
    }
}

impl From<StoredAuthKind> for ConnectAuthMethod {
    fn from(value: StoredAuthKind) -> Self {
        match value {
            StoredAuthKind::ApiKey => Self::ApiKey,
            StoredAuthKind::OpenAiSubscription => Self::OpenAiSubscription,
            StoredAuthKind::ClaudeCode => Self::ClaudeCode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ConnectSubscriptionState {
    pub(crate) started: bool,
    pub(crate) verification_url: Option<String>,
    pub(crate) user_code: Option<String>,
    pub(crate) device_auth_id: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_at: Option<u64>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectProviderOption {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) api_key_hint: String,
    pub(crate) models: Vec<String>,
    pub(crate) auth_methods: Vec<ConnectAuthMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OAuthState {
    pub(crate) started: bool,
    pub(crate) launch_command: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_at: Option<u64>,
    pub(crate) scopes: Vec<String>,
    pub(crate) subscription_type: Option<String>,
    pub(crate) rate_limit_tier: Option<String>,
    pub(crate) organization_id: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectState {
    pub(crate) show_connect_modal: bool,
    pub(crate) mode: ConnectModalMode,
    pub(crate) provider_options: Vec<ConnectProviderOption>,
    pub(crate) selected_index: usize,
    pub(crate) filter: String,
    pub(crate) input: String,
    pub(crate) base_url_input: String,
    pub(crate) provider_name_input: String,
    pub(crate) input_cursor: usize,
    pub(crate) selected_provider: Option<ConnectProviderOption>,
    pub(crate) selected_auth_method: Option<ConnectAuthMethod>,
    pub(crate) subscription_state: ConnectSubscriptionState,
    pub(crate) oauth_state: OAuthState,
    pub(crate) available_models: Vec<String>,
    pub(crate) model_selected_index: usize,
    pub(crate) saved_connections: Vec<StoredConnection>,
    pub(crate) active_connection_id: Option<String>,
}

impl Default for ConnectState {
    fn default() -> Self {
        Self {
            show_connect_modal: false,
            mode: ConnectModalMode::Providers,
            provider_options: built_in_providers(),
            selected_index: 0,
            filter: String::new(),
            input: String::new(),
            base_url_input: String::new(),
            provider_name_input: String::new(),
            input_cursor: 0,
            selected_provider: None,
            selected_auth_method: None,
            subscription_state: ConnectSubscriptionState::default(),
            oauth_state: OAuthState::default(),
            available_models: Vec::new(),
            model_selected_index: 0,
            saved_connections: Vec::new(),
            active_connection_id: None,
        }
    }
}

pub(crate) fn built_in_providers() -> Vec<ConnectProviderOption> {
    vec![
        ConnectProviderOption {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            description: "ChatGPT and OpenAI API models".to_string(),
            api_key_hint: "Paste your OpenAI API key to continue.".to_string(),
            models: vec![
                "gpt-5.1-codex".to_string(),
                "gpt-5.1-codex-mini".to_string(),
                "gpt-5.2".to_string(),
                "gpt-5.2-codex".to_string(),
                "gpt-5.3-codex-spark".to_string(),
                "gpt-5.4".to_string(),
                "gpt-5.4-mini".to_string(),
                "gpt-5.3-codex".to_string(),
                "gpt-5.1-codex-max".to_string(),
                "gpt-5-codex".to_string(),
                "codex-mini".to_string(),
                "gpt-5.1".to_string(),
                "gpt-5".to_string(),
                "gpt-5-mini".to_string(),
                "gpt-4.1".to_string(),
            ],
            auth_methods: vec![
                ConnectAuthMethod::ApiKey,
                ConnectAuthMethod::OpenAiSubscription,
            ],
        },
        ConnectProviderOption {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            description: "Claude models with your Anthropic account".to_string(),
            api_key_hint: "Paste your Anthropic API key to continue.".to_string(),
            models: vec![
                "claude-sonnet-4-6".to_string(),
                "claude-opus-4-6".to_string(),
                "claude-haiku-4-5".to_string(),
                "claude-sonnet-4-5".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ClaudeCode, ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "openai-compatible".to_string(),
            name: "Custom OpenAI Compatible".to_string(),
            description: "Bring your own OpenAI-compatible endpoint".to_string(),
            api_key_hint: "Paste the API key for your compatible endpoint.".to_string(),
            models: vec![
                "gpt-4.1".to_string(),
                "deepseek-chat".to_string(),
                "qwen-coder-plus".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            description: "Open model router with many hosted providers".to_string(),
            api_key_hint: "Paste your OpenRouter API key.".to_string(),
            models: vec![
                "openai/gpt-5".to_string(),
                "anthropic/claude-sonnet-4.5".to_string(),
                "deepseek/deepseek-chat".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            description: "DeepSeek chat and reasoning models".to_string(),
            api_key_hint: "Paste your DeepSeek API key.".to_string(),
            models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "groq".to_string(),
            name: "Groq".to_string(),
            description: "Fast OpenAI-compatible inference".to_string(),
            api_key_hint: "Paste your Groq API key.".to_string(),
            models: vec![
                "llama-3.3-70b-versatile".to_string(),
                "moonshotai/kimi-k2-instruct".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            description: "Grok models through xAI".to_string(),
            api_key_hint: "Paste your xAI API key.".to_string(),
            models: vec!["grok-4".to_string(), "grok-code-fast-1".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "mistral".to_string(),
            name: "Mistral".to_string(),
            description: "Mistral API models".to_string(),
            api_key_hint: "Paste your Mistral API key.".to_string(),
            models: vec![
                "mistral-large-latest".to_string(),
                "mistral-medium-latest".to_string(),
                "codestral-latest".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "cerebras".to_string(),
            name: "Cerebras".to_string(),
            description: "Cerebras hosted OpenAI-compatible models".to_string(),
            api_key_hint: "Paste your Cerebras API key.".to_string(),
            models: vec!["qwen-3-coder-480b".to_string(), "llama3.1-8b".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "togetherai".to_string(),
            name: "Together AI".to_string(),
            description: "Together hosted open models".to_string(),
            api_key_hint: "Paste your Together API key.".to_string(),
            models: vec![
                "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
                "Qwen/Qwen2.5-Coder-32B-Instruct".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "perplexity".to_string(),
            name: "Perplexity".to_string(),
            description: "Sonar online search models".to_string(),
            api_key_hint: "Paste your Perplexity API key.".to_string(),
            models: vec!["sonar".to_string(), "sonar-pro".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "moonshotai".to_string(),
            name: "Moonshot AI".to_string(),
            description: "Kimi models through Moonshot".to_string(),
            api_key_hint: "Paste your Moonshot API key.".to_string(),
            models: vec![
                "kimi-k2-0711-preview".to_string(),
                "moonshot-v1-128k".to_string(),
            ],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "alibaba".to_string(),
            name: "Alibaba Cloud".to_string(),
            description: "Qwen models through DashScope".to_string(),
            api_key_hint: "Paste your DashScope API key.".to_string(),
            models: vec!["qwen-plus".to_string(), "qwen-coder-plus".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
        ConnectProviderOption {
            id: "nvidia".to_string(),
            name: "NVIDIA NIM".to_string(),
            description: "NVIDIA hosted OpenAI-compatible models".to_string(),
            api_key_hint: "Paste your NVIDIA API key.".to_string(),
            models: vec!["meta/llama-3.1-405b-instruct".to_string()],
            auth_methods: vec![ConnectAuthMethod::ApiKey],
        },
    ]
}
