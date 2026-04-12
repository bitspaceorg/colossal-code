use crate::{BackendConfig, model_config, safety_config};
use anyhow::Result;
use colossal_linux_sandbox::protocol::SandboxPolicy;
use std::ffi::OsStr;
use std::path::PathBuf;

pub fn resolve_workspace_root() -> PathBuf {
    crate::agent_state::resolve_workspace_root()
}

pub fn resolve_tools_binary_path_for_runtime() -> Result<PathBuf> {
    crate::agent_state::resolve_tools_binary_path_for_runtime()
}

pub fn sandbox_policy_from_config(config: &safety_config::SafetyConfig) -> SandboxPolicy {
    crate::agent_state::sandbox_policy_from_config(config)
}

pub fn sandbox_policy_from_config_with_workspace(
    config: &safety_config::SafetyConfig,
    workspace: PathBuf,
) -> SandboxPolicy {
    crate::agent_state::sandbox_policy_from_config_with_workspace(config, workspace)
}

pub fn prompt_context() -> (String, String) {
    let os_info = std::env::consts::OS;
    let os_version = if os_info == "linux" {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("PRETTY_NAME="))
                    .map(|line| {
                        line.trim_start_matches("PRETTY_NAME=")
                            .trim_matches('"')
                            .to_string()
                    })
            })
            .unwrap_or_else(|| "Linux".to_string())
    } else {
        os_info.to_string()
    };
    let workspace_path = resolve_workspace_root().display().to_string();
    (os_version, workspace_path)
}

pub fn render_system_prompt(
    template: &str,
    os_version: &str,
    workspace_path: &str,
    model_label: &str,
    safety_mode: Option<safety_config::SafetyMode>,
) -> String {
    let mut result = template
        .replace("{os_version}", os_version)
        .replace("{workspace_path}", workspace_path)
        .replace("{model_name}", model_label);

    if let Some(mode) = safety_mode {
        if mode == safety_config::SafetyMode::ReadOnly {
            result = filter_readonly_sections(&result);
        }
    }

    result
}

fn filter_readonly_sections(template: &str) -> String {
    let mut result = template.to_string();

    if let Some(start) = result.find("<making_code_changes>") {
        if let Some(end) = result.find("</making_code_changes>") {
            let end = result[end..]
                .find('\n')
                .map(|i| end + i + 1)
                .unwrap_or(end + "</making_code_changes>".len());
            result.drain(start..end);
        }
    }

    result.trim().to_string()
}

fn label_from_filename(model_filename: &str) -> String {
    std::path::Path::new(model_filename)
        .file_stem()
        .and_then(OsStr::to_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| model_filename.to_string())
}

pub fn model_label_from_backend(backend_config: &BackendConfig) -> String {
    match backend_config {
        BackendConfig::None => String::new(),
        BackendConfig::Local {
            model_path,
            model_files,
        } => model_files
            .first()
            .map(|filename| label_from_filename(filename))
            .or_else(|| {
                std::path::Path::new(model_path)
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "local model".to_string()),
        BackendConfig::Http { model, .. } => model.clone(),
    }
}

pub async fn regenerate_system_prompt(
    system_prompt: &std::sync::Arc<tokio::sync::Mutex<String>>,
    suffix: Option<String>,
) -> Result<()> {
    let (os_version, workspace_path) = prompt_context();
    let system_prompt_template =
        crate::read_system_prompt().unwrap_or_else(|_e| crate::get_default_niterules());
    let model_label = {
        // Placeholder - actual model label should be passed in
        "model".to_string()
    };
    let safety_mode: Option<safety_config::SafetyMode> = None;
    let mut prompt = render_system_prompt(
        &system_prompt_template,
        &os_version,
        &workspace_path,
        &model_label,
        safety_mode,
    );
    if let Some(s) = suffix {
        prompt.push_str(&s);
    }
    let mut system_prompt_guard = system_prompt.lock().await;
    *system_prompt_guard = prompt;
    Ok(())
}

pub fn load_thinking_tags(_model_path: &str, model_filename: &str) -> model_config::ThinkingTags {
    let filename_stem = std::path::Path::new(model_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(model_filename);

    let name_variants = vec![
        filename_stem.to_lowercase(),
        filename_stem.to_string(),
        filename_stem
            .to_lowercase()
            .split('-')
            .take_while(|s| !s.starts_with('q') || s.len() > 2)
            .collect::<Vec<_>>()
            .join("-"),
    ];

    for variant in &name_variants {
        let (has_thinking, tags) =
            model_config::ModelConfig::load_or_detect(variant, model_filename);
        if has_thinking {
            return tags;
        }
    }

    let (_, tags) = model_config::ModelConfig::load_or_detect("", model_filename);
    tags
}
