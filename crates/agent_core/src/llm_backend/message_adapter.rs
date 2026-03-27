use either::Either;
use indexmap::IndexMap;
use mistralrs_core::{CalledFunction, MessageContent, ToolCallResponse, ToolCallType};
use serde_json::Value;

pub(super) fn normalized_message_key(key: &str) -> &str {
    if key == "function" { "tool_calls" } else { key }
}

pub(super) fn assistant_tool_calls_from_message(
    message: &IndexMap<String, MessageContent>,
) -> Vec<ToolCallResponse> {
    let Some(value) = message
        .get("tool_calls")
        .or_else(|| message.get("function"))
    else {
        return Vec::new();
    };

    let Either::Right(entries) = value else {
        return Vec::new();
    };

    entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let index = entry
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(idx);
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("call-{}", index));
            let function = entry.get("function");
            let name = function
                .and_then(|inner| inner.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = function
                .and_then(|inner| inner.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();

            ToolCallResponse {
                index,
                id,
                tp: ToolCallType::Function,
                function: CalledFunction { name, arguments },
            }
        })
        .collect()
}
