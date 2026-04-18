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

pub fn request_builder_from_serialized_messages(
    messages: &[serde_json::Value],
    tools: Vec<Tool>,
    replacement_system_prompt: Option<&str>,
) -> anyhow::Result<mistralrs::RequestBuilder> {
    let mut builder = mistralrs::RequestBuilder::new()
        .set_tools(tools)
        .set_tool_choice(ToolChoice::Auto)
        .enable_thinking(true);

    if let Some(system_prompt) = replacement_system_prompt {
        builder = builder.add_message(TextMessageRole::System, system_prompt);
    }

    let mut skipped_replaced_system = false;

    for message in messages {
        let Some(role_name) = message_left_str(message, "role") else {
            continue;
        };

        if role_name == "system" && replacement_system_prompt.is_some() && !skipped_replaced_system
        {
            skipped_replaced_system = true;
            continue;
        }

        if role_name == "tool"
            && let (Some(content), Some(tool_call_id)) = (
                message_left_str(message, "content"),
                message_left_str(message, "tool_call_id"),
            )
        {
            builder = builder.add_tool_message(content, tool_call_id);
            continue;
        }

        let role = role_from_str(role_name);
        let content_text = message_left_str(message, "content").unwrap_or_default();

        if let Some(functions) =
            message_right(message, "tool_calls").or_else(|| message_right(message, "function"))
        {
            let tool_calls = parse_tool_calls(functions);
            if !tool_calls.is_empty() {
                builder = builder.add_message_with_tool_call(role, content_text, tool_calls);
                continue;
            }
        }

        if role_name == "system" || !content_text.is_empty() {
            builder = builder.add_message(role, content_text);
        }
    }

    Ok(builder)
}

pub fn rebuild_request_builder(
    existing: &mistralrs::RequestBuilder,
    system_prompt: &str,
    tools: Vec<Tool>,
    reminder: Option<&str>,
) -> anyhow::Result<mistralrs::RequestBuilder> {
    let serialized = serde_json::to_value(existing.messages_ref())?;
    let mut builder = request_builder_from_serialized_messages(
        serialized.as_array().map(Vec::as_slice).unwrap_or(&[]),
        tools,
        Some(system_prompt),
    )?;

    if let Some(reminder) = reminder {
        builder = builder.add_message(TextMessageRole::System, reminder);
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_builder_replaces_only_first_system_prompt_and_preserves_reminders_and_tool_history()
    {
        let builder = request_builder_from_serialized_messages(
            &[
                json!({
                    "role": "system",
                    "content": "You are Claude Code, Anthropic's official CLI for Claude. Old auth prompt.",
                }),
                json!({
                    "role": "user",
                    "content": "create the file with exec",
                }),
                json!({
                    "role": "assistant",
                    "content": "I will inspect and then run the command.",
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "function": {
                                "name": "exec_command",
                                "arguments": "{\"command\":\"touch demo.txt\"}",
                            }
                        }
                    ]
                }),
                json!({
                    "role": "tool",
                    "tool_call_id": "call-1",
                    "content": "created demo.txt",
                }),
                json!({
                    "role": "system",
                    "content": "<system-reminder>\nYour operational mode has changed from plan to build.\nYou are no longer in read-only mode.\nYou are permitted to make file changes, run shell commands, and utilize your arsenal of tools as needed.\n</system-reminder>",
                }),
                json!({
                    "role": "assistant",
                    "content": "The file exists now.",
                }),
            ],
            vec![],
            Some("You are powered by the model named gpt-5.4."),
        )
        .expect("rebuild request builder");

        let messages = serde_json::to_value(builder.messages_ref()).expect("serialize messages");
        let entries = messages.as_array().expect("message array");

        assert_eq!(message_left_str(&entries[0], "role"), Some("system"));
        assert_eq!(
            message_left_str(&entries[0], "content"),
            Some("You are powered by the model named gpt-5.4."),
        );
        assert_eq!(
            message_left_str(&entries[1], "content"),
            Some("create the file with exec")
        );
        assert_eq!(message_left_str(&entries[3], "role"), Some("tool"));
        assert_eq!(
            message_left_str(&entries[3], "content"),
            Some("created demo.txt")
        );
        assert_eq!(message_left_str(&entries[4], "role"), Some("system"));
        assert!(
            message_left_str(&entries[4], "content")
                .unwrap_or_default()
                .contains("You are no longer in read-only mode."),
        );

        let tool_calls = message_right(&entries[2], "tool_calls")
            .or_else(|| message_right(&entries[2], "function"))
            .and_then(|value| value.as_array())
            .expect("assistant tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(
            tool_calls[0]
                .get("function")
                .and_then(|value| value.get("name"))
                .and_then(|value| value.as_str()),
            Some("exec_command")
        );
    }
}
