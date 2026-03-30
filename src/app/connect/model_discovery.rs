use color_eyre::Result;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};

use crate::app::connect::ConnectProviderOption;
use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

#[derive(Debug, Clone, Default)]
pub(crate) struct ProviderModelMetadata {
    pub(crate) display_name: String,
    pub(crate) context_length: Option<usize>,
    pub(crate) supported_effort_levels: Vec<String>,
}

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
    #[serde(default)]
    npm: Option<String>,
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    name: String,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    context: usize,
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
    let remote = fetch_provider_model_metadata(provider_id).ok();
    model_ids
        .iter()
        .map(|model_id| {
            let display_name = remote
                .as_ref()
                .and_then(|map| map.get(model_id))
                .map(|metadata| metadata.display_name.clone())
                .unwrap_or_else(|| fallback_model_display_name(provider_id, model_id));
            (model_id.clone(), display_name)
        })
        .collect()
}

pub(crate) fn format_model_display_name(provider_id: &str, model_id: &str) -> String {
    fetch_provider_model_metadata(provider_id)
        .ok()
        .and_then(|map| {
            map.get(model_id)
                .map(|metadata| metadata.display_name.clone())
        })
        .unwrap_or_else(|| fallback_model_display_name(provider_id, model_id))
}

pub(crate) fn fallback_formatted_model_display_name(provider_id: &str, model_id: &str) -> String {
    fallback_model_display_name(provider_id, model_id)
}

fn fetch_openai_api_models(api_key: &str) -> Result<Vec<String>> {
    let api_key = api_key.trim().to_string();
    run_blocking_request(move || {
        let client = Client::new();
        let response = client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(api_key)
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
    })
}

fn fetch_openai_subscription_models(connection: &StoredConnection) -> Result<Vec<String>> {
    let Some(access_token) = connection.access_token.clone() else {
        return Ok(Vec::new());
    };

    let account_id = connection.account_id.clone();
    run_blocking_request(move || {
        let client = Client::new();
        let mut request = client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(access_token.trim());
        if let Some(account_id) = account_id.as_deref() {
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
    })
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

pub(crate) fn provider_model_metadata(
    provider_id: &str,
    model_id: &str,
) -> Option<ProviderModelMetadata> {
    provider_models_metadata(provider_id)
        .ok()
        .and_then(|map| map.get(model_id).cloned())
}

pub(crate) fn provider_models_metadata(
    provider_id: &str,
) -> Result<HashMap<String, ProviderModelMetadata>> {
    fetch_provider_model_metadata(provider_id)
}

fn fetch_provider_model_metadata(
    provider_id: &str,
) -> Result<HashMap<String, ProviderModelMetadata>> {
    let provider_id = provider_id.to_string();
    run_blocking_request(move || {
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
            .get(&provider_id)
            .map(|provider| {
                provider
                    .models
                    .iter()
                    .map(|(id, model)| {
                        (
                            id.clone(),
                            ProviderModelMetadata {
                                display_name: model.name.clone(),
                                context_length: model.limit.as_ref().map(|limit| limit.context),
                                supported_effort_levels: derive_supported_reasoning_efforts(
                                    &provider_id,
                                    provider.npm.as_deref(),
                                    id,
                                    model,
                                ),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default())
    })
}

fn run_blocking_request<T, F>(operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    std::thread::spawn(operation)
        .join()
        .map_err(|_| color_eyre::eyre::eyre!("blocking request thread panicked"))?
}

fn derive_supported_reasoning_efforts(
    provider_id: &str,
    provider_npm: Option<&str>,
    model_id: &str,
    model: &ModelsDevModel,
) -> Vec<String> {
    if !model.reasoning {
        return Vec::new();
    }

    const WIDELY_SUPPORTED_EFFORTS: &[&str] = &["low", "medium", "high"];
    let id = model_id.to_ascii_lowercase();
    let release_date = model.release_date.as_deref().unwrap_or("");

    match provider_npm.or(Some(provider_id)) {
        Some("@ai-sdk/openai") | Some("openai") => {
            if id == "gpt-5-pro" {
                return Vec::new();
            }

            if id.contains("codex") {
                let mut efforts = WIDELY_SUPPORTED_EFFORTS
                    .iter()
                    .map(|effort| (*effort).to_string())
                    .collect::<Vec<_>>();
                if release_date >= "2025-12-04" {
                    efforts.push("xhigh".to_string());
                }
                return efforts;
            }

            let mut efforts = WIDELY_SUPPORTED_EFFORTS
                .iter()
                .map(|effort| (*effort).to_string())
                .collect::<Vec<_>>();
            if id == "gpt-5" || id.starts_with("gpt-5.") || id.contains("gpt-5-") {
                efforts.insert(0, "minimal".to_string());
            }
            if release_date >= "2025-11-13" {
                efforts.insert(0, "none".to_string());
            }
            if release_date >= "2025-12-04" {
                efforts.push("xhigh".to_string());
            }
            efforts
        }
        Some("@ai-sdk/openai-compatible") | Some("openai-compatible") => WIDELY_SUPPORTED_EFFORTS
            .iter()
            .map(|effort| (*effort).to_string())
            .collect(),
        _ => Vec::new(),
    }
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
    use super::{
        ModelsDevLimit, ModelsDevModel, derive_supported_reasoning_efforts,
        fallback_model_display_name,
    };

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

    #[test]
    fn derives_openai_variants_from_models_dev_metadata() {
        let model = ModelsDevModel {
            name: "GPT-5.4".to_string(),
            reasoning: true,
            release_date: Some("2026-03-05".to_string()),
            limit: Some(ModelsDevLimit { context: 1_050_000 }),
        };

        assert_eq!(
            derive_supported_reasoning_efforts("openai", Some("@ai-sdk/openai"), "gpt-5.4", &model),
            vec!["none", "minimal", "low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn derives_openai_compatible_reasoning_efforts_from_models_dev_metadata() {
        let model = ModelsDevModel {
            name: "deepseek-chat".to_string(),
            reasoning: true,
            release_date: Some("2026-01-01".to_string()),
            limit: Some(ModelsDevLimit { context: 128_000 }),
        };

        assert_eq!(
            derive_supported_reasoning_efforts(
                "openai-compatible",
                Some("@ai-sdk/openai-compatible"),
                "deepseek-chat",
                &model,
            ),
            vec!["low", "medium", "high"]
        );
    }
}
