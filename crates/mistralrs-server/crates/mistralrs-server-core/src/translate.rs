use std::collections::HashMap;

use either::Either;
use indexmap::IndexMap;
use mistralrs_core::{
    Constraint, Function, MessageContent as CoreMessageContent, NormalRequest, RequestMessage,
    Response, SamplingParams, Tool as CoreTool, ToolChoice as CoreToolChoice, ToolType,
};
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;
use upstream_mistralrs_server_core::util::parse_image_url;

use crate::{
    ChatContent, ChatMessage, ChatRequest, ContextControls, EmbeddingInput, EmbeddingRequest,
    GenerateRequest, MessageRole, ModelManagerError, StructuredOutputSchema, ToolChoice,
    ToolDefinition, ToolCall,
};

use mistralrs_core::EmbeddingRequest as EngineEmbeddingRequest;

/// Builds sampling params shared by chat and completion flows.
pub fn to_sampling_params(
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    controls: &Option<ContextControls>,
    logit_bias: Option<&HashMap<String, f32>>,
) -> SamplingParams {
    let mut params = SamplingParams::deterministic();
    params.temperature = temperature.map(|value| value as f64);
    params.max_len = max_tokens.map(|value| value as usize);
    params.top_n_logprobs = 0;
    params.n_choices = 1;
    params.logits_bias = logit_bias.and_then(convert_logit_bias);

    if let Some(ctx) = controls {
        params.top_k = ctx.top_k.map(|value| value as usize);
        params.top_p = ctx.top_p.map(|value| value as f64);
        params.min_p = ctx.min_p.map(|value| value as f64);
        params.repetition_penalty = ctx.repeat_penalty;
        params.presence_penalty = ctx.presence_penalty;
        params.frequency_penalty = ctx.frequency_penalty;
    }

    params
}

/// Creates a [`NormalRequest`] for `/api/generate` style completions.
pub fn build_generate_request(
    req: &GenerateRequest,
    tx: Sender<Response>,
    request_id: usize,
    streaming: bool,
) -> Result<NormalRequest, ModelManagerError> {
    let sampling_params = to_sampling_params(
        req.temperature,
        req.max_tokens,
        &req.context,
        req.logit_bias.as_ref(),
    );
    let (tools, tool_choice) = convert_tools(&req.tools, &req.tool_choice)?;

    Ok(NormalRequest {
        messages: RequestMessage::Completion {
            text: req.prompt.clone(),
            echo_prompt: false,
            best_of: None,
        },
        sampling_params,
        response: tx,
        return_logprobs: false,
        is_streaming: streaming,
        id: request_id,
        constraint: structured_constraint(&req.structured_output),
        suffix: None,
        tools,
        tool_choice,
        logits_processors: None,
        return_raw_logits: false,
        web_search_options: None,
        model_id: Some(req.model.clone()),
    })
}

/// Creates an embedding request for the upstream engine.
pub fn build_embedding_request(
    req: &EmbeddingRequest,
    tx: Sender<Response>,
    request_id: usize,
) -> EngineEmbeddingRequest {
    let inputs = match &req.input {
        EmbeddingInput::Single(value) => vec![value.clone()],
        EmbeddingInput::Multiple(values) => values.clone(),
    };
    EngineEmbeddingRequest {
        inputs,
        normalize: req.normalize,
        id: request_id,
        model_id: Some(req.model.clone()),
        response: tx,
    }
}

/// Creates a [`NormalRequest`] for `/api/chat` style interactions.
pub async fn build_chat_request(
    req: &ChatRequest,
    tx: Sender<Response>,
    request_id: usize,
    streaming: bool,
) -> Result<NormalRequest, ModelManagerError> {
    let (messages, image_urls) = convert_chat_messages(&req.messages)?;
    let sampling_params = to_sampling_params(
        req.temperature,
        req.max_tokens,
        &req.context,
        req.logit_bias.as_ref(),
    );

    let request_message = if image_urls.is_empty() {
        RequestMessage::Chat {
            messages,
            enable_thinking: None,
        }
    } else {
        let mut images = Vec::with_capacity(image_urls.len());
        for url in image_urls {
            let parsed = parse_image_url(&url)
                .await
                .map_err(|err| ModelManagerError::Other(err.to_string()))?;
            images.push(parsed);
        }
        RequestMessage::VisionChat {
            messages,
            images,
            audios: Vec::new(),
            enable_thinking: None,
        }
    };

    let (tools, tool_choice) = convert_tools(&req.tools, &req.tool_choice)?;

    Ok(NormalRequest {
        messages: request_message,
        sampling_params,
        response: tx,
        return_logprobs: false,
        is_streaming: streaming,
        id: request_id,
        constraint: structured_constraint(&req.structured_output),
        suffix: None,
        tools,
        tool_choice,
        logits_processors: None,
        return_raw_logits: false,
        web_search_options: None,
        model_id: Some(req.model.clone()),
    })
}

fn convert_logit_bias(
    source: &HashMap<String, f32>,
) -> Option<HashMap<u32, f32>> {
    let converted: HashMap<u32, f32> = source
        .iter()
        .filter_map(|(token, bias)| token.parse::<u32>().ok().map(|id| (id, *bias)))
        .collect();
    if converted.is_empty() {
        None
    } else {
        Some(converted)
    }
}

fn convert_tools(
    defs: &[ToolDefinition],
    choice: &ToolChoice,
) -> Result<(Option<Vec<CoreTool>>, Option<CoreToolChoice>), ModelManagerError> {
    let tools: Vec<CoreTool> = defs
        .iter()
        .map(|tool| CoreTool {
            tp: ToolType::Function,
            function: Function {
                description: tool.description.clone(),
                name: tool.name.clone(),
                parameters: match &tool.parameters {
                    Value::Object(map) => Some(map.clone().into_iter().collect()),
                    other => Some(HashMap::from([("schema".to_string(), other.clone())])),
                },
            },
        })
        .collect();
    let converted_choice = match choice {
        ToolChoice::Auto => None,
        ToolChoice::None => Some(CoreToolChoice::None),
        ToolChoice::Tool(name) => Some(
            tools
                .iter()
                .find(|tool| tool.function.name == *name)
                .cloned()
                .map(CoreToolChoice::Tool)
                .ok_or_else(|| ModelManagerError::Other(format!("unknown tool: {name}")))?,
        ),
    };
    Ok(((!tools.is_empty()).then_some(tools), converted_choice))
}

fn structured_constraint(schema: &Option<StructuredOutputSchema>) -> Constraint {
    schema
        .as_ref()
        .map(|s| Constraint::JsonSchema(s.schema.clone()))
        .unwrap_or(Constraint::None)
}

fn convert_chat_messages(
    messages: &[ChatMessage],
) -> Result<(Vec<IndexMap<String, CoreMessageContent>>, Vec<String>), ModelManagerError> {
    let mut converted = Vec::with_capacity(messages.len());
    let mut image_urls = Vec::new();
    for message in messages {
        let role = role_as_str(&message.role).to_string();
        let (content, mut urls) = convert_message_content(&message.content)?;
        image_urls.append(&mut urls);

        let mut map = IndexMap::new();
        map.insert("role".to_string(), Either::Left(role));
        map.insert("content".to_string(), content);
        if let Some(name) = &message.name {
            map.insert("name".to_string(), Either::Left(name.clone()));
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            map.insert("tool_call_id".to_string(), Either::Left(tool_call_id.clone()));
        }
        if !message.tool_calls.is_empty() {
            let entries = message
                .tool_calls
                .iter()
                .map(tool_call_entry)
                .collect::<Vec<_>>();
            map.insert("function".to_string(), Either::Right(entries));
        }
        converted.push(map);
    }
    Ok((converted, image_urls))
}

fn role_as_str(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn convert_message_content(
    content: &[ChatContent],
) -> Result<(CoreMessageContent, Vec<String>), ModelManagerError> {
    if content.is_empty() {
        return Ok((Either::Left(String::new()), Vec::new()));
    }
    let mut urls = Vec::new();
    let mut text_parts = Vec::new();
    let mut structured_parts = Vec::new();
    let mut has_media = false;

    for chunk in content {
        match chunk {
            ChatContent::Text { text } => {
                if has_media {
                    let mut map = IndexMap::new();
                    map.insert("type".to_string(), Value::String("text".to_string()));
                    map.insert("text".to_string(), Value::String(text.clone()));
                    structured_parts.push(map);
                } else {
                    text_parts.push(text.clone());
                }
            }
            ChatContent::Image { data, mime_type } => {
                has_media = true;
                let url = format!("data:{};base64,{}", mime_type, data);
                urls.push(url.clone());
                structured_parts.push(image_part(url));
            }
            ChatContent::ImageUrl { url } => {
                has_media = true;
                urls.push(url.clone());
                structured_parts.push(image_part(url.clone()));
            }
        }
    }

    if has_media {
        if !text_parts.is_empty() {
            let mut map = IndexMap::new();
            map.insert("type".to_string(), Value::String("text".to_string()));
            map.insert("text".to_string(), Value::String(text_parts.join("\\n")));
            structured_parts.push(map);
        }
        Ok((Either::Right(structured_parts), urls))
    } else {
        Ok((Either::Left(text_parts.join("\n")), urls))
    }
}

fn image_part(url: String) -> IndexMap<String, Value> {
    let mut map = IndexMap::new();
    map.insert("type".to_string(), Value::String("image_url".to_string()));
    map.insert("image_url".to_string(), json!({"url": url}));
    map
}

fn tool_call_entry(call: &ToolCall) -> IndexMap<String, Value> {
    let mut map = IndexMap::new();
    map.insert("id".to_string(), Value::String(call.id.clone()));
    map.insert("type".to_string(), Value::String("function".to_string()));
    map.insert(
        "function".to_string(),
        json!({
            "name": call.name,
            "arguments": call.arguments.to_string(),
        }),
    );
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ChatContent, ChatMessage, ContextControls, EmbeddingInput, EmbeddingRequest, MessageRole,
        ToolDefinition,
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    #[test]
    fn sampling_params_reflect_context() {
        let ctx = ContextControls {
            num_ctx: Some(8192),
            top_k: Some(50),
            top_p: Some(0.8),
            min_p: Some(0.1),
            repeat_penalty: Some(1.1),
            presence_penalty: Some(0.2),
            frequency_penalty: Some(0.3),
        };
        let bias = HashMap::from([("42".to_string(), 1.5)]);
        let params = to_sampling_params(Some(0.7), Some(64), &Some(ctx.clone()), Some(&bias));
        assert!((params.temperature.unwrap() - 0.7f64).abs() < 1e-6);
        assert_eq!(params.max_len, Some(64));
        assert_eq!(params.top_k, Some(50));
        assert!((params.top_p.unwrap() - 0.8f64).abs() < 1e-6);
        assert!((params.min_p.unwrap() - 0.1f64).abs() < 1e-6);
        assert_eq!(params.repetition_penalty, Some(1.1));
        assert_eq!(params.presence_penalty, Some(0.2));
        assert_eq!(params.frequency_penalty, Some(0.3));
        assert_eq!(params.logits_bias.as_ref().unwrap().get(&42), Some(&1.5));
    }

    #[test]
    fn build_embedding_request_single_input() {
        let (tx, _rx) = mpsc::channel(1);
        let req = EmbeddingRequest {
            model: "demo".into(),
            input: EmbeddingInput::Single("hello".into()),
            normalize: true,
        };
        let engine = build_embedding_request(&req, tx, 11);
        assert_eq!(engine.inputs, vec!["hello".to_string()]);
        assert!(engine.normalize);
        assert_eq!(engine.id, 11);
        assert_eq!(engine.model_id.as_deref(), Some("demo"));
    }

    #[test]
    fn build_embedding_request_multiple_inputs() {
        let (tx, _rx) = mpsc::channel(1);
        let req = EmbeddingRequest {
            model: "demo".into(),
            input: EmbeddingInput::Multiple(vec!["a".into(), "b".into()]),
            normalize: false,
        };
        let engine = build_embedding_request(&req, tx, 2);
        assert_eq!(engine.inputs, vec!["a".to_string(), "b".to_string()]);
        assert!(!engine.normalize);
    }

    #[test]
    fn convert_message_content_handles_images() {
        let (content, urls) = convert_message_content(&[
            ChatContent::Text {
                text: "describe".to_string(),
            },
            ChatContent::ImageUrl {
                url: "file:///tmp/image.png".to_string(),
            },
        ])
        .expect("conversion");
        assert!(matches!(content, Either::Right(_)));
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn convert_tools_errors_on_unknown_choice() {
        let defs = vec![ToolDefinition {
            name: "foo".into(),
            description: None,
            parameters: json!({"type": "object"}),
        }];
        let err = convert_tools(&defs, &ToolChoice::Tool("bar".into())).unwrap_err();
        assert!(matches!(err, ModelManagerError::Other(msg) if msg.contains("unknown tool")));
    }

    #[tokio::test]
    async fn build_chat_request_promotes_images() {
        let image_data = "R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==";
        let request = ChatRequest {
            model: "vision".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: vec![ChatContent::Image {
                    data: image_data.into(),
                    mime_type: "image/gif".into(),
                }],
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            context: None,
            stream: false,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            keep_alive: None,
            logit_bias: None,
            structured_output: None,
        };
        let (tx, _rx) = mpsc::channel::<Response>(1);
        let normal = build_chat_request(&request, tx, 1, false)
            .await
            .expect("build");
        if let RequestMessage::VisionChat { .. } = normal.messages {
        } else {
            panic!("expected vision chat request");
        }
    }
}
