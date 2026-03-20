use std::time::Instant;

use agent_core::AgentMessage;

use crate::app::runtime::agent_stream::AgentStreamOutcome;
use crate::app::runtime::agent_stream_policy::{
    find_latest_compaction_summary, is_tool_error_result, parse_thinking_summary,
    should_trigger_mid_stream_auto_summarize,
};
use crate::app::state::message::{CompactionEntry, MessageState, MessageType};
use crate::app::state::orchestrator::ToolCallStatus;
use crate::app::state::ui_message_event::UiMessageEvent;
use crate::app::{App, StepToolCallEntry, TodoItem};

pub(super) fn drain_agent_rx_impl(app: &mut App) -> AgentStreamOutcome {
    let mut outcome = AgentStreamOutcome::new();
    if let Some(rx) = &mut app.agent_rx {
        while let Ok(msg) = rx.try_recv() {
            // Skip processing agent messages if we've interrupted
            if app.agent_state.agent_interrupted {
                match msg {
                    AgentMessage::GenerationStats(stats) => {
                        App::record_generation_stats_fields(
                            &mut app.generation_stats,
                            &mut app.generation_stats_rendered,
                            stats,
                        );

                        if !app.agent_state.agent_processing && !app.thinking_indicator_active {
                            App::ensure_generation_stats_marker_fields(
                                &mut app.messages,
                                &mut app.message_types,
                                &mut app.message_states,
                                &mut app.message_metadata,
                                &mut app.message_timestamps,
                                &app.generation_stats,
                                &mut app.generation_stats_rendered,
                            );
                        }
                    }
                    AgentMessage::Done => {
                        App::remove_thinking_animation_placeholder(
                            &mut app.messages,
                            &mut app.message_types,
                        );
                        app.thinking_indicator_active = false;
                        app.is_thinking = false;
                        app.thinking_start_time = None;
                        app.thinking_current_summary = None;
                        app.thinking_token_count = 0;

                        App::ensure_generation_stats_marker_fields(
                            &mut app.messages,
                            &mut app.message_types,
                            &mut app.message_states,
                            &mut app.message_metadata,
                            &mut app.message_timestamps,
                            &app.generation_stats,
                            &mut app.generation_stats_rendered,
                        );

                        app.agent_state.agent_interrupted = false;
                        app.safety_state.show_approval_prompt = false;
                        app.safety_state.approval_prompt_content.clear();
                        app.safety_state.show_sandbox_prompt = false;
                        app.safety_state.sandbox_blocked_path.clear();
                        // Still check for auto-summarization even after interruption
                        // If context is low, we should summarize regardless
                        outcome.check_auto_summarize = true;
                    }
                    _ => {}
                }
                continue;
            }

            // Skip main agent messages while orchestration is running
            if app.orchestration_in_progress {
                match &msg {
                    AgentMessage::ThinkingContent(_, _)
                    | AgentMessage::AgentResponse(_, _)
                    | AgentMessage::ThinkingSummary(_)
                    | AgentMessage::GenerationStats(_)
                    | AgentMessage::ToolCallStarted(_, _)
                    | AgentMessage::ToolCallCompleted(_, _) => {
                        // Discard main agent output while orchestrating
                        // Subagent tool calls go through OrchestratorEvent, not here
                        continue;
                    }
                    AgentMessage::Done => {
                        // Agent finished but orchestration still running - ignore
                        continue;
                    }
                    _ => {}
                }
            }

            match msg {
                AgentMessage::ThinkingContent(thinking, token_count) => {
                    if app.limit_thinking_to_first_token && app.agent_state.agent_response_started {
                        continue;
                    }
                    // Add or maintain thinking animation placeholder
                    let should_add_thinking = if let Some(last_msg) = app.messages.last() {
                        !matches!(
                            UiMessageEvent::parse(last_msg),
                            Some(UiMessageEvent::ThinkingAnimation)
                        )
                    } else {
                        true
                    };

                    if should_add_thinking {
                        app.messages
                            .push(UiMessageEvent::ThinkingAnimation.to_message());
                        app.message_types.push(MessageType::Agent);
                        app.thinking_indicator_active = true;
                    }
                    app.is_thinking = true;
                    // Don't reset thinking_start_time here - it was already set on submit
                    if app.thinking_start_time.is_none() {
                        app.thinking_start_time = Some(Instant::now());
                    }

                    // Accumulate raw thinking content for export
                    app.thinking_raw_content.push_str(&thinking);

                    // Use actual token count from tokenizer
                    app.thinking_token_count += token_count;

                    if should_trigger_mid_stream_auto_summarize(
                        app.agent_state.is_compacting,
                        app.is_auto_summarize,
                        app.queued_messages.is_empty(),
                        app.current_context_tokens,
                        app.last_known_context_tokens,
                        app.streaming_completion_tokens,
                        app.thinking_token_count,
                        app.auto_summarize_threshold,
                    ) {
                        outcome.trigger_mid_stream_auto_summarize = true;
                    }
                }
                AgentMessage::ThinkingSummary(summary) => {
                    if app.limit_thinking_to_first_token {
                        continue;
                    }
                    let (summary_text, token_count, chunk_count) = parse_thinking_summary(&summary);

                    // If we have a current summary, move it to a static tree line
                    if let Some((old_summary, old_tokens, old_chunks)) =
                        app.thinking_current_summary.take()
                    {
                        App::remove_thinking_animation_placeholder(
                            &mut app.messages,
                            &mut app.message_types,
                        );

                        // Add old summary as static tree line with token count and chunk count
                        app.messages.push(App::format_thinking_tree_line(
                            old_summary,
                            old_tokens,
                            old_chunks,
                            false,
                        ));
                        app.message_types.push(MessageType::Agent);

                        App::ensure_thinking_animation_placeholder(
                            &mut app.messages,
                            &mut app.message_types,
                            app.thinking_indicator_active,
                        );
                    }
                    // Store new summary as current (will show with snowflake)
                    app.thinking_current_summary = Some((summary_text, token_count, chunk_count));
                    // Reset animation position to start wave from beginning
                    app.thinking_position = 0;
                }
                AgentMessage::AgentResponse(text, token_count) => {
                    // Skip empty responses
                    if text.is_empty() {
                        continue;
                    }

                    // Accumulate completion tokens for real-time context tracking
                    app.streaming_completion_tokens += token_count;

                    if should_trigger_mid_stream_auto_summarize(
                        app.agent_state.is_compacting,
                        app.is_auto_summarize,
                        app.queued_messages.is_empty(),
                        app.current_context_tokens,
                        app.last_known_context_tokens,
                        app.streaming_completion_tokens,
                        app.thinking_token_count,
                        app.auto_summarize_threshold,
                    ) {
                        outcome.trigger_mid_stream_auto_summarize = true;
                    }

                    // IMPORTANT: Remove thinking animation FIRST, unconditionally
                    App::remove_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                    );

                    // THEN convert summary to static tree line if it exists
                    if let Some((final_summary, token_count, chunk_count)) =
                        app.thinking_current_summary.take()
                    {
                        app.messages.push(App::format_thinking_tree_line(
                            final_summary,
                            token_count,
                            chunk_count,
                            true,
                        ));
                        app.message_types.push(MessageType::Agent);
                    }
                    app.is_thinking = false;
                    // Note: Don't clear thinking_raw_content here - it will be used in export

                    // Check if we should append to existing message or create new one
                    // Determine if the last rendered entry is a thinking tree line.
                    let last_is_thinking_tree = app
                        .messages
                        .last()
                        .map(|msg| msg.starts_with("├── ") || msg.starts_with("└── "))
                        .unwrap_or(false);

                    let should_create_new = if !app.agent_state.agent_response_started {
                        // First chunk of agent response - always create new message
                        true
                    } else if last_is_thinking_tree {
                        // When a thinking summary was just rendered, force a fresh bubble
                        true
                    } else if let Some(last_msg) = app.messages.last() {
                        // Already started - create a new bubble if the last row is a
                        // structured UI/event line, not plain assistant prose.
                        last_msg.starts_with('[')
                            || last_msg.starts_with(" ⎿ ")
                            || last_msg == "● Interrupted"
                            || last_msg.starts_with("├── ")
                            || last_msg.starts_with("└── ")
                    } else {
                        true
                    };

                    if should_create_new {
                        app.messages.push(text);
                        app.message_types.push(MessageType::Agent);
                        app.agent_state.agent_response_started = true;
                    } else {
                        // Append to existing agent response
                        if let Some(last_msg) = app.messages.last_mut() {
                            last_msg.push_str(&text);
                        }
                    }

                    App::ensure_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                        app.thinking_indicator_active,
                    );
                }
                AgentMessage::ToolCallStarted(tool_name, arguments) => {
                    // Keep thinking animation visible by moving it below tool call output
                    App::remove_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                    );

                    // Convert summary to static tree line if it exists, but keep the animation running
                    if let Some((current_summary, token_count, chunk_count)) =
                        app.thinking_current_summary.take()
                    {
                        app.messages.push(App::format_thinking_tree_line(
                            current_summary,
                            token_count,
                            chunk_count,
                            false,
                        ));
                        app.message_types.push(MessageType::Agent);
                    }

                    // Store raw arguments for file change tracking
                    app.last_tool_args = Some((tool_name.clone(), arguments.clone()));

                    // Format arguments for display
                    let formatted_args = App::format_tool_arguments(&tool_name, &arguments);
                    let started_event = UiMessageEvent::ToolCallStarted {
                        tool_name: tool_name.clone(),
                        args: formatted_args.clone(),
                    }
                    .to_message();

                    let mut updated_existing_started = false;
                    for msg in app.messages.iter_mut().rev() {
                        let Some(UiMessageEvent::ToolCallStarted {
                            tool_name: started_tool,
                            args: existing_args,
                        }) = UiMessageEvent::parse(msg)
                        else {
                            continue;
                        };

                        if started_tool == tool_name {
                            if existing_args != formatted_args {
                                *msg = started_event.clone();
                            }
                            updated_existing_started = true;
                            break;
                        }
                    }

                    if !updated_existing_started {
                        app.messages.push(started_event);
                        app.message_types.push(MessageType::Agent);
                    }
                    app.thinking_indicator_active = true;

                    if app.current_spec.is_some() {
                        if let Some(prefix) = app.active_step_prefix.clone() {
                            let planned_label = if prefix.contains('.') {
                                app.step_label_overrides.get(&prefix).cloned()
                            } else {
                                None
                            };
                            let label = planned_label
                                .unwrap_or_else(|| App::describe_tool_call(&tool_name, &arguments));
                            let entry_id = app.next_tool_call_id;
                            app.next_tool_call_id = app.next_tool_call_id.saturating_add(1);
                            let session_metadata = app.orchestrator_sessions.get(&prefix);
                            let role = session_metadata
                                .map(|entry| entry.role.clone())
                                .unwrap_or(crate::app::SessionRole::Implementor);
                            let worktree_branch =
                                session_metadata.and_then(|entry| entry.worktree_branch.clone());
                            let worktree_path =
                                session_metadata.and_then(|entry| entry.worktree_path.clone());
                            app.step_tool_calls.entry(prefix.clone()).or_default().push(
                                StepToolCallEntry {
                                    id: entry_id,
                                    label,
                                    status: ToolCallStatus::Started,
                                    role,
                                    worktree_branch,
                                    worktree_path,
                                },
                            );
                            app.active_tool_call = Some((prefix, entry_id));
                        }
                    }

                    App::ensure_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                        app.thinking_indicator_active,
                    );

                    // Keep thinking animation running during tool calls
                    // Don't stop thinking animation - let it continue during tool execution
                    // app.is_thinking = false;
                    // app.thinking_start_time = None;
                    // app.thinking_token_count = 0;
                    // Note: Don't clear thinking_raw_content here - it will be used in export
                }
                AgentMessage::ToolCallCompleted(tool_name, result) => {
                    let is_error = is_tool_error_result(&result);

                    if let Some((prefix, entry_id)) = app.active_tool_call.take() {
                        if let Some(entries) = app.step_tool_calls.get_mut(&prefix) {
                            if let Some(entry) =
                                entries.iter_mut().find(|entry| entry.id == entry_id)
                            {
                                entry.status = if is_error {
                                    ToolCallStatus::Error
                                } else {
                                    ToolCallStatus::Completed
                                };
                            }
                        }
                    }

                    // Check for sandbox permission errors and offer to add to writable roots
                    if let Ok(result_yaml) = serde_yaml::from_str::<serde_yaml::Value>(&result) {
                        if let Some(obj) = result_yaml.as_mapping() {
                            if let Some(msg) = obj
                                .get(serde_yaml::Value::String("message".to_string()))
                                .and_then(|v| v.as_str())
                            {
                                // Check if this is a sandbox permission error
                                if msg.contains("Sandbox denied")
                                    || msg.contains("permission denied")
                                {
                                    // Try to extract file path from the error message
                                    // Typical format: "Sandbox denied (code N): path/to/file"
                                    if let Some(path_start) = msg.find("): ") {
                                        let potential_path = msg[path_start + 3..].trim();
                                        // Basic validation that it looks like a path
                                        if potential_path.starts_with('/')
                                            || potential_path.starts_with('.')
                                        {
                                            // Show sandbox permission prompt
                                            app.safety_state.sandbox_blocked_path =
                                                potential_path.to_string();
                                            app.safety_state.show_sandbox_prompt = true;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Special handling for todo_write tool
                    if tool_name == "todo_write" {
                        // Parse the result to extract todos and store them for saving
                        if let Ok(result_json) = serde_json::from_str::<serde_json::Value>(&result)
                        {
                            if let Some(todos_array) =
                                result_json.get("todos").and_then(|v| v.as_array())
                            {
                                let todos: Vec<TodoItem> = todos_array
                                    .iter()
                                    .filter_map(|t| App::parse_todo_item(t))
                                    .collect();

                                // Store todos to be saved after message processing
                                outcome.pending_todos = Some(todos);
                            }
                        }
                    }

                    // Special handling for orchestrate_task tool
                    if tool_name == "orchestrate_task" {
                        if let Ok(result_json) = serde_json::from_str::<serde_json::Value>(&result)
                        {
                            if result_json.get("status").and_then(|v| v.as_str())
                                == Some("orchestration_requested")
                            {
                                if let Some(goal) = result_json.get("goal").and_then(|v| v.as_str())
                                {
                                    app.orchestration_pending = Some(goal.to_string());
                                    // Block main agent while orchestration runs
                                    app.orchestration_in_progress = true;
                                    // Stop the main agent's thinking animation
                                    app.thinking_indicator_active = false;
                                    App::remove_thinking_animation_placeholder(
                                        &mut app.messages,
                                        &mut app.message_types,
                                    );
                                }
                            }
                        }
                    }

                    // Track file changes for Write and Edit tools using stored raw arguments
                    if let Some((stored_tool, stored_args)) = &app.last_tool_args {
                        if stored_tool == &tool_name {
                            outcome.pending_file_change =
                                Some((tool_name.clone(), stored_args.clone(), result.clone()));
                        }
                    }

                    // If thinking is active, remove thinking animation temporarily
                    App::remove_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                    );

                    // Find and replace the started message with completed.
                    // If no started entry is present (stream ordering/race), append completed.
                    let mut replaced_started = false;
                    for msg in app.messages.iter_mut().rev() {
                        let Some(UiMessageEvent::ToolCallStarted {
                            tool_name: started_tool,
                            args,
                        }) = UiMessageEvent::parse(msg)
                        else {
                            continue;
                        };
                        if started_tool == tool_name {
                            let raw_args = app
                                .last_tool_args
                                .as_ref()
                                .filter(|(stored_tool, _)| stored_tool == &tool_name)
                                .map(|(_, args)| args.as_str());
                            let formatted_result =
                                App::format_tool_result(&tool_name, &result, raw_args);
                            *msg = UiMessageEvent::ToolCallCompleted {
                                tool_name: tool_name.clone(),
                                args,
                                result: formatted_result,
                                raw_arguments: raw_args.map(str::to_string),
                            }
                            .to_message();
                            replaced_started = true;
                            break;
                        }
                    }

                    if !replaced_started {
                        let fallback_args = app
                            .last_tool_args
                            .as_ref()
                            .filter(|(stored_tool, _)| stored_tool == &tool_name)
                            .map(|(_, args)| App::format_tool_arguments(&tool_name, args))
                            .unwrap_or_default();
                        let raw_args = app
                            .last_tool_args
                            .as_ref()
                            .filter(|(stored_tool, _)| stored_tool == &tool_name)
                            .map(|(_, args)| args.as_str());
                        let formatted_result =
                            App::format_tool_result(&tool_name, &result, raw_args);
                        app.messages.push(
                            UiMessageEvent::ToolCallCompleted {
                                tool_name: tool_name.clone(),
                                args: fallback_args,
                                result: formatted_result,
                                raw_arguments: raw_args.map(str::to_string),
                            }
                            .to_message(),
                        );
                        app.message_types.push(MessageType::Agent);
                    }

                    // Re-add thinking animation at the bottom while tool results stream
                    // But NOT if orchestration just started - it has its own animation
                    if !app.orchestration_in_progress {
                        app.thinking_indicator_active = true;
                        App::ensure_thinking_animation_placeholder(
                            &mut app.messages,
                            &mut app.message_types,
                            app.thinking_indicator_active,
                        );
                    }
                }
                AgentMessage::RequestApproval(content) => {
                    app.safety_state.show_approval_prompt = true;
                    app.safety_state.approval_prompt_content = content;
                }
                AgentMessage::ThinkingComplete(_residual_tokens) => {
                    // Thinking content has stopped streaming, but keep the indicator
                    // active so the user still sees progress while tool calls run.
                    app.is_thinking = false;
                }
                AgentMessage::Error(err) => {
                    // IMPORTANT: Remove thinking animation FIRST, unconditionally
                    App::remove_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                    );

                    // For errors, discard the thinking summary (don't convert to static tree line)
                    app.thinking_current_summary = None;

                    // Add the error message
                    app.messages.push(format!("[Error: {}]", err));
                    app.message_types.push(MessageType::Agent);
                    app.agent_state.agent_processing = false;
                    app.is_thinking = false;
                    app.thinking_indicator_active = false;
                    app.safety_state.show_approval_prompt = false;
                    app.safety_state.approval_prompt_content.clear();
                    app.safety_state.show_sandbox_prompt = false;
                    app.safety_state.sandbox_blocked_path.clear();
                    app.thinking_start_time = None;
                    app.thinking_token_count = 0;
                    app.agent_state.agent_response_started = false;
                }
                AgentMessage::GenerationStats(stats) => {
                    // Store the generation stats
                    App::record_generation_stats_fields(
                        &mut app.generation_stats,
                        &mut app.generation_stats_rendered,
                        stats,
                    );

                    if !app.agent_state.agent_processing && !app.thinking_indicator_active {
                        App::ensure_generation_stats_marker_fields(
                            &mut app.messages,
                            &mut app.message_types,
                            &mut app.message_states,
                            &mut app.message_metadata,
                            &mut app.message_timestamps,
                            &app.generation_stats,
                            &mut app.generation_stats_rendered,
                        );
                    }
                }
                AgentMessage::BackgroundTaskStarted(session_id, command, log_file) => {
                    // Add background task to the list with current time as start time
                    app.background_tasks.push((
                        session_id,
                        command,
                        log_file,
                        std::time::Instant::now(),
                    ));
                }
                AgentMessage::ContextCleared => {
                    // Context cleared - if no inject expected, sync is complete
                    if !app.context_inject_expected {
                        app.context_sync_pending = false;
                        app.context_sync_started = None;
                    }
                    // Otherwise wait for ContextInjected
                }
                AgentMessage::ContextInjected => {
                    // Context injection complete - sync is done
                    app.context_sync_pending = false;
                    app.context_sync_started = None;
                    app.context_inject_expected = false;
                    outcome.schedule_resume_prompt = true;
                }
                AgentMessage::Done => {
                    // IMPORTANT: Remove thinking animation FIRST, unconditionally
                    App::remove_thinking_animation_placeholder(
                        &mut app.messages,
                        &mut app.message_types,
                    );

                    // THEN convert summary to static tree line if it exists
                    if let Some((final_summary, token_count, chunk_count)) =
                        app.thinking_current_summary.take()
                    {
                        app.messages.push(App::format_thinking_tree_line(
                            final_summary,
                            token_count,
                            chunk_count,
                            true,
                        ));
                        app.message_types.push(MessageType::Agent);
                    }
                    app.agent_state.agent_processing = false;
                    app.is_thinking = false;
                    app.thinking_indicator_active = false;
                    app.safety_state.show_approval_prompt = false;
                    app.safety_state.approval_prompt_content.clear();
                    app.safety_state.show_sandbox_prompt = false;
                    app.safety_state.sandbox_blocked_path.clear();
                    app.thinking_start_time = None;
                    app.thinking_token_count = 0;
                    app.streaming_completion_tokens = 0; // Reset for next turn
                    app.agent_state.agent_response_started = false;

                    // Handle compaction completion
                    if app.agent_state.is_compacting {
                        let was_auto_summarize = app.is_auto_summarize;

                        let summary =
                            find_latest_compaction_summary(&app.messages, &app.message_types)
                                .unwrap_or_default();

                        // Check if we got a valid summary before clearing anything
                        if summary.is_empty() {
                            // Compaction failed - preserve conversation state
                            app.agent_state.is_compacting = false;
                            app.is_auto_summarize = false;
                            app.compaction_resume_prompt = None;
                            app.compaction_resume_ready = false;

                            // Show error banner but keep all existing messages
                            let error_msg = if was_auto_summarize {
                                " ⎿ Auto-summarization failed: no summary generated. Conversation preserved."
                            } else {
                                " ⎿ Summarization failed: no summary generated. Conversation preserved."
                            };
                            app.messages.push(error_msg.to_string());
                            app.message_types.push(MessageType::Agent);
                            app.message_states.push(MessageState::Sent);
                            app.message_metadata.push(None);
                            app.message_timestamps.push(std::time::SystemTime::now());

                            // Skip to allow retry later
                            continue;
                        }

                        // Valid summary - now safe to clear and proceed
                        app.agent_state.is_compacting = false;
                        app.is_auto_summarize = false;

                        // Capture a rewind point before wiping the transcript
                        if let Some(rewind_point) = App::snapshot_rewind_point(
                            &app.messages,
                            &app.message_types,
                            &app.message_states,
                            &app.message_metadata,
                            &app.message_timestamps,
                            &app.current_file_changes,
                        ) {
                            app.rewind_points.push(rewind_point);
                            app.current_file_changes.clear();
                            if app.rewind_points.len() > 50 {
                                app.rewind_points.remove(0);
                            }
                        }

                        // Clear all messages
                        app.messages.clear();
                        app.message_types.clear();
                        app.message_states.clear();
                        app.message_metadata.clear();
                        app.message_timestamps.clear();

                        // Clear agent context and inject summary as new context
                        if let Some(tx) = &app.agent_tx {
                            // Start context sync - will wait for ContextInjected
                            app.context_sync_pending = true;
                            app.context_sync_started = Some(Instant::now());
                            app.context_inject_expected = true;
                            let _ = tx.send(AgentMessage::ClearContext);
                            let _ = tx.send(AgentMessage::InjectContext(summary.clone()));
                        }

                        // Add the summary as the new context (summary is guaranteed non-empty here)
                        app.last_compacted_summary = Some(summary.clone());

                        app.compaction_history.push(CompactionEntry {
                            summary: summary.clone(),
                        });
                        if app.compaction_history.len() > crate::app::MAX_COMPACTION_HISTORY {
                            app.compaction_history.remove(0);
                        }
                        app.summary_history_selected =
                            app.compaction_history.len().saturating_sub(1);

                        // Different banner for auto vs manual summarization
                        let banner_text = if was_auto_summarize {
                            "Context low · auto-summarized · ctrl+o for history"
                        } else {
                            "Conversation summarized · ctrl+o for history"
                        };
                        let banner_line =
                            format!("{}{}", crate::app::SUMMARY_BANNER_PREFIX, banner_text);
                        app.messages.push(banner_line);
                        app.message_types.push(MessageType::Agent);
                        app.message_states.push(MessageState::Sent);
                        app.message_metadata.push(None);
                        app.message_timestamps.push(std::time::SystemTime::now());

                        // Different user message for auto vs manual
                        let user_msg = if was_auto_summarize {
                            "[auto-summarized]".to_string()
                        } else {
                            "/summarize".to_string()
                        };
                        app.messages.push(user_msg);
                        app.message_types.push(MessageType::User);
                        app.message_states.push(MessageState::Sent);
                        app.message_metadata.push(None);
                        app.message_timestamps.push(std::time::SystemTime::now());

                        let result_msg = if was_auto_summarize {
                            " ⎿ Auto-summarized (ctrl+o to see full summary)"
                        } else {
                            " ⎿ Summarized (ctrl+o to see full summary)"
                        };
                        app.messages.push(result_msg.to_string());
                        app.message_types.push(MessageType::Agent);
                        app.message_states.push(MessageState::Sent);
                        app.message_metadata.push(None);
                        app.message_timestamps.push(std::time::SystemTime::now());

                        if app.compaction_resume_prompt.is_some() {
                            app.compaction_resume_ready = true;
                        }

                        // Skip normal done handling for compaction
                        continue;
                    }

                    // Check for interrupt pending FIRST
                    if let Some(interrupt_msg) = app.agent_state.interrupt_pending.take() {
                        // Mark last message (interrupted one) as Interrupted
                        {
                            if let Some(last_state) = app.message_states.last_mut() {
                                if matches!(last_state, MessageState::Sent) {
                                    *last_state = MessageState::Interrupted;
                                }
                            }
                        }

                        // Add interrupt marker message
                        app.messages.push("● Interrupted".to_string());
                        app.message_types.push(MessageType::Agent);
                        app.message_states.push(MessageState::Sent);

                        // Add the prompt message
                        app.messages
                            .push(" ⎿ What should Nite do instead?".to_string());
                        app.message_types.push(MessageType::Agent);
                        app.message_states.push(MessageState::Sent);

                        App::ensure_generation_stats_marker_fields(
                            &mut app.messages,
                            &mut app.message_types,
                            &mut app.message_states,
                            &mut app.message_metadata,
                            &mut app.message_timestamps,
                            &app.generation_stats,
                            &mut app.generation_stats_rendered,
                        );

                        // Set flag to process interrupt after rx is dropped
                        outcome.process_interrupt = Some(interrupt_msg);
                    } else {
                        // Update last message state from Queued to Sent if needed
                        {
                            if let Some(last_state) = app.message_states.last_mut() {
                                if matches!(last_state, MessageState::Queued) {
                                    *last_state = MessageState::Sent;
                                }
                            }
                        }

                        // Set flag to create a rewind point after rx is dropped
                        outcome.create_rewind = true;

                        outcome.process_queued = true; // Set flag to process queued message after rx is dropped

                        // Set flag to check for auto-summarization after rx borrow is dropped
                        outcome.check_auto_summarize = true;

                        App::ensure_generation_stats_marker_fields(
                            &mut app.messages,
                            &mut app.message_types,
                            &mut app.message_states,
                            &mut app.message_metadata,
                            &mut app.message_timestamps,
                            &app.generation_stats,
                            &mut app.generation_stats_rendered,
                        );
                    }
                }
                AgentMessage::ModelLoaded => {
                    // Model has been loaded successfully
                    app.messages
                        .push(" ✔ Model loaded successfully".to_string());
                    app.message_types.push(MessageType::Agent);
                    app.message_states.push(MessageState::Sent);
                }
                _ => {}
            }
        }
    }
    outcome
}
