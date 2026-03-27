use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpTransportKind {
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolChoiceFormat {
    OpenAi,
    Responses,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MaxTokensStyle {
    DefaultChatCompletions,
    ResponsesOptional,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HttpBackendProfile {
    transport: HttpTransportKind,
    tool_choice_format: ToolChoiceFormat,
    max_tokens_style: MaxTokensStyle,
    supports_store_flag: bool,
    requires_instructions: bool,
    supports_chatgpt_account_id: bool,
    google_api: bool,
}

impl HttpBackendProfile {
    pub(super) fn detect(base_url: &str, completions_path: &str) -> Self {
        let path = completions_path.to_ascii_lowercase();
        let is_google_api = base_url.contains("generativelanguage.googleapis.com")
            && !path.contains("chat/completions");
        let is_responses_api =
            path.contains("/responses") || path.contains("backend-api/codex/responses");
        let is_chatgpt_codex = path.contains("backend-api/codex/responses");

        if is_responses_api {
            return Self {
                transport: HttpTransportKind::Responses,
                tool_choice_format: ToolChoiceFormat::Responses,
                max_tokens_style: MaxTokensStyle::ResponsesOptional,
                supports_store_flag: true,
                requires_instructions: true,
                supports_chatgpt_account_id: is_chatgpt_codex,
                google_api: false,
            };
        }

        Self {
            transport: HttpTransportKind::ChatCompletions,
            tool_choice_format: if is_google_api {
                ToolChoiceFormat::Unsupported
            } else {
                ToolChoiceFormat::OpenAi
            },
            max_tokens_style: if is_google_api {
                MaxTokensStyle::Unsupported
            } else {
                MaxTokensStyle::DefaultChatCompletions
            },
            supports_store_flag: !is_google_api,
            requires_instructions: false,
            supports_chatgpt_account_id: false,
            google_api: is_google_api,
        }
    }

    pub(super) fn transport(self) -> HttpTransportKind {
        self.transport
    }

    pub(super) fn is_google_api(self) -> bool {
        self.google_api
    }

    pub(super) fn requires_instructions(self) -> bool {
        self.requires_instructions
    }

    pub(super) fn supports_chatgpt_account_id(self) -> bool {
        self.supports_chatgpt_account_id
    }

    pub(super) fn maybe_store_flag(self) -> Option<Value> {
        if self.supports_store_flag {
            Some(json!(false))
        } else {
            None
        }
    }

    pub(super) fn apply_max_tokens(self, payload: &mut Value, requested: Option<usize>) {
        match self.max_tokens_style {
            MaxTokensStyle::DefaultChatCompletions => {
                if let Some(max_len) = requested.or(Some(32_000)) {
                    payload["max_tokens"] = json!(max_len);
                }
            }
            MaxTokensStyle::ResponsesOptional => {
                if let Some(max_len) = requested {
                    payload["max_output_tokens"] = json!(max_len);
                }
            }
            MaxTokensStyle::Unsupported => {}
        }
    }

    pub(super) fn tool_choice_format(self) -> ToolChoiceFormat {
        self.tool_choice_format
    }
}
