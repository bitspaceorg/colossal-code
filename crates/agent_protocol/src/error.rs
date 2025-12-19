//! Error types for the A2A protocol

use thiserror::Error;

/// Result type alias for A2A operations
pub type A2AResult<T> = Result<T, A2AError>;

/// Errors that can occur during A2A protocol operations
#[derive(Error, Debug)]
pub enum A2AError {
    // JSON-RPC standard errors
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid params: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    // A2A specific errors (codes -32001 to -32099)
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Task not cancelable")]
    TaskNotCancelable,

    #[error("Push notification not supported")]
    PushNotificationNotSupported,

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Content type not supported: {0}")]
    ContentTypeNotSupported(String),

    #[error("Invalid agent card: {0}")]
    InvalidAgentCard(String),

    #[error("Version not supported: {0}")]
    VersionNotSupported(String),

    #[error("Authentication required")]
    AuthenticationRequired,

    #[error("Authorization failed: {0}")]
    AuthorizationFailed(String),

    // Network/transport errors
    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Stream error: {0}")]
    StreamError(String),

    // Serialization errors
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    // HTTP errors
    #[error("HTTP error: {0}")]
    HttpError(String),

    #[error("Request error: {0}")]
    RequestError(#[from] reqwest::Error),
}

impl A2AError {
    /// Get the JSON-RPC error code for this error
    pub fn code(&self) -> i32 {
        match self {
            A2AError::ParseError(_) => -32700,
            A2AError::InvalidRequest(_) => -32600,
            A2AError::MethodNotFound(_) => -32601,
            A2AError::InvalidParams(_) => -32602,
            A2AError::InternalError(_) => -32603,
            A2AError::TaskNotFound(_) => -32001,
            A2AError::TaskNotCancelable => -32002,
            A2AError::PushNotificationNotSupported => -32003,
            A2AError::UnsupportedOperation(_) => -32004,
            A2AError::ContentTypeNotSupported(_) => -32005,
            A2AError::InvalidAgentCard(_) => -32006,
            A2AError::AuthenticationRequired => -32007,
            A2AError::AuthorizationFailed(_) => -32008,
            A2AError::VersionNotSupported(_) => -32009,
            _ => -32000, // Generic server error
        }
    }

    /// Get the HTTP status code for this error
    pub fn http_status(&self) -> u16 {
        match self {
            A2AError::ParseError(_) => 400,
            A2AError::InvalidRequest(_) => 400,
            A2AError::MethodNotFound(_) => 404,
            A2AError::InvalidParams(_) => 400,
            A2AError::InternalError(_) => 500,
            A2AError::TaskNotFound(_) => 404,
            A2AError::TaskNotCancelable => 400,
            A2AError::PushNotificationNotSupported => 400,
            A2AError::UnsupportedOperation(_) => 400,
            A2AError::ContentTypeNotSupported(_) => 415,
            A2AError::InvalidAgentCard(_) => 400,
            A2AError::AuthenticationRequired => 401,
            A2AError::AuthorizationFailed(_) => 403,
            A2AError::VersionNotSupported(_) => 400,
            A2AError::ConnectionError(_) => 503,
            A2AError::Timeout(_) => 504,
            A2AError::StreamError(_) => 500,
            _ => 500,
        }
    }
}
