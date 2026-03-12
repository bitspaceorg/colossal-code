use agent_core::model_config::ModelConfig;
use serde_json::Value;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::app::init::model_context::gguf::context_length_from_gguf;

pub fn detect_context_length(models_dir: Option<&Path>, identifier: &str) -> Option<usize> {
    if identifier.trim().is_empty() {
        return None;
    }

    if let Some(base) = models_dir
        && let Some(length) = detect_from_filesystem(base, identifier)
    {
        return Some(length);
    }

    let candidates = candidate_model_names(identifier);
    if let Some(length) = context_length_from_configs(&candidates) {
        return Some(length);
    }

    env_context_length()
}

fn detect_from_filesystem(base: &Path, identifier: &str) -> Option<usize> {
    let joined = sanitize_path(base, identifier);
    if joined.is_file()
        && joined
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        && let Some(length) = context_length_from_gguf(&joined)
    {
        return Some(length);
    }

    if joined.is_dir()
        && let Some(length) = context_length_from_hf_dir(&joined)
    {
        return Some(length);
    }

    None
}

fn context_length_from_hf_dir(dir: &Path) -> Option<usize> {
    let config_path = dir.join("config.json");
    if !config_path.is_file() {
        return None;
    }

    let file = File::open(config_path).ok()?;
    let json: Value = serde_json::from_reader(file).ok()?;
    context_length_from_config_json(&json)
}

fn context_length_from_configs(candidates: &[String]) -> Option<usize> {
    for name in candidates {
        if let Ok(cfg) = ModelConfig::from_model_name(name)
            && let Some(length) = cfg
                .metadata_overrides
                .as_ref()
                .and_then(|m| m.context_lengths.iter().copied().max())
        {
            return usize::try_from(length).ok();
        }
    }
    None
}

pub(crate) fn candidate_model_names(identifier: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    push_candidate(&mut ordered, &mut seen, identifier.to_string());

    if let Some(stem) = Path::new(identifier).file_stem().and_then(|s| s.to_str()) {
        push_candidate(&mut ordered, &mut seen, stem.to_string());
    }

    if let Some(last) = identifier.split('/').next_back()
        && last != identifier
    {
        push_candidate(&mut ordered, &mut seen, last.to_string());
    }

    let base_candidates = ordered.clone();
    for base in base_candidates {
        push_candidate(&mut ordered, &mut seen, base.to_lowercase());
        push_candidate(&mut ordered, &mut seen, base.replace('_', "-"));
        push_candidate(
            &mut ordered,
            &mut seen,
            base.to_lowercase().replace('_', "-"),
        );
    }

    let quant_candidates = ordered.clone();
    for base in quant_candidates {
        let lowered = base.to_lowercase();
        let trimmed: String = lowered
            .split('-')
            .take_while(|segment| !segment.starts_with('q') || segment.len() > 2)
            .collect::<Vec<_>>()
            .join("-");
        if !trimmed.is_empty() {
            push_candidate(&mut ordered, &mut seen, trimmed);
        }
    }

    ordered
}

fn push_candidate(ordered: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    let normalized = trimmed.to_string();
    if seen.insert(normalized.clone()) {
        ordered.push(normalized);
    }
}

pub(crate) fn context_length_from_config_json(value: &Value) -> Option<usize> {
    const KEYS: [&str; 9] = [
        "max_position_embeddings",
        "max_seq_len",
        "max_seq_length",
        "max_sequence_length",
        "context_window",
        "context_length",
        "n_ctx",
        "n_positions",
        "model_max_length",
    ];

    for key in KEYS {
        if let Some(length) = value.get(key).and_then(value_as_usize) {
            return Some(length);
        }
    }

    if let Some(rope_scaling) = value.get("rope_scaling") {
        if let Some(length) = rope_scaling
            .get("max_position_embeddings")
            .and_then(value_as_usize)
        {
            return Some(length);
        }

        if let (Some(factor), Some(base)) = (
            rope_scaling.get("factor").and_then(|v| v.as_f64()),
            value
                .get("max_position_embeddings")
                .and_then(value_as_usize),
        ) {
            let scaled = (base as f64 * factor).round();
            if scaled > 0.0 {
                return Some(scaled as usize);
            }
        }
    }

    None
}

pub(crate) fn value_as_usize(value: &Value) -> Option<usize> {
    if let Some(num) = value.as_u64() {
        return usize::try_from(num).ok();
    }
    if let Some(num) = value.as_i64()
        && num >= 0
    {
        return usize::try_from(num as u64).ok();
    }
    if let Some(num) = value.as_f64()
        && num.is_finite()
        && num > 0.0
    {
        return usize::try_from(num.round() as u64).ok();
    }
    None
}

fn env_context_length() -> Option<usize> {
    std::env::var("NITE_DEFAULT_CONTEXT_TOKENS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

fn sanitize_path(base: &Path, identifier: &str) -> PathBuf {
    let buf = PathBuf::from(identifier);
    if buf.is_relative() {
        base.join(buf)
    } else {
        buf
    }
}
