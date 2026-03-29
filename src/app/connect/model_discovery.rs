use color_eyre::Result;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};

use crate::app::connect::ConnectProviderOption;
use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    name: String,
}

pub(crate) fn resolve_provider_models(
    provider: &ConnectProviderOption,
    auth_kind: Option<StoredAuthKind>,
    api_key: Option<&str>,
    saved_connection: Option<&StoredConnection>,
) -> Result<Vec<String>> {
    if provider.id != "openai" {
        return Ok(provider.models.clone());
    }

    let fetched = match auth_kind.unwrap_or(StoredAuthKind::ApiKey) {
        StoredAuthKind::ApiKey => api_key
            .filter(|value| !value.trim().is_empty())
            .map(fetch_openai_api_models)
            .transpose()?,
        StoredAuthKind::OpenAiSubscription => match saved_connection {
            Some(connection) => Some(fetch_openai_subscription_models(connection)?),
            None => None,
        },
    };

    if let Some(models) = fetched.filter(|models| !models.is_empty()) {
        return Ok(models);
    }

    Ok(provider.models.clone())
}

pub(crate) fn resolve_model_display_names(
    provider_id: &str,
    model_ids: &[String],
) -> HashMap<String, String> {
    let remote = fetch_models_dev_provider_names(provider_id).ok();
    model_ids
        .iter()
        .map(|model_id| {
            let display_name = remote
                .as_ref()
                .and_then(|map| map.get(model_id))
                .cloned()
                .unwrap_or_else(|| fallback_model_display_name(provider_id, model_id));
            (model_id.clone(), display_name)
        })
        .collect()
}

pub(crate) fn format_model_display_name(provider_id: &str, model_id: &str) -> String {
    fetch_models_dev_provider_names(provider_id)
        .ok()
        .and_then(|map| map.get(model_id).cloned())
        .unwrap_or_else(|| fallback_model_display_name(provider_id, model_id))
}

pub(crate) fn fallback_formatted_model_display_name(provider_id: &str, model_id: &str) -> String {
    fallback_model_display_name(provider_id, model_id)
}

fn fetch_openai_api_models(api_key: &str) -> Result<Vec<String>> {
    let client = Client::new();
    let response = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key.trim())
        .send()?;
    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "OpenAI model discovery failed with status {}",
            response.status()
        ));
    }

    let payload: OpenAiModelsResponse = response.json()?;
    Ok(filter_openai_model_ids(
        payload.data.into_iter().map(|entry| entry.id),
    ))
}

fn fetch_openai_subscription_models(connection: &StoredConnection) -> Result<Vec<String>> {
    let Some(access_token) = connection.access_token.as_deref() else {
        return Ok(Vec::new());
    };

    let client = Client::new();
    let mut request = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(access_token.trim());
    if let Some(account_id) = connection.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request.send()?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }

    let payload: OpenAiModelsResponse = response.json()?;
    let mut models = filter_openai_model_ids(payload.data.into_iter().map(|entry| entry.id));
    let allowed = [
        "gpt-5.1-codex",
        "gpt-5.1-codex-max",
        "gpt-5.1-codex-mini",
        "gpt-5.2",
        "gpt-5.2-codex",
        "gpt-5.3-codex",
        "gpt-5.3-codex-spark",
        "gpt-5.4",
        "gpt-5.4-mini",
    ];
    for model in allowed {
        if !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    models.sort();
    Ok(models)
}

fn filter_openai_model_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut models = BTreeSet::new();
    for id in ids {
        let lowered = id.to_ascii_lowercase();
        let likely_text_model = lowered.starts_with("gpt-")
            || lowered.starts_with("o1")
            || lowered.starts_with("o3")
            || lowered.starts_with("o4")
            || lowered.contains("codex")
            || lowered == "codex-mini";
        let excluded = [
            "audio",
            "transcribe",
            "translation",
            "embedding",
            "whisper",
            "tts",
            "image",
            "dall",
            "moderation",
            "realtime",
            "search",
            "omni",
        ]
        .iter()
        .any(|needle| lowered.contains(needle));
        if likely_text_model && !excluded {
            models.insert(id);
        }
    }
    models.into_iter().collect()
}

fn fetch_models_dev_provider_names(provider_id: &str) -> Result<HashMap<String, String>> {
    let client = Client::new();
    let response = client
        .get("https://models.dev/api.json")
        .header(reqwest::header::USER_AGENT, "nite")
        .send()?;
    if !response.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "models.dev lookup failed with status {}",
            response.status()
        ));
    }

    let payload: HashMap<String, ModelsDevProvider> = response.json()?;
    Ok(payload
        .get(provider_id)
        .map(|provider| {
            provider
                .models
                .iter()
                .map(|(id, model)| (id.clone(), model.name.clone()))
                .collect()
        })
        .unwrap_or_default())
}

fn fallback_model_display_name(provider_id: &str, model_id: &str) -> String {
    if matches!(provider_id, "openai" | "codex") {
        return model_id
            .split('-')
            .map(format_openai_token)
            .collect::<Vec<_>>()
            .join("-");
    }
    model_id.to_string()
}

fn format_openai_token(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    if token.starts_with("gpt") || token.starts_with('o') {
        return token.to_ascii_uppercase();
    }
    if token == "mini" || token == "nano" || token == "max" {
        return token.to_string();
    }

    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut formatted = String::new();
    formatted.push(first.to_ascii_uppercase());
    formatted.push_str(chars.as_str());
    formatted
}

#[cfg(test)]
mod tests {
    use super::fallback_model_display_name;

    #[test]
    fn formats_openai_model_ids_when_catalog_is_missing() {
        assert_eq!(fallback_model_display_name("openai", "gpt-5.4"), "GPT-5.4");
        assert_eq!(
            fallback_model_display_name("openai", "gpt-5.1-codex-mini"),
            "GPT-5.1-Codex-mini"
        );
        assert_eq!(fallback_model_display_name("openai", "o4-mini"), "O4-mini");
    }

    #[test]
    fn leaves_unknown_provider_ids_unchanged() {
        assert_eq!(
            fallback_model_display_name("anthropic", "claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
    }
}
