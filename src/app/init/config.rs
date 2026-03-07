use color_eyre::Result;

use crate::{
    AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY, AUTO_SUMMARIZE_THRESHOLD_VERSION,
    AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY, App, DEFAULT_AUTO_SUMMARIZE_THRESHOLD,
    LEGACY_AUTO_SUMMARIZE_THRESHOLD, MAX_AUTO_SUMMARIZE_THRESHOLD, MIN_AUTO_SUMMARIZE_THRESHOLD,
    ModelInfo, model_context,
};

fn parse_auto_summarize_threshold(raw: &str) -> Option<f32> {
    let sanitized = raw.trim().trim_end_matches('%').trim();
    sanitized.parse::<f32>().ok()
}

fn resolve_auto_summarize_threshold(stored_value: Option<f32>, stored_version: Option<u32>) -> f32 {
    match (stored_value, stored_version) {
        (Some(value), Some(version)) => {
            if version < AUTO_SUMMARIZE_THRESHOLD_VERSION
                && (value - LEGACY_AUTO_SUMMARIZE_THRESHOLD).abs() < f32::EPSILON
            {
                DEFAULT_AUTO_SUMMARIZE_THRESHOLD
            } else {
                value
            }
        }
        (Some(value), None) => {
            if (value - LEGACY_AUTO_SUMMARIZE_THRESHOLD).abs() < f32::EPSILON {
                DEFAULT_AUTO_SUMMARIZE_THRESHOLD
            } else {
                value
            }
        }
        (None, _) => DEFAULT_AUTO_SUMMARIZE_THRESHOLD,
    }
}

impl App {
    pub(crate) fn models_directory_path() -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".config").join(".nite").join("models"))
    }

    pub(crate) fn load_config_value(key: &str) -> Option<String> {
        crate::app::persistence::config::load_config_value(key)
    }

    pub(crate) fn load_vim_mode_setting() -> bool {
        Self::load_config_value("vim-keybind")
            .map(|v| v == "true")
            .unwrap_or(false)
    }

    pub(crate) fn load_model_setting() -> Option<String> {
        Self::load_config_value("model")
    }

    pub(crate) fn clamp_auto_summarize_threshold(value: f32) -> f32 {
        value.clamp(MIN_AUTO_SUMMARIZE_THRESHOLD, MAX_AUTO_SUMMARIZE_THRESHOLD)
    }

    pub(crate) fn load_auto_summarize_threshold_setting() -> f32 {
        let stored_value = Self::load_config_value(AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY)
            .as_deref()
            .and_then(parse_auto_summarize_threshold)
            .map(Self::clamp_auto_summarize_threshold);

        let stored_version = Self::load_config_value(AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY)
            .and_then(|raw| raw.trim().parse::<u32>().ok());

        resolve_auto_summarize_threshold(stored_value, stored_version)
    }

    pub(crate) fn detect_context_tokens(model: Option<&str>) -> Option<usize> {
        if model.is_none() {
            return None;
        }
        let models_dir = Self::models_directory_path();
        let dir_ref = models_dir.as_deref();
        model.and_then(|name| model_context::detect_context_length(dir_ref, name))
    }

    pub(crate) fn refresh_context_window(&mut self) {
        self.current_context_tokens = Self::detect_context_tokens(self.current_model.as_deref());
    }

    pub(crate) fn save_config(&self) -> Result<()> {
        let mut config_map = crate::app::persistence::config::load_config_map();

        config_map.insert("vim-keybind".to_string(), self.vim_mode_enabled.to_string());
        if let Some(ref model) = self.current_model {
            config_map.insert("model".to_string(), model.clone());
        }
        config_map.insert(
            AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY.to_string(),
            format!("{:.1}", self.auto_summarize_threshold),
        );
        config_map.insert(
            AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY.to_string(),
            AUTO_SUMMARIZE_THRESHOLD_VERSION.to_string(),
        );

        crate::app::persistence::config::write_config_map(&config_map)?;
        Ok(())
    }

    pub(crate) fn save_vim_mode_setting(&self) -> Result<()> {
        self.save_config()
    }

    pub(crate) fn load_models(&mut self) -> Result<()> {
        let Some(models_dir) = Self::models_directory_path() else {
            self.available_models.clear();
            return Ok(());
        };

        if !models_dir.exists() {
            self.available_models.clear();
            return Ok(());
        }

        let mut models = Vec::new();
        for entry in std::fs::read_dir(&models_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("gguf") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let metadata = std::fs::metadata(&path)?;
                    let size_bytes = metadata.len();
                    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);

                    let quantization = model_context::extract_quantization(file_name);
                    let architecture = model_context::extract_architecture(file_name);
                    let parameter_count = model_context::extract_parameter_count(file_name);
                    let author = model_context::extract_author(file_name);
                    let version = model_context::extract_version(file_name);
                    let context_length = model_context::context_length_from_gguf(&path);
                    let file_hash = model_context::compute_file_hash(&path);

                    let display_name = file_name
                        .strip_suffix(".gguf")
                        .unwrap_or(file_name)
                        .to_string();

                    models.push(ModelInfo {
                        filename: file_name.to_string(),
                        display_name,
                        size_mb,
                        quantization,
                        architecture,
                        parameter_count,
                        file_hash,
                        author,
                        version,
                        context_length,
                    });
                }
            }
        }

        models.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        self.available_models = models;
        self.model_selected_index = 0;

        Ok(())
    }

    pub(crate) fn initialize_config_file() -> Result<()> {
        crate::app::persistence::config::initialize_default_file("vim-keybind = false\n")
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_auto_summarize_threshold;
    use crate::{
        AUTO_SUMMARIZE_THRESHOLD_VERSION, DEFAULT_AUTO_SUMMARIZE_THRESHOLD,
        LEGACY_AUTO_SUMMARIZE_THRESHOLD,
    };

    #[test]
    fn legacy_threshold_migrates_to_new_default() {
        let resolved =
            resolve_auto_summarize_threshold(Some(LEGACY_AUTO_SUMMARIZE_THRESHOLD), None);
        assert_eq!(resolved, DEFAULT_AUTO_SUMMARIZE_THRESHOLD);

        let resolved_with_old_version = resolve_auto_summarize_threshold(
            Some(LEGACY_AUTO_SUMMARIZE_THRESHOLD),
            Some(AUTO_SUMMARIZE_THRESHOLD_VERSION - 1),
        );
        assert_eq!(resolved_with_old_version, DEFAULT_AUTO_SUMMARIZE_THRESHOLD);
    }

    #[test]
    fn explicit_non_legacy_threshold_is_preserved() {
        let resolved = resolve_auto_summarize_threshold(Some(72.5), None);
        assert_eq!(resolved, 72.5);

        let resolved_with_current_version =
            resolve_auto_summarize_threshold(Some(72.5), Some(AUTO_SUMMARIZE_THRESHOLD_VERSION));
        assert_eq!(resolved_with_current_version, 72.5);
    }
}
