use color_eyre::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn config_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
    Ok(Path::new(&home).join(".config").join(".nite"))
}

pub fn config_file_path() -> Result<PathBuf> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("nite.conf"))
}

pub fn initialize_default_file(default_content: &str) -> Result<()> {
    let config_path = config_file_path()?;
    if !config_path.exists() {
        std::fs::write(config_path, default_content)?;
    }
    Ok(())
}

pub fn load_config_value(key: &str) -> Option<String> {
    let config_path = config_file_path().ok()?;
    let content = std::fs::read_to_string(config_path).ok()?;
    load_config_value_from_content(&content, key)
}

pub fn load_config_map() -> HashMap<String, String> {
    let Ok(config_path) = config_file_path() else {
        return HashMap::new();
    };
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return HashMap::new();
    };

    parse_config_map_from_content(&content)
}

fn load_config_value_from_content(content: &str, key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }

    content.lines().find_map(|line| {
        let (parsed_key, parsed_value) = parse_config_line(line)?;
        (parsed_key == key).then(|| parsed_value.to_string())
    })
}

fn parse_config_map_from_content(content: &str) -> HashMap<String, String> {
    let mut config_map = HashMap::new();
    for line in content.lines() {
        if let Some((key, value)) = parse_config_line(line) {
            config_map.insert(key.to_string(), value.to_string());
        }
    }
    config_map
}

fn parse_config_line(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let (raw_key, raw_value) = trimmed.split_once('=')?;
    let key = raw_key.trim();
    if key.is_empty() {
        return None;
    }

    Some((key, raw_value.trim()))
}

pub fn write_config_map(config_map: &HashMap<String, String>) -> Result<()> {
    let config_path = config_file_path()?;
    let mut content = String::new();
    for (key, value) in config_map {
        content.push_str(&format!("{} = {}\n", key, value));
    }
    std::fs::write(config_path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_map_parses_key_values() {
        let rendered = "model = foo\nvim-keybind = true\n";
        let parsed = parse_config_map_from_content(rendered);

        assert_eq!(parsed.get("model"), Some(&"foo".to_string()));
        assert_eq!(parsed.get("vim-keybind"), Some(&"true".to_string()));
    }

    #[test]
    fn load_config_value_uses_exact_key_matching() {
        let content = "model-extra = should-not-match\nmodel = expected\n";

        let value = load_config_value_from_content(content, "model");

        assert_eq!(value.as_deref(), Some("expected"));
    }

    #[test]
    fn parse_config_map_ignores_comments_and_malformed_lines() {
        let content = "\n# full-line comment\n=missing-key\nno-delimiter\nvim-keybind = true\n";

        let parsed = parse_config_map_from_content(content);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get("vim-keybind"), Some(&"true".to_string()));
    }
}
