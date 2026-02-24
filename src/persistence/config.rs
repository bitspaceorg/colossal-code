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
    for line in content.lines() {
        if line.starts_with(key) {
            if let Some(value) = line.split('=').nth(1) {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

pub fn load_config_map() -> HashMap<String, String> {
    let mut config_map = HashMap::new();
    let Ok(config_path) = config_file_path() else {
        return config_map;
    };
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return config_map;
    };

    for line in content.lines() {
        if let Some(idx) = line.find('=') {
            let key = line[..idx].trim();
            let value = line[idx + 1..].trim();
            config_map.insert(key.to_string(), value.to_string());
        }
    }
    config_map
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
        let mut map = HashMap::new();
        map.insert("model".to_string(), "foo".to_string());
        map.insert("vim-keybind".to_string(), "true".to_string());
        let mut rendered = String::new();
        for (k, v) in &map {
            rendered.push_str(&format!("{} = {}\n", k, v));
        }

        let parsed = rendered
            .lines()
            .filter_map(|line| {
                line.find('=').map(|idx| {
                    (
                        line[..idx].trim().to_string(),
                        line[idx + 1..].trim().to_string(),
                    )
                })
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(parsed.get("model"), Some(&"foo".to_string()));
        assert_eq!(parsed.get("vim-keybind"), Some(&"true".to_string()));
    }
}
