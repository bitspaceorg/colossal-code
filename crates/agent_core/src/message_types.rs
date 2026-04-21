use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ExecCommandResult {
    pub command: String,
    pub status: String,
    pub cmd_out: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ExecCommandResult {
    pub fn success(command: String, cmd_out: String) -> Self {
        Self {
            command,
            status: "Success".to_string(),
            cmd_out,
            message: None,
        }
    }

    pub fn failure(command: String, cmd_out: String, message: String) -> Self {
        Self {
            command,
            status: "Failure".to_string(),
            cmd_out,
            message: Some(message),
        }
    }
}

#[derive(Debug, Clone)]
pub enum BackendConfig {
    None,
    Local {
        model_path: String,
        model_files: Vec<String>,
    },
    Http {
        base_url: String,
        api_key: String,
        model: String,
        completions_path: String,
        requires_model_load: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    None,
    Local,
    Http,
    ExternalHttp,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerationStats {
    pub avg_completion_tok_per_sec: f32,
    pub completion_tokens: usize,
    pub prompt_tokens: usize,
    pub time_to_first_token_sec: f32,
    pub stop_reason: String,
}

#[derive(Debug, Clone)]
pub enum AgentMessage {
    UserInput(String),
    AgentResponse(String, usize),
    ThinkingContent(String, usize),
    ThinkingSummary(String),
    ThinkingComplete(usize),
    ToolCallStarted(String, String),
    ToolCallCompleted(String, String),
    Error(String),
    Cancel,
    ClearContext,
    InjectContext(String),
    ContextCleared,
    ContextInjected,
    BackgroundTaskStarted(String, String, String),
    Done,
    ModelLoaded,
    GenerationStats(GenerationStats),
    ReloadModel(String),
    RequestApproval(String),
    ApprovalResponse(bool),
}
