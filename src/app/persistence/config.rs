use color_eyre::Result;
use std::collections::{HashMap, HashSet};
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

pub(crate) fn load_config_value_from_content(content: &str, key: &str) -> Option<String> {
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
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    let content = render_config_with_updates(&existing, config_map);
    std::fs::write(config_path, content)?;
    Ok(())
}

fn render_config_with_updates(existing: &str, updates: &HashMap<String, String>) -> String {
    let mut seen: HashSet<String> = HashSet::new();
    let mut lines = Vec::new();

    for line in existing.lines() {
        if let Some((key, _)) = parse_config_line(line) {
            if let Some(value) = updates.get(key) {
                if seen.insert(key.to_string()) {
                    lines.push(format!("{} = {}", key, value));
                }
            } else {
                lines.push(line.to_string());
            }
        } else {
            lines.push(line.to_string());
        }
    }

    let mut missing_keys: Vec<_> = updates.keys().filter(|k| !seen.contains(*k)).collect();
    missing_keys.sort();
    for key in missing_keys {
        if let Some(value) = updates.get(key) {
            lines.push(format!("{} = {}", key, value));
        }
    }

    if lines.is_empty() {
        String::new()
    } else {
        let mut rendered = lines.join("\n");
        rendered.push('\n');
        rendered
    }
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

    #[test]
    fn load_config_value_trims_requested_key() {
        let content = " model = expected\n";

        let value = load_config_value_from_content(content, "  model  ");

        assert_eq!(value.as_deref(), Some("expected"));
    }

    #[test]
    fn parse_config_map_uses_last_value_and_preserves_equals_in_values() {
        let content = "api-url = https://example.test/query?a=b\nmodel = first\nmodel = second\n";

        let parsed = parse_config_map_from_content(content);

        assert_eq!(
            parsed.get("api-url"),
            Some(&"https://example.test/query?a=b".to_string())
        );
        assert_eq!(parsed.get("model"), Some(&"second".to_string()));
    }

    #[test]
    fn render_config_with_updates_preserves_comments_and_blank_lines() {
        let existing = "# keep header\nvim-keybind = false\n\n# model comment\nmodel = old\n";
        let mut updates = HashMap::new();
        updates.insert("vim-keybind".to_string(), "true".to_string());
        updates.insert("model".to_string(), "new-model".to_string());
        updates.insert("auto-summarize-threshold".to_string(), "85.0".to_string());

        let rendered = render_config_with_updates(existing, &updates);

        assert!(rendered.contains("# keep header"));
        assert!(rendered.contains("# model comment"));
        assert!(rendered.contains("\n\n# model comment\n"));
        assert!(rendered.contains("vim-keybind = true"));
        assert!(rendered.contains("model = new-model"));
        assert!(rendered.contains("auto-summarize-threshold = 85.0"));
    }

    #[test]
    fn render_config_with_updates_deduplicates_duplicate_keys() {
        let existing = "model = first\nmodel = second\n# comment\n";
        let mut updates = HashMap::new();
        updates.insert("model".to_string(), "final".to_string());

        let rendered = render_config_with_updates(existing, &updates);

        assert_eq!(rendered.matches("model = final").count(), 1);
        assert!(rendered.contains("# comment"));
    }
}
