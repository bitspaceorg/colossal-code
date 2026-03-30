use serde_json::{Value, json};

#[derive(Debug, Clone, Default)]
pub(super) struct OpenAiResponsesOptions {
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    text_verbosity: Option<String>,
    service_tier: Option<String>,
    max_tool_calls: Option<u64>,
    parallel_tool_calls: Option<bool>,
}

impl OpenAiResponsesOptions {
    pub(super) fn from_env() -> Self {
        Self {
            reasoning_effort: env_non_empty("NITE_REASONING_EFFORT"),
            reasoning_summary: env_non_empty("NITE_OPENAI_REASONING_SUMMARY"),
            text_verbosity: env_choice("NITE_OPENAI_TEXT_VERBOSITY", &["low", "medium", "high"]),
            service_tier: env_non_empty("NITE_OPENAI_SERVICE_TIER"),
            max_tool_calls: env_non_empty("NITE_OPENAI_MAX_TOOL_CALLS")
                .and_then(|value| value.parse::<u64>().ok()),
            parallel_tool_calls: env_bool("NITE_OPENAI_PARALLEL_TOOL_CALLS"),
        }
    }

    pub(super) fn apply_to_responses_payload(&self, payload: &mut Value) {
        if self.reasoning_effort.is_some() || self.reasoning_summary.is_some() {
            let mut reasoning = serde_json::Map::new();
            if let Some(effort) = &self.reasoning_effort {
                reasoning.insert("effort".to_string(), json!(effort));
            }
            if let Some(summary) = &self.reasoning_summary {
                reasoning.insert("summary".to_string(), json!(summary));
            }
            payload["reasoning"] = Value::Object(reasoning);
        }

        if let Some(verbosity) = &self.text_verbosity {
            payload["text"] = json!({ "verbosity": verbosity });
        }
        if let Some(service_tier) = &self.service_tier {
            payload["service_tier"] = json!(service_tier);
        }
        if let Some(max_tool_calls) = self.max_tool_calls {
            payload["max_tool_calls"] = json!(max_tool_calls);
        }
        if let Some(parallel_tool_calls) = self.parallel_tool_calls {
            payload["parallel_tool_calls"] = json!(parallel_tool_calls);
        }
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_choice(key: &str, allowed: &[&str]) -> Option<String> {
    let value = env_non_empty(key)?;
    let normalized = value.to_ascii_lowercase();
    allowed
        .iter()
        .copied()
        .find(|candidate| *candidate == normalized)
        .map(ToOwned::to_owned)
}

fn env_bool(key: &str) -> Option<bool> {
    env_non_empty(key).and_then(|value| match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}
