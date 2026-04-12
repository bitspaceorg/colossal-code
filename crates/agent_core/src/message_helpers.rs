use mistralrs::{RequestLike, TextMessageRole, Tool, ToolChoice};

pub fn role_from_str(role: &str) -> TextMessageRole {
    match role {
        "system" => TextMessageRole::System,
        "user" => TextMessageRole::User,
        "assistant" => TextMessageRole::Assistant,
        "tool" => TextMessageRole::Tool,
        other => TextMessageRole::Custom(other.to_string()),
    }
}

pub fn value_left_str(value: &serde_json::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("Left").and_then(|inner| inner.as_str()))
}

pub fn message_left_str<'a>(message: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    message.get(key).and_then(value_left_str)
}

pub fn message_right<'a>(
    message: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Value> {
    message
        .get(key)
        .and_then(|value| value.get("Right").or(Some(value)))
}

pub fn parse_tool_calls(value: &serde_json::Value) -> Vec<mistralrs::ToolCallResponse> {
    use mistralrs::{CalledFunction, ToolCallType};
    value
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, entry)| {
            let id = entry.get("id")?.as_str()?.to_string();
            let function = entry.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let arguments = function.get("arguments")?.as_str()?.to_string();
            Some(mistralrs::ToolCallResponse {
                index,
                id,
                tp: ToolCallType::Function,
                function: CalledFunction { name, arguments },
            })
        })
        .collect()
}

pub fn rebuild_request_builder(
    existing: &mistralrs::RequestBuilder,
    system_prompt: &str,
    tools: Vec<Tool>,
    reminder: Option<&str>,
) -> anyhow::Result<mistralrs::RequestBuilder> {
    let serialized = serde_json::to_value(existing.messages_ref())?;
    let mut builder = mistralrs::RequestBuilder::new()
        .add_message(TextMessageRole::System, system_prompt)
        .set_tools(tools)
        .set_tool_choice(ToolChoice::Auto)
        .enable_thinking(true);

    if let Some(messages) = serialized.as_array() {
        for message in messages {
            let Some(role) = message_left_str(message, "role") else {
                continue;
            };
            if role == "system" {
                continue;
            }

            if role == "tool"
                && let (Some(content), Some(tool_call_id)) = (
                    message_left_str(message, "content"),
                    message_left_str(message, "tool_call_id"),
                )
            {
                builder = builder.add_tool_message(content, tool_call_id);
                continue;
            }

            let role = role_from_str(role);
            let content_text = message_left_str(message, "content").unwrap_or_default();

            if let Some(functions) = message_right(message, "function") {
                let tool_calls = parse_tool_calls(functions);
                if !tool_calls.is_empty() {
                    builder = builder.add_message_with_tool_call(role, content_text, tool_calls);
                    continue;
                }
            }

            if !content_text.is_empty() {
                builder = builder.add_message(role, content_text);
            }
        }
    }

    if let Some(reminder) = reminder {
        builder = builder.add_message(TextMessageRole::System, reminder);
    }

    Ok(builder)
}
