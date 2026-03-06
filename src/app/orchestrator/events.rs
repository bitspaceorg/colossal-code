use agent_core::{StepStatus, VerificationStatus, orchestrator::{OrchestratorEvent, SubAgentMessage}};

use crate::{
    App, MessageState, MessageType, SessionRole, StepToolCallEntry, SubAgentContext,
    ToolCallStatus,
};

impl App {
    pub(crate) fn describe_exec_command(command: &str) -> String {
        Self::infer_search_label(command).unwrap_or_else(|| format!("Ran {}", command))
    }

    pub(crate) fn infer_search_label(command: &str) -> Option<String> {
        let normalized = command
            .replace('"', " ")
            .replace('\'', " ")
            .replace('`', " ")
            .replace('(', " ")
            .replace(')', " ");
        let tokens: Vec<&str> = normalized.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }

        for (idx, token) in tokens.iter().enumerate() {
            let lower = token.to_ascii_lowercase();
            if lower != "rg" && lower != "ripgrep" && lower != "grep" {
                continue;
            }

            let mut pattern: Option<String> = None;
            let mut target: Option<String> = None;
            let mut cursor = idx + 1;
            while cursor < tokens.len() {
                let candidate = tokens[cursor];
                if Self::should_skip_token(candidate) || candidate.starts_with('-') {
                    cursor += 1;
                    continue;
                }
                if pattern.is_none() {
                    pattern = Some(candidate.to_string());
                    cursor += 1;
                    continue;
                }
                target = Some(candidate.to_string());
                break;
            }

            if let Some(pattern) = pattern {
                if let Some(target) = target {
                    return Some(format!("Searched {} in {}", pattern, target));
                }
                return Some(format!("Searched {}", pattern));
            }
        }

        None
    }

    pub(crate) fn should_skip_token(token: &str) -> bool {
        matches!(token, "&&" | "||" | "|" | ";") || token.starts_with('>') || token.starts_with('<')
    }

    pub(crate) fn format_tool_arguments(_tool_name: &str, arguments_json: &str) -> String {
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json)
            && let Some(obj) = args.as_object()
        {
            let mut parts = Vec::new();
            for (k, v) in obj {
                let val_str = match v {
                    serde_json::Value::String(s) => {
                        if s.chars().count() > 100 {
                            let truncated: String = s.chars().take(97).collect();
                            format!("\"{}...\"", truncated)
                        } else {
                            format!("\"{}\"", s)
                        }
                    }
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Array(arr) => {
                        let items: Vec<String> = arr
                            .iter()
                            .take(3)
                            .map(|item| match item {
                                serde_json::Value::String(s) => format!("\"{}\"", s),
                                _ => format!("{}", item),
                            })
                            .collect();
                        format!("[{}]", items.join(", "))
                    }
                    serde_json::Value::Null => "null".to_string(),
                    serde_json::Value::Object(_) => "{...}".to_string(),
                };
                parts.push(format!("{}: {}", k, val_str));
            }
            return parts.join(", ");
        }

        "".to_string()
    }

    pub(crate) fn format_tool_result(tool_name: &str, result_yaml: &str) -> String {
        if let Ok(result) = serde_yaml::from_str::<serde_yaml::Value>(result_yaml)
            && let Some(obj) = result.as_mapping()
            && let Some(status) = obj
                .get(serde_yaml::Value::String("status".to_string()))
                .and_then(|v| v.as_str())
        {
            if status == "Success" {
                if let Some(text) = Self::format_success_tool_result(tool_name, obj) {
                    return text;
                }
                return "Success".to_string();
            }
            if status == "Background" {
                return Self::format_background_tool_result(obj);
            }
            if status == "orchestration_requested" {
                return String::new();
            }
            return Self::format_failed_tool_result(obj);
        }

        Self::format_tool_result_fallback(result_yaml)
    }

    fn format_success_tool_result(
        tool_name: &str,
        obj: &serde_yaml::Mapping,
    ) -> Option<String> {
        match tool_name {
            "read_file" => obj
                .get(serde_yaml::Value::String("content".to_string()))
                .and_then(|v| v.as_str())
                .map(|content| {
                    let lines = content.lines().count();
                    let chars = content.chars().count();
                    format!("Read {} lines ({} chars)", lines, chars)
                }),
            "get_files" | "get_files_recursive" => obj
                .get(serde_yaml::Value::String("files".to_string()))
                .and_then(|v| v.as_sequence())
                .map(|files| {
                    if files.is_empty() {
                        return "No files found".to_string();
                    }
                    let file_names: Vec<String> = files
                        .iter()
                        .take(3)
                        .filter_map(|f| f.as_str())
                        .map(|s| s.to_string())
                        .collect();
                    if files.len() > 3 {
                        format!(
                            "Found {} files ({}... +{})",
                            files.len(),
                            file_names.join(", "),
                            files.len() - 3
                        )
                    } else {
                        format!("Found {} files ({})", files.len(), file_names.join(", "))
                    }
                }),
            "search_files_with_regex" | "grep" => obj
                .get(serde_yaml::Value::String("results".to_string()))
                .and_then(|v| v.as_sequence())
                .map(|results| {
                    if results.is_empty() {
                        "No matches found".to_string()
                    } else {
                        format!(
                            "Found {} matches in {} files",
                            results.len(),
                            results.iter().filter_map(|r| r.get("file")).count().max(1)
                        )
                    }
                }),
            "exec_command" => obj
                .get(serde_yaml::Value::String("cmd_out".to_string()))
                .and_then(|v| v.as_str())
                .map(|cmd_out| {
                    let lines = cmd_out.lines().count();
                    if let Some(first_line) = cmd_out.lines().next() {
                        let preview = if first_line.len() > 50 {
                            format!("{}...", &first_line[..47])
                        } else {
                            first_line.to_string()
                        };
                        format!("{} lines: {}", lines, preview)
                    } else {
                        format!("{} lines of output", lines)
                    }
                }),
            "write_file" => Some("File written successfully".to_string()),
            _ => None,
        }
    }

    fn format_background_tool_result(obj: &serde_yaml::Mapping) -> String {
        if let Some(session_id) = obj
            .get(serde_yaml::Value::String("session_id".to_string()))
            .and_then(|v| v.as_str())
        {
            return format!("Started in background (session {})", session_id);
        }
        "Started in background".to_string()
    }

    fn format_failed_tool_result(obj: &serde_yaml::Mapping) -> String {
        if let Some(msg) = obj
            .get(serde_yaml::Value::String("message".to_string()))
            .and_then(|v| v.as_str())
            && !msg.is_empty() && msg != "|+" && msg != "|-" && msg != "|"
        {
            return format!("Error: {}", msg);
        }
        "Failed".to_string()
    }

    fn format_tool_result_fallback(result_yaml: &str) -> String {
        let mut skip_yaml_keys = true;
        for line in result_yaml.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("---")
                || trimmed.starts_with("status:")
                || trimmed.starts_with("path:")
                || trimmed.starts_with("message:")
            {
                continue;
            }
            if trimmed == "|+" || trimmed == "|-" || trimmed == "|" || trimmed == ">" {
                skip_yaml_keys = false;
                continue;
            }
            if !skip_yaml_keys {
                if trimmed.len() > 60 {
                    return format!("{}...", &trimmed[..57]);
                }
                return trimmed.to_string();
            }
        }

        "Completed".to_string()
    }

    pub(crate) fn describe_tool_call(tool_name: &str, arguments_json: &str) -> String {
        let parsed = serde_json::from_str::<serde_json::Value>(arguments_json).ok();
        if let Some(label) = Self::describe_tool_call_from_parsed(tool_name, parsed.as_ref()) {
            return label;
        }

        let friendly = tool_name.replace('_', " ");
        let formatted = Self::format_tool_arguments(tool_name, arguments_json);
        if formatted.is_empty() {
            friendly
        } else {
            format!("{} ({})", friendly, formatted)
        }
    }

    fn describe_tool_call_from_parsed(
        tool_name: &str,
        parsed: Option<&serde_json::Value>,
    ) -> Option<String> {
        match tool_name {
            "exec_command" => parsed
                .and_then(|value| value.get("command"))
                .and_then(|command| command.as_str())
                .map(Self::describe_exec_command),
            "read_file" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Read {}", path)),
            "edit_file" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Edited {}", path)),
            "delete_path" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {}", path)),
            "delete_many" => parsed
                .and_then(|value| value.get("paths"))
                .and_then(|paths| paths.as_array())
                .and_then(|paths| paths.first())
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {} and more", path)),
            "get_files" | "get_files_recursive" => parsed
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Listed {}", path)),
            "search_files_with_regex" => parsed
                .and_then(|value| value.get("pattern"))
                .and_then(|pattern| pattern.as_str())
                .map(|pattern| format!("Searched files for '{}'", pattern)),
            "semantic_search" => parsed
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched '{}'", query)),
            "web_search" => parsed
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched web for '{}'", query)),
            "html_to_text" => parsed
                .and_then(|value| value.get("url"))
                .and_then(|url| url.as_str())
                .map(|url| format!("Fetched {}", url)),
            "TodoWrite" => Some(Self::describe_todo_write_call(parsed)),
            _ => None,
        }
    }

    fn describe_todo_write_call(parsed: Option<&serde_json::Value>) -> String {
        if let Some(todos) = parsed
            .and_then(|v| v.get("todos"))
            .and_then(|t| t.as_array())
        {
            if let Some(in_progress) = todos
                .iter()
                .find(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
            {
                let content = in_progress
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("task");
                let truncated = if content.len() > 40 {
                    format!("{}...", &content[..40])
                } else {
                    content.to_string()
                };
                return format!("Marked todo {} as in-progress", truncated);
            }
            if let Some(completed) = todos
                .iter()
                .find(|t| t.get("status").and_then(|s| s.as_str()) == Some("completed"))
            {
                let content = completed
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("task");
                let truncated = if content.len() > 40 {
                    format!("{}...", &content[..40])
                } else {
                    content.to_string()
                };
                return format!("Marked todo {} as completed", truncated);
            }
            if let Some(first) = todos.first() {
                let content = first
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("task");
                let truncated = if content.len() > 40 {
                    format!("{}...", &content[..40])
                } else {
                    content.to_string()
                };
                return format!("Created todo {}", truncated);
            }
        }
        "Updated todos".to_string()
    }

    pub(crate) fn handle_orchestrator_event_impl(&mut self, event: OrchestratorEvent) {
        self.has_orchestrator_activity = true;
        match event {
            OrchestratorEvent::StepStatusChanged {
                spec_id,
                spec_title,
                step_index,
                step_title,
                prefix,
                role,
                status,
            } => self.handle_step_status_changed(
                &spec_id,
                &spec_title,
                &step_index,
                &step_title,
                &prefix,
                role,
                status,
            ),
            OrchestratorEvent::SummaryUpdated { summary } => self.handle_summary_updated(summary),
            OrchestratorEvent::VerifierFailed { summary, feedback: _ } => {
                self.handle_verifier_failed(summary)
            }
            OrchestratorEvent::ChildSpecPushed {
                parent_step_index,
                child_spec_id: _,
                child_step_count,
            } => {
                self.status_message = Some(format!(
                    "Step {} split into {} sub-steps",
                    parent_step_index, child_step_count
                ));
            }
            OrchestratorEvent::ChannelClosed { .. } => {}
            OrchestratorEvent::Paused => {
                self.orchestrator_paused = true;
                self.status_message = Some("Orchestrator paused".to_string());
            }
            OrchestratorEvent::Resumed => {
                self.orchestrator_paused = false;
                self.status_message = Some("Orchestrator resumed".to_string());
            }
            OrchestratorEvent::Aborted => self.handle_orchestrator_stopped("Run aborted"),
            OrchestratorEvent::Completed => self.handle_orchestrator_stopped("Spec completed"),
            OrchestratorEvent::Error(error) => {
                self.orchestration_in_progress = false;
                self.messages.push(format!("[SPEC ERROR] {}", error));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(std::time::SystemTime::now());
            }
            OrchestratorEvent::ToolCallStarted {
                prefix,
                tool_name,
                arguments,
            } => self.handle_tool_call_started(prefix, tool_name, arguments),
            OrchestratorEvent::ToolCallCompleted {
                prefix,
                tool_name: _,
                result: _,
                is_error,
            } => self.handle_tool_call_completed(prefix, is_error),
            OrchestratorEvent::AgentMessage { prefix, message } => {
                self.handle_sub_agent_message(prefix, message)
            }
            OrchestratorEvent::StepCancelled { prefix } => {
                if let Some(context) = self.sub_agent_contexts.get_mut(&prefix) {
                    context.ensure_generation_stats_marker();
                }
                self.status_message = Some(format!("Step {} cancelled", prefix));
            }
        }
    }

    fn handle_step_status_changed(
        &mut self,
        spec_id: &str,
        spec_title: &str,
        step_index: &str,
        step_title: &str,
        prefix: &str,
        role: agent_core::orchestrator::StepRole,
        status: StepStatus,
    ) {
        if let Some(ref mut spec) = self.current_spec
            && spec.id == spec_id
            && let Some(step) = spec.steps.iter_mut().find(|s| s.index == step_index)
        {
            step.status = status;
        }

        self.sync_session_for_step(
            spec_id,
            spec_title,
            prefix,
            step_index,
            step_title,
            status,
            role,
        );
        self.update_active_step_prefix_for_status(prefix, status);
        self.status_message = Some(format!(
            "Step {}: {}",
            step_index,
            Self::status_label_for_step(status)
        ));
    }

    fn handle_summary_updated(&mut self, summary: agent_core::TaskSummary) {
        self.upsert_summary_history(summary.clone());
        let status_str = match summary.verification.status {
            VerificationStatus::Passed => "passed",
            VerificationStatus::Failed => "failed",
            VerificationStatus::Pending => "pending",
        };
        self.status_message = Some(format!("Step {} summary {}", summary.step_index, status_str));
    }

    fn handle_verifier_failed(&mut self, summary: agent_core::TaskSummary) {
        self.upsert_summary_history(summary.clone());
        self.status_message = Some(format!("Verifier failed on step {}", summary.step_index));
    }

    fn handle_orchestrator_stopped(&mut self, message: &str) {
        self.teardown_orchestrator_handles();
        self.reset_orchestrator_views();
        self.orchestration_in_progress = false;
        self.status_message = Some(message.to_string());
    }

    fn handle_tool_call_started(&mut self, prefix: String, tool_name: String, arguments: String) {
        let planned_label = if prefix.contains('.') {
            self.step_label_overrides.get(&prefix).cloned()
        } else {
            None
        };
        let label = planned_label.unwrap_or_else(|| Self::describe_tool_call(&tool_name, &arguments));
        let entry_id = self.next_tool_call_id;
        self.next_tool_call_id = self.next_tool_call_id.saturating_add(1);

        let session_metadata = self.orchestrator_sessions.get(&prefix);
        let role = session_metadata
            .map(|entry| entry.role.clone())
            .unwrap_or(SessionRole::Implementor);
        let worktree_branch = session_metadata.and_then(|entry| entry.worktree_branch.clone());
        let worktree_path = session_metadata.and_then(|entry| entry.worktree_path.clone());

        self.step_tool_calls
            .entry(prefix.clone())
            .or_default()
            .push(StepToolCallEntry {
                id: entry_id,
                label,
                status: ToolCallStatus::Started,
                role,
                worktree_branch,
                worktree_path,
            });
        self.active_tool_call = Some((prefix, entry_id));
    }

    fn handle_tool_call_completed(&mut self, prefix: String, is_error: bool) {
        if let Some((active_prefix, entry_id)) = self.active_tool_call.take()
            && active_prefix == prefix
            && let Some(entries) = self.step_tool_calls.get_mut(&prefix)
            && let Some(entry) = entries.iter_mut().find(|e| e.id == entry_id)
        {
            entry.status = if is_error {
                ToolCallStatus::Error
            } else {
                ToolCallStatus::Completed
            };
        }
    }

    fn handle_sub_agent_message(&mut self, prefix: String, message: SubAgentMessage) {
        let context = self
            .sub_agent_contexts
            .entry(prefix.clone())
            .or_insert_with(|| {
                let step_title = self
                    .current_spec
                    .as_ref()
                    .and_then(|spec| Self::find_step_by_prefix(&spec.steps, &prefix))
                    .map(|step| step.title.clone())
                    .unwrap_or_else(|| format!("Step {}", prefix));
                SubAgentContext::new(prefix.clone(), step_title)
            });

        match message {
            SubAgentMessage::UserPrompt { content } => {
                context.add_user_message(content);
            }
            SubAgentMessage::Text { content } => {
                context.add_agent_text(content);
            }
            SubAgentMessage::Thinking {
                content,
                duration_secs,
            } => {
                if duration_secs == 0 {
                    context.start_thinking(content);
                } else {
                    context.finish_thinking(duration_secs);
                }
            }
            SubAgentMessage::ToolCall {
                tool_name,
                arguments,
                result,
                is_error: _,
            } => {
                if let Some(result) = result {
                    let formatted_result = Self::format_tool_result(&tool_name, &result);
                    context.complete_tool_call(&tool_name, formatted_result);
                } else {
                    let formatted_args = Self::format_tool_arguments(&tool_name, &arguments);
                    context.add_tool_call_started(&tool_name, formatted_args);
                }
            }
            SubAgentMessage::GenerationStats {
                tokens_per_sec,
                input_tokens,
                output_tokens,
            } => {
                context.set_generation_stats(tokens_per_sec, input_tokens, output_tokens);
            }
            SubAgentMessage::Done => {
                let elapsed = context
                    .thinking_start_time
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(context.thinking_elapsed_secs);
                context.finish_thinking(elapsed);
            }
            SubAgentMessage::Error { message } => {
                context.add_agent_text(format!("[Error: {}]", message));
                let elapsed = context
                    .thinking_start_time
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(context.thinking_elapsed_secs);
                context.finish_thinking(elapsed);
            }
        }
    }
}
