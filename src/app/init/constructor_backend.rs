use crate::app::App;
use crate::app::persistence::auth_store::{StoredAuthKind, StoredConnection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendConfig {
    pub(super) mode: String,
    pub(super) base_url: Option<String>,
    pub(super) api_key: String,
    pub(super) completions_path: Option<String>,
    pub(super) google_user_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendEnvironment {
    pub(super) backend_mode: &'static str,
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) completions_path: Option<String>,
    pub(super) account_id: Option<String>,
    pub(super) refresh_token: Option<String>,
    pub(super) access_expires_at: Option<u64>,
    pub(super) google_user_project: Option<String>,
    pub(super) limit_thinking_to_first_token: bool,
}

impl BackendConfig {
    pub(crate) fn read() -> Self {
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

    pub(crate) fn into_environment(self) -> BackendEnvironment {
        let mode = self.mode.to_lowercase();

        match mode.as_str() {
            "local" => BackendEnvironment {
                backend_mode: "local",
                base_url: None,
                api_key: None,
                completions_path: None,
                account_id: None,
                refresh_token: None,
                access_expires_at: None,
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
                account_id: None,
                refresh_token: None,
                access_expires_at: None,
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
                account_id: None,
                refresh_token: None,
                access_expires_at: None,
                google_user_project: self.google_user_project,
                limit_thinking_to_first_token: false,
            },
        }
    }

    pub(crate) fn from_connection(connection: &StoredConnection) -> Option<BackendEnvironment> {
        match connection.auth_kind {
            StoredAuthKind::ApiKey => Some(BackendEnvironment {
                backend_mode: "external",
                base_url: connection.base_url.clone(),
                api_key: connection.api_key.clone(),
                completions_path: connection.completions_path.clone(),
                account_id: None,
                refresh_token: None,
                access_expires_at: None,
                google_user_project: None,
                limit_thinking_to_first_token: true,
            }),
            StoredAuthKind::OpenAiSubscription => Some(BackendEnvironment {
                backend_mode: "external",
                base_url: connection.base_url.clone(),
                api_key: connection.access_token.clone(),
                completions_path: connection.completions_path.clone(),
                account_id: connection.account_id.clone(),
                refresh_token: connection.refresh_token.clone(),
                access_expires_at: connection.access_expires_at,
                google_user_project: None,
                limit_thinking_to_first_token: true,
            }),
        }
    }
}

impl App {
    pub(crate) fn apply_backend_environment(env: &BackendEnvironment) {
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

        if let Some(value) = env.account_id.as_deref() {
            unsafe {
                std::env::set_var("NITE_HTTP_ACCOUNT_ID", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_ACCOUNT_ID");
            }
        }

        if let Some(value) = env.refresh_token.as_deref() {
            unsafe {
                std::env::set_var("NITE_HTTP_REFRESH_TOKEN", value);
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_REFRESH_TOKEN");
            }
        }

        if let Some(value) = env.access_expires_at {
            unsafe {
                std::env::set_var("NITE_HTTP_ACCESS_EXPIRES_AT", value.to_string());
            }
        } else {
            unsafe {
                std::env::remove_var("NITE_HTTP_ACCESS_EXPIRES_AT");
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
