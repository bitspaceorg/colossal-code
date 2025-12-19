//! Agent Card types for capability discovery
//!
//! The Agent Card is a self-describing manifest that allows agents to
//! discover each other's capabilities, authentication requirements,
//! and supported interaction modes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::PROTOCOL_VERSION;

/// Agent Card - A self-describing manifest for agent discovery
///
/// This is served at `/.well-known/agent.json` and describes
/// the agent's capabilities, skills, and how to interact with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Protocol version (e.g., "0.3.0")
    pub protocol_version: String,

    /// Unique identifier for this agent
    pub name: String,

    /// Human-readable description of what this agent does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// URL to the agent's icon/avatar
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,

    /// Link to documentation describing this agent
    #[serde(skip_serializing_if = "Option::is_none", rename = "documentationUrl")]
    pub documentation_url: Option<String>,

    /// Provider/organization information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,

    /// Supported protocol interfaces (HTTP, gRPC, etc.)
    pub supported_interfaces: Vec<SupportedInterface>,

    /// Preferred transport to reach this agent
    #[serde(skip_serializing_if = "Option::is_none", rename = "preferredTransport")]
    pub preferred_transport: Option<ProtocolBinding>,

    /// Agent capabilities
    pub capabilities: AgentCapabilities,

    /// Skills this agent can perform
    pub skills: Vec<Skill>,

    /// Authentication/authorization schemes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_schemes: Option<Vec<SecurityScheme>>,

    /// Default input content types accepted
    #[serde(default)]
    pub default_input_modes: Vec<String>,

    /// Default output content types produced
    #[serde(default)]
    pub default_output_modes: Vec<String>,

    /// JSON Web Signatures that verify this card
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures: Option<Vec<AgentCardSignature>>,

    /// Whether authenticated extended cards are supported
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "supportsAuthenticatedExtendedCard"
    )]
    pub supports_authenticated_extended_card: Option<bool>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Provider information for the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProvider {
    /// Organization name
    pub organization: String,

    /// Contact URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Optional extension metadata beyond the core specification
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExtension {
    /// Unique URI describing the extension
    pub uri: String,
    /// Human friendly description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the extension is required for interoperability
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// Arbitrary parameters used by the extension
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

/// JSON Web Signature metadata for Agent Cards
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCardSignature {
    /// Base64url-encoded protected JWS header
    pub protected: String,
    /// Base64url-encoded signature
    pub signature: String,
    /// Optional unprotected header values
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<HashMap<String, serde_json::Value>>,
}

/// Supported protocol interface
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedInterface {
    /// Protocol binding type
    pub protocol: ProtocolBinding,

    /// Base URL for this interface
    pub url: String,
}

/// Protocol binding types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolBinding {
    /// JSON-RPC 2.0 over HTTP(S)
    JsonRpc,
    /// gRPC
    Grpc,
}

/// Agent capabilities
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Supports SSE streaming responses
    #[serde(default)]
    pub streaming: bool,

    /// Supports push notifications via webhooks
    #[serde(default)]
    pub push_notifications: bool,

    /// Supports state transition history
    #[serde(default)]
    pub state_transition_history: bool,

    /// Optional protocol extensions this agent understands
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<AgentExtension>>,
}

/// A skill that the agent can perform
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// Unique identifier for this skill
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Description of what this skill does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Input content types this skill accepts
    #[serde(default)]
    pub input_modes: Vec<String>,

    /// Output content types this skill produces
    #[serde(default)]
    pub output_modes: Vec<String>,

    /// Example prompts for this skill
    #[serde(default)]
    pub examples: Vec<String>,

    /// Optional per-skill security requirements
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<HashMap<String, Vec<String>>>>,
}

/// Security/authentication scheme
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityScheme {
    /// Scheme identifier
    pub id: String,

    /// Type of authentication
    #[serde(rename = "type")]
    pub scheme_type: SecuritySchemeType,

    /// Description of this scheme
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// For API key: header name or query param name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// For API key: where the key is sent
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub location: Option<ApiKeyLocation>,

    /// For OAuth2: flows configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows: Option<serde_json::Value>,

    /// For OpenID Connect: discovery URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openid_connect_url: Option<String>,

    /// For OAuth2: discovery metadata endpoint per RFC 8414
    #[serde(skip_serializing_if = "Option::is_none", rename = "metadataUrl")]
    pub metadata_url: Option<String>,
}

/// Types of security schemes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SecuritySchemeType {
    ApiKey,
    Http,
    OAuth2,
    OpenIdConnect,
    MutualTls,
}

/// Location for API key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

/// Builder for creating Agent Cards
#[derive(Debug, Default)]
pub struct AgentCardBuilder {
    name: Option<String>,
    description: Option<String>,
    icon_url: Option<String>,
    documentation_url: Option<String>,
    provider: Option<AgentProvider>,
    base_url: Option<String>,
    preferred_transport: Option<ProtocolBinding>,
    additional_interfaces: Vec<SupportedInterface>,
    capabilities: AgentCapabilities,
    skills: Vec<Skill>,
    security_schemes: Vec<SecurityScheme>,
    input_modes: Vec<String>,
    output_modes: Vec<String>,
    metadata: Option<serde_json::Value>,
    signatures: Vec<AgentCardSignature>,
    supports_authenticated_card: Option<bool>,
}

impl AgentCardBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the agent name (required)
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the agent description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the agent icon URL
    pub fn icon_url(mut self, url: impl Into<String>) -> Self {
        self.icon_url = Some(url.into());
        self
    }

    /// Set the documentation URL
    pub fn documentation_url(mut self, url: impl Into<String>) -> Self {
        self.documentation_url = Some(url.into());
        self
    }

    /// Set the provider information
    pub fn provider(mut self, organization: impl Into<String>, url: Option<String>) -> Self {
        self.provider = Some(AgentProvider {
            organization: organization.into(),
            url,
        });
        self
    }

    /// Set the base URL for the agent
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Set the preferred transport reported in the agent card
    pub fn preferred_transport(mut self, protocol: ProtocolBinding) -> Self {
        self.preferred_transport = Some(protocol);
        self
    }

    /// Append an additional interface beyond the default base URL
    pub fn add_interface(mut self, protocol: ProtocolBinding, url: impl Into<String>) -> Self {
        self.additional_interfaces.push(SupportedInterface {
            protocol,
            url: url.into(),
        });
        self
    }

    /// Enable streaming support
    pub fn streaming(mut self, enabled: bool) -> Self {
        self.capabilities.streaming = enabled;
        self
    }

    /// Enable push notification support
    pub fn push_notifications(mut self, enabled: bool) -> Self {
        self.capabilities.push_notifications = enabled;
        self
    }

    /// Add a skill
    pub fn skill(mut self, id: impl Into<String>, name: impl Into<String>, description: Option<String>) -> Self {
        self.skills.push(Skill {
            id: id.into(),
            name: name.into(),
            description,
            tags: vec![],
            input_modes: vec!["text/plain".to_string()],
            output_modes: vec!["text/plain".to_string()],
            examples: vec![],
            security: None,
        });
        self
    }

    /// Add a skill with full configuration
    pub fn skill_full(mut self, skill: Skill) -> Self {
        self.skills.push(skill);
        self
    }

    /// Add an API key security scheme
    pub fn api_key_auth(mut self, header_name: impl Into<String>) -> Self {
        self.security_schemes.push(SecurityScheme {
            id: "api_key".to_string(),
            scheme_type: SecuritySchemeType::ApiKey,
            description: Some("API key authentication".to_string()),
            name: Some(header_name.into()),
            location: Some(ApiKeyLocation::Header),
            flows: None,
            openid_connect_url: None,
            metadata_url: None,
        });
        self
    }

    /// Add bearer token authentication
    pub fn bearer_auth(mut self) -> Self {
        self.security_schemes.push(SecurityScheme {
            id: "bearer".to_string(),
            scheme_type: SecuritySchemeType::Http,
            description: Some("Bearer token authentication".to_string()),
            name: None,
            location: None,
            flows: None,
            openid_connect_url: None,
            metadata_url: None,
        });
        self
    }

    /// Attach a capability extension advertised by this agent
    pub fn add_extension(mut self, extension: AgentExtension) -> Self {
        let extensions = self
            .capabilities
            .extensions
            .get_or_insert_with(Vec::new);
        extensions.push(extension);
        self
    }

    /// Attach a signature entry to the card
    pub fn add_signature(mut self, signature: AgentCardSignature) -> Self {
        self.signatures.push(signature);
        self
    }

    /// Indicate whether authenticated extended cards are supported
    pub fn supports_authenticated_extended_card(mut self, enabled: bool) -> Self {
        self.supports_authenticated_card = Some(enabled);
        self
    }

    /// Set default input modes
    pub fn input_modes(mut self, modes: Vec<String>) -> Self {
        self.input_modes = modes;
        self
    }

    /// Set default output modes
    pub fn output_modes(mut self, modes: Vec<String>) -> Self {
        self.output_modes = modes;
        self
    }

    /// Set additional metadata
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Build the Agent Card
    pub fn build(self) -> Result<AgentCard, &'static str> {
        let name = self.name.ok_or("Agent name is required")?;
        let base_url = self.base_url.ok_or("Base URL is required")?;

        let default_input_modes = if self.input_modes.is_empty() {
            vec!["text/plain".to_string(), "application/json".to_string()]
        } else {
            self.input_modes
        };

        let default_output_modes = if self.output_modes.is_empty() {
            vec!["text/plain".to_string(), "application/json".to_string()]
        } else {
            self.output_modes
        };

        let preferred_transport = self
            .preferred_transport
            .unwrap_or(ProtocolBinding::JsonRpc);

        let mut interfaces = Vec::with_capacity(1 + self.additional_interfaces.len());
        interfaces.push(SupportedInterface {
            protocol: preferred_transport.clone(),
            url: base_url,
        });
        interfaces.extend(self.additional_interfaces);

        Ok(AgentCard {
            protocol_version: PROTOCOL_VERSION.to_string(),
            name,
            description: self.description,
            icon_url: self.icon_url,
            documentation_url: self.documentation_url,
            provider: self.provider,
            supported_interfaces: interfaces,
            preferred_transport: Some(preferred_transport),
            capabilities: self.capabilities,
            skills: self.skills,
            security_schemes: if self.security_schemes.is_empty() {
                None
            } else {
                Some(self.security_schemes)
            },
            default_input_modes,
            default_output_modes,
            signatures: if self.signatures.is_empty() {
                None
            } else {
                Some(self.signatures)
            },
            supports_authenticated_extended_card: self.supports_authenticated_card,
            metadata: self.metadata,
        })
    }
}

impl AgentCard {
    /// Create a new builder
    pub fn builder() -> AgentCardBuilder {
        AgentCardBuilder::new()
    }

    /// Get the JSON-RPC endpoint URL
    pub fn jsonrpc_url(&self) -> Option<&str> {
        self.supported_interfaces
            .iter()
            .find(|i| i.protocol == ProtocolBinding::JsonRpc)
            .map(|i| i.url.as_str())
    }

    /// Check if this agent supports streaming
    pub fn supports_streaming(&self) -> bool {
        self.capabilities.streaming
    }

    /// Check if this agent supports push notifications
    pub fn supports_push_notifications(&self) -> bool {
        self.capabilities.push_notifications
    }

    /// Find a skill by ID
    pub fn find_skill(&self, id: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.id == id)
    }

    /// Parse an agent card from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_agent_card_builder() {
        let card = AgentCard::builder()
            .name("test-agent")
            .description("A test agent")
            .base_url("http://localhost:8080")
            .streaming(true)
            .skill("chat", "Chat", Some("General conversation".to_string()))
            .bearer_auth()
            .build()
            .unwrap();

        assert_eq!(card.name, "test-agent");
        assert!(card.capabilities.streaming);
        assert_eq!(card.skills.len(), 1);
        assert!(card.security_schemes.is_some());
    }

    #[test]
    fn test_agent_card_serialization() {
        let card = AgentCard::builder()
            .name("test-agent")
            .base_url("http://localhost:8080")
            .build()
            .unwrap();

        let json = card.to_json().unwrap();
        let parsed = AgentCard::from_json(&json).unwrap();
        assert_eq!(parsed.name, card.name);
    }

    #[test]
    fn test_security_scheme_metadata_serialization() {
        let scheme = SecurityScheme {
            id: "oauth".to_string(),
            scheme_type: SecuritySchemeType::OAuth2,
            description: None,
            name: None,
            location: None,
            flows: Some(json!({
                "clientCredentials": {
                    "tokenUrl": "https://example.com/token",
                    "scopes": {"default": "Default scope"}
                }
            })),
            openid_connect_url: None,
            metadata_url: Some("https://example.com/.well-known/oauth-authorization-server".to_string()),
        };

        let json_value = serde_json::to_value(&scheme).unwrap();
        assert_eq!(json_value["metadataUrl"], "https://example.com/.well-known/oauth-authorization-server");
        assert_eq!(json_value["type"], "oAuth2");
    }

    #[test]
    fn test_agent_card_with_signature() {
        let signature = AgentCardSignature {
            protected: "eyJhbGciOiJSUzI1NiJ9".to_string(),
            signature: "dGVzdF9zaWduYXR1cmU".to_string(),
            header: Some(HashMap::from([("alg".to_string(), json!("RS256"))])),
        };

        let card = AgentCard::builder()
            .name("signed-agent")
            .base_url("https://example.com/api")
            .add_signature(signature)
            .supports_authenticated_extended_card(true)
            .build()
            .unwrap();

        let card_json = serde_json::to_value(&card).unwrap();
        assert!(card_json.get("signatures").is_some());
        assert_eq!(card_json["supportsAuthenticatedExtendedCard"], true);
    }
}
