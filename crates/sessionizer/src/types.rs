use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecCommandParams {
    pub command: Vec<String>,
    pub shell: crate::shell::Shell,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    pub max_output_tokens: u32,
    pub sandbox_policy: crate::protocol::SandboxPolicy,
    pub is_background: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteStdinParams {
    pub session_id: SessionId,
    pub chars: String,
    pub yield_time_ms: u64,
    pub max_output_tokens: u32,
    pub sandbox_policy: crate::protocol::SandboxPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExitStatus {
    Completed { code: i32 },
    Ongoing(SessionId),
    Timeout,
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Create a new SessionId from a string
    pub fn new(id: String) -> Self {
        SessionId(id)
    }
    
    /// Get the string representation of the SessionId
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub struct ExecCommandOutput {
    pub duration: Duration,
    pub exit_status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub aggregated_output: String,
    pub log_file: Option<PathBuf>,
}

impl ExecCommandOutput {
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "duration_ms": self.duration.as_millis(),
            "exit_status": match &self.exit_status {
                ExitStatus::Completed { code } => serde_json::json!({ "Completed": { "code": code } }),
                ExitStatus::Ongoing(session_id) => serde_json::json!({ "Ongoing": { "session_id": session_id.as_str() } }),
                ExitStatus::Timeout => serde_json::json!("Timeout"),
                ExitStatus::Killed => serde_json::json!("Killed"),
            },
            "stdout": self.stdout,
            "stderr": self.stderr,
            "aggregated_output": self.aggregated_output,
            "log_file": self.log_file.as_ref().map(|p| p.to_string_lossy().to_string()),
        })
    }

    pub fn to_structured_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "duration_ms": self.duration.as_millis(),
            "exit_status": match &self.exit_status {
                ExitStatus::Completed { code } => serde_json::json!({ "type": "Completed", "code": code }),
                ExitStatus::Ongoing(id) => serde_json::json!({ "type": "Ongoing", "session_id": id.as_str() }),
                ExitStatus::Timeout => serde_json::json!({ "type": "Timeout" }),
                ExitStatus::Killed => serde_json::json!({ "type": "Killed" }),
            },
            "stdout": self.stdout,
            "stderr": self.stderr,
            "aggregated_output": self.aggregated_output,
            "log_file": self.log_file.as_ref().map(|p| p.to_string_lossy().to_string()),
        })
    }
}

#[derive(Debug)]
pub struct StreamOutput<T> {
    pub text: T,
    pub truncated_after_lines: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    Stdout(String),
    Stderr(String),
    Exit(i32),
    Error(String),
}

impl StreamEvent {
    /// Create a StreamEvent from raw bytes, attempting to determine if it's stdout or stderr
    /// In a PTY environment, stdout and stderr are typically merged, but we can still
    /// provide this structure for API consistency
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match String::from_utf8(bytes.to_vec()) {
            Ok(s) => StreamEvent::Stdout(s),
            Err(e) => StreamEvent::Error(format!("Failed to decode UTF-8: {}", e)),
        }
    }
}