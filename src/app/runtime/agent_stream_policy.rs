use crate::app::state::message::MessageType;

pub(crate) fn should_trigger_mid_stream_auto_summarize(
    is_compacting: bool,
    is_auto_summarize: bool,
    queued_messages_empty: bool,
    current_context_tokens: Option<usize>,
    last_known_context_tokens: usize,
    streaming_completion_tokens: usize,
    thinking_token_count: usize,
    auto_summarize_threshold: f32,
) -> bool {
    if is_compacting || is_auto_summarize || !queued_messages_empty {
        return false;
    }

    let Some(limit) = current_context_tokens else {
        return false;
    };
    if limit == 0 {
        return false;
    }

    let streaming_tokens = streaming_completion_tokens.saturating_add(thinking_token_count);
    let used = last_known_context_tokens.saturating_add(streaming_tokens);
    if used == 0 {
        return false;
    }

    let remaining = limit.saturating_sub(used);
    let percent_left = (remaining as f32 / limit as f32 * 100.0).clamp(0.0, 100.0);
    let percent_used = 100.0 - percent_left;

    percent_used >= auto_summarize_threshold
}

pub(crate) fn parse_thinking_summary(summary: &str) -> (String, usize, usize) {
    if let Some(last_pipe) = summary.rfind('|') {
        let chunk_str = &summary[last_pipe + 1..];
        let chunk_count = chunk_str.parse::<usize>().unwrap_or(0);

        let summary_without_chunk = &summary[..last_pipe];
        if let Some(first_pipe) = summary_without_chunk.rfind('|') {
            let text = summary_without_chunk[..first_pipe].to_string();
            let token_str = &summary_without_chunk[first_pipe + 1..];
            let token_count = token_str.parse::<usize>().unwrap_or(0);
            (text, token_count, chunk_count)
        } else {
            (summary.to_string(), 0, 0)
        }
    } else {
        (summary.to_string(), 0, 0)
    }
}

pub(crate) fn is_tool_error_result(result: &str) -> bool {
    result.starts_with("Error:")
        || result.starts_with("error:")
        || result.starts_with("Failed:")
        || result.starts_with("failed:")
        || result.starts_with("Permission denied")
        || result.starts_with("No such file")
        || result.starts_with("Command failed")
        || (result.len() < 500 && result.contains("\"error\""))
        || (result.len() < 500 && result.contains("\"is_error\": true"))
        || result.contains("status: Failure")
}

pub(crate) fn find_latest_compaction_summary(
    messages: &[String],
    message_types: &[MessageType],
) -> Option<String> {
    messages
        .iter()
        .zip(message_types.iter())
        .rev()
        .find_map(|(msg, msg_type)| {
            if matches!(msg_type, MessageType::Agent)
                && !msg.starts_with('[')
                && !msg.starts_with(" ⎿")
                && !msg.starts_with('●')
            {
                Some(msg.clone())
            } else {
                None
            }
        })
}
