use crate::{
    Agent, AgentMessage, GenerationStats, ThinkingStartDecision, analyze_thinking_start,
    has_visible_text, preview_thinking, thinking_debug_log,
};
use futures::StreamExt;
use indexmap::IndexMap;
use mistralrs::{
    ChatCompletionChunkResponse, Delta, RequestBuilder, Response, TextMessageRole,
    ToolCallResponse, ToolChoice,
};
use std::time::Instant;
use tokio::sync::mpsc;

impl Agent {
    pub async fn process_message(
        &self,
        user_message: String,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> crate::Result<()> {
        self.reset_cancel();

        let mut conversation_guard = self.conversation.lock().await;

        let request_builder = if let Some(existing_conversation) = conversation_guard.take() {
            existing_conversation.add_message(TextMessageRole::User, &user_message)
        } else {
            let system_prompt_content = {
                let system_prompt_guard = self.system_prompt.lock().await;
                system_prompt_guard.clone()
            };

            let tools = {
                let tools_guard = self.tools.lock().await;
                tools_guard.clone()
            };
            RequestBuilder::new()
                .add_message(TextMessageRole::System, &system_prompt_content)
                .add_message(TextMessageRole::User, &user_message)
                .set_tools(tools)
                .set_tool_choice(ToolChoice::Auto)
                .enable_thinking(true)
        };
        drop(conversation_guard);

        self.run_generation(request_builder, tx).await
    }

    async fn run_generation(
        &self,
        request_builder: RequestBuilder,
        tx: mpsc::UnboundedSender<AgentMessage>,
    ) -> crate::Result<()> {
        use crate::exec_command::execute_tool_call;

        let mut current_request_builder = request_builder;
        let mut has_more_tool_calls = true;
        let mut _final_accumulated_content = String::new();

        while has_more_tool_calls {
            let mut stream = self
                .backend
                .stream_chat_request(current_request_builder.clone())
                .await?;
            let mut accumulated_tool_calls: IndexMap<usize, ToolCallResponse> = IndexMap::new();
            let mut accumulated_content = String::new();
            has_more_tool_calls = false;

            let stream_start_time = Instant::now();
            let mut first_token_time: Option<Instant> = None;
            let mut total_generated_tokens: usize = 0;

            let mut in_thinking = false;
            let mut thinking_buffer = String::new();
            let mut allow_thinking_start = true;
            let mut pending_prefix = String::new();
            let mut pending_agent_response_prefix = String::new();
            let mut final_response_started = false;

            loop {
                macro_rules! check_cancel {
                    () => {
                        if self.is_cancel_requested() {
                            if !thinking_buffer.is_empty() && in_thinking {
                                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                                summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                                summarizer_guard.flush().await;
                                for (summary, token_count, chunk_count) in
                                    summarizer_guard.get_new_summaries()
                                {
                                    let _ = tx.send(AgentMessage::ThinkingSummary(format!(
                                        "{}|{}|{}",
                                        summary, token_count, chunk_count
                                    )));
                                }
                            }

                            if !accumulated_content.is_empty() && !in_thinking {
                                let token_count =
                                    Self::estimate_tokens_heuristic(&accumulated_content);
                                let _ = tx.send(AgentMessage::AgentResponse(
                                    accumulated_content.clone(),
                                    token_count,
                                ));
                            }

                            let elapsed_sec = stream_start_time.elapsed().as_secs_f32();
                            let time_to_first = first_token_time
                                .map(|t| t.duration_since(stream_start_time).as_secs_f32())
                                .unwrap_or(0.0);
                            let api_usage = self.backend.get_latest_usage().await;
                            let completion_tokens = total_generated_tokens;
                            let prompt_tokens =
                                api_usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0);
                            let avg_tok_per_sec = if elapsed_sec > 0.0 && total_generated_tokens > 0
                            {
                                total_generated_tokens as f32 / elapsed_sec
                            } else {
                                0.0
                            };
                            let stats = GenerationStats {
                                avg_completion_tok_per_sec: avg_tok_per_sec,
                                completion_tokens,
                                prompt_tokens,
                                time_to_first_token_sec: time_to_first,
                                stop_reason: "cancelled".to_string(),
                            };
                            let _ = tx.send(AgentMessage::GenerationStats(stats));

                            let mut updated_request = current_request_builder.clone();
                            if !accumulated_content.is_empty() {
                                updated_request = updated_request
                                    .add_message(TextMessageRole::Assistant, &accumulated_content);
                            }
                            let mut conversation_guard = self.conversation.lock().await;
                            *conversation_guard = Some(updated_request);
                            drop(conversation_guard);

                            let _ = tx.send(AgentMessage::Done);
                            return Ok(());
                        }
                    };
                }

                check_cancel!();

                let response = tokio::select! {
                    res = stream.next() => {
                        match res {
                            Some(r) => r,
                            None => break,
                        }
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                        check_cancel!();
                        continue;
                    }
                };

                match response {
                    Response::Chunk(ChatCompletionChunkResponse { choices, usage, .. }) => {
                        if let Some(usage_stats) = usage {
                            if accumulated_tool_calls.is_empty() {
                                let stop_reason = choices
                                    .first()
                                    .and_then(|c| c.finish_reason.as_ref())
                                    .cloned()
                                    .unwrap_or_else(|| "unknown".to_string());
                                let prompt_tokens = if usage_stats.prompt_tokens > 0 {
                                    usage_stats.prompt_tokens
                                } else if usage_stats.total_tokens > usage_stats.completion_tokens {
                                    usage_stats.total_tokens - usage_stats.completion_tokens
                                } else {
                                    0
                                };

                                let stats = GenerationStats {
                                    avg_completion_tok_per_sec: usage_stats.avg_compl_tok_per_sec,
                                    completion_tokens: usage_stats.completion_tokens,
                                    prompt_tokens,
                                    time_to_first_token_sec: usage_stats.total_prompt_time_sec,
                                    stop_reason,
                                };

                                let _ = tx.send(AgentMessage::GenerationStats(stats));
                            }
                        }

                        if let Some(choice) = choices.first() {
                            match &choice.delta {
                                Delta {
                                    content: Some(content),
                                    tool_calls: None,
                                    ..
                                } => {
                                    if content.is_empty() {
                                        continue;
                                    }

                                    let thinking_tags_guard = self.thinking_tags.lock().await;
                                    let open_tag = thinking_tags_guard.open_tag.clone();
                                    drop(thinking_tags_guard);

                                    let mut chunk_content = content.clone();
                                    let mut process_as_thinking = in_thinking;

                                    if allow_thinking_start {
                                        pending_prefix.push_str(&chunk_content);

                                        match analyze_thinking_start(&pending_prefix, &open_tag) {
                                            ThinkingStartDecision::NeedMoreData => {
                                                check_cancel!();
                                                continue;
                                            }
                                            ThinkingStartDecision::Detected {
                                                content_start_idx,
                                            } => {
                                                thinking_debug_log(
                                                    "Detected <think> start in HTTP chunk",
                                                );
                                                allow_thinking_start = false;
                                                in_thinking = true;
                                                process_as_thinking = true;
                                                let after_tag =
                                                    pending_prefix.split_off(content_start_idx);
                                                pending_prefix.clear();
                                                chunk_content = after_tag;
                                            }
                                            ThinkingStartDecision::NotThinking => {
                                                thinking_debug_log(
                                                    "Chunk does not start with <think>, treating as visible content",
                                                );
                                                allow_thinking_start = false;
                                                chunk_content = pending_prefix.clone();
                                                pending_prefix.clear();
                                            }
                                        }
                                    }

                                    if process_as_thinking {
                                        if chunk_content.is_empty() {
                                            continue;
                                        }

                                        thinking_buffer.push_str(&chunk_content);

                                        let thinking_tags_guard = self.thinking_tags.lock().await;
                                        let close_tag = thinking_tags_guard.close_tag.clone();
                                        drop(thinking_tags_guard);

                                        let end_tag_result = thinking_buffer
                                            .find(close_tag.as_str())
                                            .map(|idx| (idx, close_tag.len()));

                                        if let Some((end_idx, end_tag_len)) = end_tag_result {
                                            in_thinking = false;
                                            thinking_debug_log("Detected  closing tag");

                                            let final_thinking = &thinking_buffer[..end_idx];
                                            if !final_thinking.is_empty() {
                                                let token_count =
                                                    Self::estimate_tokens_heuristic(final_thinking);
                                                if first_token_time.is_none() && token_count > 0 {
                                                    first_token_time = Some(Instant::now());
                                                }
                                                total_generated_tokens += token_count;
                                                let _ = tx.send(AgentMessage::ThinkingContent(
                                                    final_thinking.to_string(),
                                                    token_count,
                                                ));
                                                thinking_debug_log(format!(
                                                    "Sent ThinkingContent (final) tokens={} preview=\"{}\"",
                                                    token_count,
                                                    preview_thinking(final_thinking)
                                                ));
                                                check_cancel!();

                                                let mut summarizer_guard =
                                                    self.thinking_summarizer.lock().await;
                                                summarizer_guard
                                                    .add_thinking_chunk(final_thinking)
                                                    .await;
                                                for (summary, token_count, chunk_count) in
                                                    summarizer_guard.get_new_summaries()
                                                {
                                                    let _ = tx.send(AgentMessage::ThinkingSummary(
                                                        format!(
                                                            "{}|{}|{}",
                                                            summary, token_count, chunk_count
                                                        ),
                                                    ));
                                                    check_cancel!();
                                                }
                                            }

                                            let mut summarizer_guard =
                                                self.thinking_summarizer.lock().await;
                                            summarizer_guard.flush().await;
                                            for (summary, token_count, chunk_count) in
                                                summarizer_guard.get_new_summaries()
                                            {
                                                let _ = tx.send(AgentMessage::ThinkingSummary(
                                                    format!(
                                                        "{}|{}|{}",
                                                        summary, token_count, chunk_count
                                                    ),
                                                ));
                                                check_cancel!();
                                            }
                                            let residual_tokens =
                                                summarizer_guard.get_residual_token_count();
                                            if residual_tokens > 0 {
                                                let _ = tx.send(AgentMessage::ThinkingComplete(
                                                    residual_tokens,
                                                ));
                                            }

                                            let after_think =
                                                &thinking_buffer[end_idx + end_tag_len..];
                                            if !after_think.is_empty() {
                                                if has_visible_text(after_think) {
                                                    let mut outbound = String::new();
                                                    if !pending_agent_response_prefix.is_empty() {
                                                        outbound.push_str(
                                                            &pending_agent_response_prefix,
                                                        );
                                                        pending_agent_response_prefix.clear();
                                                    }
                                                    outbound.push_str(after_think);
                                                    accumulated_content.push_str(&outbound);
                                                    let token_count =
                                                        Self::estimate_tokens_heuristic(&outbound);
                                                    if first_token_time.is_none() && token_count > 0
                                                    {
                                                        first_token_time = Some(Instant::now());
                                                    }
                                                    total_generated_tokens += token_count;
                                                    let _ = tx.send(AgentMessage::AgentResponse(
                                                        outbound,
                                                        token_count,
                                                    ));
                                                    final_response_started = true;
                                                } else {
                                                    pending_agent_response_prefix
                                                        .push_str(after_think);
                                                }
                                            }
                                            thinking_buffer.clear();
                                        } else {
                                            let char_count = thinking_buffer.chars().count();
                                            if char_count > 11 {
                                                let send_char_count = char_count - 11;
                                                if let Some((byte_idx, _)) = thinking_buffer
                                                    .char_indices()
                                                    .nth(send_char_count)
                                                {
                                                    let to_send = &thinking_buffer[..byte_idx];
                                                    let mut remaining = to_send;
                                                    while !remaining.is_empty() {
                                                        check_cancel!();

                                                        let chunk_chars =
                                                            remaining.chars().take(100).count();
                                                        if let Some((chunk_byte_end, _)) = remaining
                                                            .char_indices()
                                                            .nth(chunk_chars)
                                                        {
                                                            let chunk =
                                                                &remaining[..chunk_byte_end];
                                                            let token_count =
                                                                Self::estimate_tokens_heuristic(
                                                                    chunk,
                                                                );
                                                            if first_token_time.is_none()
                                                                && token_count > 0
                                                            {
                                                                first_token_time =
                                                                    Some(Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(
                                                                AgentMessage::ThinkingContent(
                                                                    chunk.to_string(),
                                                                    token_count,
                                                                ),
                                                            );
                                                            thinking_debug_log(format!(
                                                                "Sent ThinkingContent (stream) tokens={} preview=\"{}\"",
                                                                token_count,
                                                                preview_thinking(chunk)
                                                            ));
                                                            remaining =
                                                                &remaining[chunk_byte_end..];
                                                        } else {
                                                            let token_count =
                                                                Self::estimate_tokens_heuristic(
                                                                    remaining,
                                                                );
                                                            if first_token_time.is_none()
                                                                && token_count > 0
                                                            {
                                                                first_token_time =
                                                                    Some(Instant::now());
                                                            }
                                                            total_generated_tokens += token_count;
                                                            let _ = tx.send(
                                                                AgentMessage::ThinkingContent(
                                                                    remaining.to_string(),
                                                                    token_count,
                                                                ),
                                                            );
                                                            thinking_debug_log(format!(
                                                                "Sent ThinkingContent (final chunk) tokens={} preview=\"{}\"",
                                                                token_count,
                                                                preview_thinking(remaining)
                                                            ));
                                                            break;
                                                        }

                                                        check_cancel!();
                                                    }

                                                    let mut summarizer_guard =
                                                        self.thinking_summarizer.lock().await;
                                                    summarizer_guard
                                                        .add_thinking_chunk(to_send)
                                                        .await;
                                                    for (summary, token_count, chunk_count) in
                                                        summarizer_guard.get_new_summaries()
                                                    {
                                                        let _ = tx.send(
                                                            AgentMessage::ThinkingSummary(format!(
                                                                "{}|{}|{}",
                                                                summary, token_count, chunk_count
                                                            )),
                                                        );
                                                        check_cancel!();
                                                    }

                                                    thinking_buffer =
                                                        thinking_buffer[byte_idx..].to_string();
                                                }
                                            }
                                        }
                                    } else {
                                        let chunk_has_visible = has_visible_text(&chunk_content);
                                        if !final_response_started && !chunk_has_visible {
                                            pending_agent_response_prefix.push_str(&chunk_content);
                                            check_cancel!();
                                            continue;
                                        }

                                        let mut outbound = String::new();
                                        if !pending_agent_response_prefix.is_empty() {
                                            outbound.push_str(&pending_agent_response_prefix);
                                            pending_agent_response_prefix.clear();
                                        }
                                        outbound.push_str(&chunk_content);
                                        accumulated_content.push_str(&outbound);
                                        let token_count = 1 + Self::estimate_tokens_heuristic(
                                            &outbound[..outbound
                                                .len()
                                                .saturating_sub(chunk_content.len())],
                                        );
                                        if first_token_time.is_none() && token_count > 0 {
                                            first_token_time = Some(Instant::now());
                                        }
                                        total_generated_tokens += token_count;
                                        let _ = tx.send(AgentMessage::AgentResponse(
                                            outbound.clone(),
                                            token_count,
                                        ));
                                        if chunk_has_visible {
                                            final_response_started = true;
                                        }

                                        check_cancel!();
                                    }
                                }
                                Delta {
                                    tool_calls: Some(tool_calls),
                                    ..
                                } => {
                                    if in_thinking && !thinking_buffer.is_empty() {
                                        let token_count =
                                            Self::estimate_tokens_heuristic(&thinking_buffer);
                                        if first_token_time.is_none() && token_count > 0 {
                                            first_token_time = Some(Instant::now());
                                        }
                                        total_generated_tokens += token_count;
                                        let _ = tx.send(AgentMessage::ThinkingContent(
                                            thinking_buffer.clone(),
                                            token_count,
                                        ));
                                        thinking_debug_log(format!(
                                            "Flushing thinking content before tool call tokens={} preview=\"{}\"",
                                            token_count,
                                            preview_thinking(&thinking_buffer)
                                        ));

                                        let mut summarizer_guard =
                                            self.thinking_summarizer.lock().await;
                                        summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                                        summarizer_guard.flush().await;

                                        for (summary, token_count, chunk_count) in
                                            summarizer_guard.get_new_summaries()
                                        {
                                            let _ =
                                                tx.send(AgentMessage::ThinkingSummary(format!(
                                                    "{}|{}|{}",
                                                    summary, token_count, chunk_count
                                                )));
                                        }

                                        let residual_tokens =
                                            summarizer_guard.get_residual_token_count();
                                        if residual_tokens > 0 {
                                            let _ = tx.send(AgentMessage::ThinkingComplete(
                                                residual_tokens,
                                            ));
                                        }

                                        thinking_buffer.clear();
                                        in_thinking = false;
                                    }

                                    for tool_call in tool_calls {
                                        let previous_args = accumulated_tool_calls
                                            .get(&tool_call.index)
                                            .map(|existing| existing.function.arguments.clone());
                                        let is_new = previous_args.is_none();
                                        accumulated_tool_calls
                                            .insert(tool_call.index, tool_call.clone());
                                        let args_now = tool_call.function.arguments.clone();
                                        let args_changed = previous_args
                                            .as_ref()
                                            .map(|prev| prev != &args_now)
                                            .unwrap_or(true);

                                        if is_new || args_changed {
                                            let _ = tx.send(AgentMessage::ToolCallStarted(
                                                tool_call.function.name.clone(),
                                                args_now,
                                            ));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Response::Done(response) => {
                        if accumulated_tool_calls.is_empty()
                            && let Some(tool_calls) = response
                                .choices
                                .first()
                                .and_then(|choice| choice.message.tool_calls.clone())
                        {
                            for tool_call in tool_calls {
                                let args_now = tool_call.function.arguments.clone();
                                accumulated_tool_calls.insert(tool_call.index, tool_call.clone());
                                let _ = tx.send(AgentMessage::ToolCallStarted(
                                    tool_call.function.name.clone(),
                                    args_now,
                                ));
                            }
                        }

                        if accumulated_tool_calls.is_empty() {
                            let stop_reason = response
                                .choices
                                .first()
                                .map(|c| c.finish_reason.clone())
                                .unwrap_or_else(|| "unknown".to_string());
                            let prompt_tokens = if response.usage.prompt_tokens > 0 {
                                response.usage.prompt_tokens
                            } else if response.usage.total_tokens > response.usage.completion_tokens
                            {
                                response.usage.total_tokens - response.usage.completion_tokens
                            } else {
                                0
                            };

                            let stats = GenerationStats {
                                avg_completion_tok_per_sec: response.usage.avg_compl_tok_per_sec,
                                completion_tokens: response.usage.completion_tokens,
                                prompt_tokens,
                                time_to_first_token_sec: response.usage.total_prompt_time_sec,
                                stop_reason,
                            };

                            let _ = tx.send(AgentMessage::GenerationStats(stats));
                        }
                        break;
                    }
                    Response::InternalError(e) => {
                        let _ = tx.send(AgentMessage::Error(format!("Internal error: {:?}", e)));
                        break;
                    }
                    Response::ValidationError(e) => {
                        let _ = tx.send(AgentMessage::Error(format!("Validation error: {:?}", e)));
                        break;
                    }
                    Response::ModelError(msg, _) => {
                        let _ = tx.send(AgentMessage::Error(format!("Model error: {}", msg)));
                        break;
                    }
                    _ => {}
                }
            }

            if !pending_prefix.is_empty() {
                accumulated_content.push_str(&pending_prefix);
                let token_count = Self::estimate_tokens_heuristic(&pending_prefix);
                let _ = tx.send(AgentMessage::AgentResponse(
                    pending_prefix.clone(),
                    token_count,
                ));
            }

            if in_thinking && !thinking_buffer.is_empty() {
                let token_count = Self::estimate_tokens_heuristic(&thinking_buffer);
                let _ = tx.send(AgentMessage::ThinkingContent(
                    thinking_buffer.clone(),
                    token_count,
                ));
                thinking_debug_log(format!(
                    "Residual thinking flush tokens={} preview=\"{}\"",
                    token_count,
                    preview_thinking(&thinking_buffer)
                ));

                let mut summarizer_guard = self.thinking_summarizer.lock().await;
                summarizer_guard.add_thinking_chunk(&thinking_buffer).await;
                summarizer_guard.flush().await;
                for (summary, token_count, chunk_count) in summarizer_guard.get_new_summaries() {
                    let _ = tx.send(AgentMessage::ThinkingSummary(format!(
                        "{}|{}|{}",
                        summary, token_count, chunk_count
                    )));
                }
                let residual_tokens = summarizer_guard.get_residual_token_count();
                if residual_tokens > 0 {
                    let _ = tx.send(AgentMessage::ThinkingComplete(residual_tokens));
                }
            }

            if accumulated_tool_calls.is_empty() {
                _final_accumulated_content = accumulated_content.clone();
            }

            if !accumulated_tool_calls.is_empty() {
                has_more_tool_calls = true;
                for tool_call in accumulated_tool_calls.values().cloned() {
                    let tool_result = match execute_tool_call(self, &tool_call, tx.clone()).await {
                        Ok(result) => {
                            let _ = tx.send(AgentMessage::ToolCallCompleted(
                                tool_call.function.name.clone(),
                                result.clone(),
                            ));
                            if let Ok(pending_count) = self.pending_execution_change_count().await {
                                let _ = tx.send(AgentMessage::ExecutionState(pending_count));
                            }

                            if tool_call.function.name == "exec_command" {
                                if let Ok(parsed) =
                                    serde_yaml::from_str::<serde_json::Value>(&result)
                                {
                                    if let Some(status) =
                                        parsed.get("status").and_then(|v| v.as_str())
                                    {
                                        if status == "Background" {
                                            let session_id = parsed
                                                .get("session_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let command = parsed
                                                .get("command")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let log_file = parsed
                                                .get("log_file")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let _ = tx.send(AgentMessage::BackgroundTaskStarted(
                                                session_id, command, log_file,
                                            ));
                                        }
                                    }
                                }
                            }

                            result
                        }
                        Err(e) => {
                            let error_yaml = serde_yaml::to_string(&serde_json::json!({
                                "status": "Failure",
                                "message": e.to_string(),
                                "tool": tool_call.function.name.clone(),
                            }))
                            .unwrap_or_else(|_| {
                                "status: Failure\nmessage: Tool execution failed".to_string()
                            });

                            let _ = tx.send(AgentMessage::ToolCallCompleted(
                                tool_call.function.name.clone(),
                                error_yaml.clone(),
                            ));
                            if let Ok(pending_count) = self.pending_execution_change_count().await {
                                let _ = tx.send(AgentMessage::ExecutionState(pending_count));
                            }

                            error_yaml
                        }
                    };

                    current_request_builder = current_request_builder
                        .add_message_with_tool_call(
                            TextMessageRole::Assistant,
                            accumulated_content.clone(),
                            vec![tool_call.clone()],
                        )
                        .add_tool_message(&tool_result, &tool_call.id);
                }
            }
        }

        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(current_request_builder);
        drop(conversation_guard);

        let _ = tx.send(AgentMessage::Done);
        Ok(())
    }

    fn estimate_tokens_heuristic(text: &str) -> usize {
        text.len() / 4
    }
}
