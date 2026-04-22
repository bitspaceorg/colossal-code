mod activation;
mod claude_code;
pub(crate) mod model_discovery;
mod openai_subscription;
mod provider_oauth;
mod service;
mod types;

pub(crate) use openai_subscription::has_active_openai_device_session;
pub(crate) use types::{
    ConnectAuthMethod, ConnectModalMode, ConnectProviderOption, ConnectState,
    ConnectSubscriptionState, built_in_providers,
};
