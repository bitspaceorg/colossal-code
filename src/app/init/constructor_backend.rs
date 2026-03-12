use crate::app::App;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackendConfig {
    pub(super) mode: String,
    pub(super) base_url: Option<String>,
    pub(super) api_key: String,
    pub(super) completions_path: Option<String>,
    pub(super) google_user_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackendEnvironment {
    pub(super) backend_mode: &'static str,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) completions_path: Option<String>,
    pub(super) google_user_project: Option<String>,
    pub(super) limit_thinking_to_first_token: bool,
}

impl BackendConfig {
    pub(super) fn read() -> Self {
        Self {
            mode: Self::read_non_empty("backend").unwrap_or_else(|| "http".to_string()),
            base_url: Self::read_non_empty("http-base-url"),
            api_key: App::load_config_value("http-api-key")
                .map(|v| v.trim().to_string())
                .unwrap_or_default(),
            completions_path: Self::read_non_empty("http-completions-path"),
            google_user_project: Self::read_non_empty("google-user-project"),
        }
    }

    fn read_non_empty(key: &str) -> Option<String> {
        App::load_config_value(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub(super) fn into_environment(self) -> BackendEnvironment {
        let mode = self.mode.to_lowercase();

        match mode.as_str() {
            "local" => BackendEnvironment {
                backend_mode: "local",
                base_url: None,
                api_key: None,
                completions_path: None,
                google_user_project: self.google_user_project,
                limit_thinking_to_first_token: false,
            },
            "external" => BackendEnvironment {
                backend_mode: "external",
                base_url: Some(
                    self.base_url
                        .unwrap_or_else(|| "https://api.openai.com".to_string()),
                ),
                api_key: if self.api_key.is_empty() {
                    None
                } else {
                    Some(self.api_key)
                },
                completions_path: Some(
                    self.completions_path
                        .unwrap_or_else(|| "chat/completions".to_string()),
                ),
                google_user_project: self.google_user_project,
                limit_thinking_to_first_token: true,
            },
            _ => BackendEnvironment {
                backend_mode: "http",
                base_url: Some(
                    self.base_url
                        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string()),
                ),
                api_key: if self.api_key.is_empty() {
                    None
                } else {
                    Some(self.api_key)
                },
                completions_path: Some(
                    self.completions_path
                        .unwrap_or_else(|| "/v1/chat/completions".to_string()),
                ),
                google_user_project: self.google_user_project,
                limit_thinking_to_first_token: false,
            },
        }
    }
}

impl App {
    pub(super) fn apply_backend_environment(env: &BackendEnvironment) {
        unsafe {
            std::env::set_var("NITE_BACKEND_MODE", env.backend_mode);
        }

        if let Some(value) = env.base_url.as_deref() {
            unsafe {
                std::env::set_var("NITE_HTTP_BASE_URL", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_BASE_URL");
            }
        }

        if let Some(value) = env.api_key.as_deref() {
            unsafe {
                std::env::set_var("NITE_HTTP_API_KEY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_API_KEY");
            }
        }

        if let Some(value) = env.completions_path.as_deref() {
            unsafe {
                std::env::set_var("NITE_HTTP_COMPLETIONS_PATH", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_COMPLETIONS_PATH");
            }
        }

        if let Some(value) = env.google_user_project.as_deref() {
            unsafe {
                std::env::set_var("NITE_GOOGLE_USER_PROJECT", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_GOOGLE_USER_PROJECT");
            }
        }
    }
}
