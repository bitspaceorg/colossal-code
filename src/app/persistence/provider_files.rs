use color_eyre::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::app::persistence::config::config_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProviderFile {
    pub(crate) name: String,
    pub(crate) url_base: String,
}

pub(crate) fn provider_id_from_name(name: &str) -> String {
    let mut id = String::new();
    let mut last_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !id.is_empty() {
            id.push('-');
            last_dash = true;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() {
        "custom-provider".to_string()
    } else {
        id
    }
}

pub(crate) fn providers_dir() -> Result<PathBuf> {
    let dir = config_dir()?.join("providers");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(crate) fn save_provider_file(provider: &ProviderFile) -> Result<PathBuf> {
    let filename = format!("{}.json", provider_id_from_name(&provider.name));
    let path = providers_dir()?.join(filename);
    let content = serde_json::to_string_pretty(provider)?;
    fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::provider_id_from_name;

    #[test]
    fn provider_id_from_name_is_filename_safe() {
        assert_eq!(provider_id_from_name("OpenRouter"), "openrouter");
        assert_eq!(provider_id_from_name("My Provider API"), "my-provider-api");
        assert_eq!(provider_id_from_name(" !!! "), "custom-provider");
    }
}
