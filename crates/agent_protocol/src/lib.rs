//! # Agent Protocol (A2A)
//!
//! Implementation of the Agent-to-Agent (A2A) protocol for Rust.
//! This crate provides types and utilities for agent interoperability,
//! supporting both local and external API backends.
//!
//! ## Features
//!
//! - **Agent Card**: Self-describing manifests for capability discovery
//! - **Task Management**: Full task lifecycle with state transitions
//! - **Messages**: Structured communication with multi-part content
//! - **JSON-RPC 2.0**: Standard protocol binding over HTTP(S)
//! - **SSE Streaming**: Real-time updates via Server-Sent Events
//! - **Push Notifications**: Webhook-based async updates
//!
//! ## Example
//!
//! ```rust,ignore
//! use agent_protocol::{AgentCard, A2AServer, A2AClient};
//!
//! // Create an agent card describing this agent's capabilities
//! let card = AgentCard::builder()
//!     .name("my-agent")
//!     .description("A helpful assistant")
//!     .skill("chat", "General conversation")
//!     .build();
//!
//! // Start a server to receive requests from other agents
//! let server = A2AServer::new(card, handler).bind("0.0.0.0:8080");
//!
//! // Or connect to another agent as a client
//! let client = A2AClient::from_card_url("https://other-agent/.well-known/agent.json").await?;
//! let task = client.send_message("Hello!").await?;
//! ```

pub mod types;
pub mod jsonrpc;
pub mod server;
pub mod client;
pub mod error;

// Re-export main types
pub use types::agent_card::{
    AgentCard, AgentCardBuilder, AgentCardSignature, AgentExtension, Skill, SecurityScheme,
};
pub use types::task::{Task, TaskError, TaskMetadata, TaskState, TaskStatus};
pub use types::message::{Message, MessagePart, Role};
pub use types::artifact::Artifact;
pub use types::spec::{
    FeedbackEntry, SpecSheet, SpecStep, SpecStepRef, SpecValidationError, StepStatus,
    TaskSummary, TaskVerification, TestResult, TestRun, VerificationStatus,
};
pub use jsonrpc::{JsonRpcRequest, JsonRpcResponse, A2AMethod};
pub use server::A2AServer;
pub use client::A2AClient;
pub use error::{A2AError, A2AResult};

/// Protocol version supported by this implementation
pub const PROTOCOL_VERSION: &str = "0.3.0";

/// Well-known path for agent card discovery
pub const AGENT_CARD_PATH: &str = "/.well-known/agent.json";
