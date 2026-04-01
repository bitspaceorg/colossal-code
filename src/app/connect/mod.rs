mod activation;
mod claude_code;
pub(crate) mod model_discovery;
mod openai_subscription;
mod provider_oauth;
mod service;
mod types;

pub(crate) use types::{
    ConnectAuthMethod, ConnectModalMode, ConnectProviderOption, ConnectState,
    ConnectSubscriptionState, built_in_providers,
};
