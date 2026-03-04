use agent_core::{SpecSheet, SpecStep, StepStatus, TaskSummary, VerificationStatus};
use agent_core::orchestrator::{
    Orchestrator, OrchestratorAgent, OrchestratorEvent, SubAgentMessage,
};
use color_eyre::Result;
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::{sync::mpsc, task};

use crate::session_lifecycle;
use crate::spec_ui;
use crate::ui::thinking::create_thinking_highlight_spans;
use crate::{App, MessageState, MessageType, SessionRole, StepToolCallEntry, SubAgentContext, ToolCallStatus};

impl App {
    pub(super) fn reset_orchestrator_views(&mut self) {
        self.orchestrator_history.clear();
        self.latest_summaries.clear();
        self.orchestrator_sessions.clear();
        self.session_manager.clear_orchestrator_entries();
        self.spec_pane_selected = 0;
        self.spec_pane_show_history = false;
        self.spec_step_drawer_open = false;
        self.show_history_panel = false;
        self.history_panel_selected = 0;
        self.step_tool_calls.clear();
        self.sub_agent_contexts.clear();
        self.expanded_sub_agent = None;
        self.expanded_sub_agent_before_alt_w = None;
        self.mode_before_sub_agent = None;
        self.has_orchestrator_activity = false;
        self.rendering_sub_agent_view = false;
        self.rendering_sub_agent_prefix = None;
        self.active_step_prefix = None;
        self.active_tool_call = None;
        self.next_tool_call_id = 0;
    }

    pub(super) fn compose_step_prefix(parent_prefix: &str, index: &str) -> String {
        if parent_prefix.is_empty() {
            index.to_string()
        } else {
            format!("{}.{}", parent_prefix, index)
        }
    }

    pub(super) fn rebuild_step_label_overrides(&mut self) {
        if let Some(spec) = &self.current_spec {
            let mut labels = HashMap::new();
            for step in &spec.steps {
                Self::collect_step_labels(step, "", &mut labels);
            }
            self.step_label_overrides = labels;
        } else {
            self.step_label_overrides.clear();
        }
    }

    pub(super) fn collect_step_labels(
        step: &SpecStep,
        parent_prefix: &str,
        labels: &mut HashMap<String, String>,
    ) {
        let prefix = Self::compose_step_prefix(parent_prefix, &step.index);
        let label = if step.instructions.is_empty() {
            step.title.clone()
        } else {
            format!("{} — {}", step.title, step.instructions)
        };
        labels.insert(prefix.clone(), label);
        if let Some(sub_spec) = &step.sub_spec {
            for child in &sub_spec.steps {
                Self::collect_step_labels(child, &prefix, labels);
            }
        }
    }

    pub(super) fn teardown_orchestrator_handles(&mut self) {
        if let Some(handle) = self.orchestrator_task.take() {
            handle.abort();
        }
        self.orchestrator_control = None;
        self.orchestrator_event_rx = None;
        self.orchestrator_paused = false;
    }

    pub(super) fn start_orchestrator_run(&mut self, spec: SpecSheet) -> Result<()> {
        self.teardown_orchestrator_handles();
        self.reset_orchestrator_views();

        let agent = self
            .agent
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Agent not initialized"))?
            .clone();
        let main_agent: Arc<dyn OrchestratorAgent> = agent.clone();
        let sub_agent_factory = {
            let sub_agent = agent.clone();
            Arc::new(
                move |_step: &SpecStep,
                      cwd: Option<std::path::PathBuf>|
                      -> Arc<dyn OrchestratorAgent> {
                    if let Some(path) = cwd {
                        Arc::new(sub_agent.with_working_directory(path))
                    } else {
                        sub_agent.clone()
                    }
                },
            )
        };

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (mut orchestrator, control) = Orchestrator::new_with_control(
            main_agent,
            sub_agent_factory,
            spec.clone(),
            event_tx.clone(),
        );
        self.orchestrator_control = Some(control);
        self.orchestrator_event_rx = Some(event_rx);

        self.orchestrator_task = Some(task::spawn(async move {
            match orchestrator.run().await {
                Ok(()) => {
                    let _ = event_tx.send(OrchestratorEvent::Completed);
                }
                Err(err) => {
                    eprintln!("[ORCHESTRATOR ERROR] {}", err);
                    let _ = event_tx.send(OrchestratorEvent::Error(err.to_string()));
                }
            }
        }));

        self.status_message = Some(format!("Started orchestration for {}", spec.title));
        Ok(())
    }

    pub(super) fn upsert_summary_history(&mut self, summary: TaskSummary) {
        self.latest_summaries
            .insert(summary.step_index.clone(), summary.clone());

        if let Some(position) = self
            .orchestrator_history
            .iter()
            .position(|existing| existing.task_id == summary.task_id)
        {
            self.orchestrator_history[position] = summary;
        } else {
            self.orchestrator_history.push(summary);
        }

        if self.history_panel_selected >= self.orchestrator_history.len() {
            self.history_panel_selected = self.orchestrator_history.len().saturating_sub(1);
        }

        self.sync_spec_history_metadata();
    }

    pub(super) fn sync_spec_history_metadata(&mut self) {
        if let Some(spec) = self.current_spec.as_mut() {
            if let Ok(history_value) = serde_json::to_value(&self.orchestrator_history) {
                if !spec.metadata.is_object() {
                    spec.metadata = serde_json::Value::Object(serde_json::Map::new());
                }
                if let Some(obj) = spec.metadata.as_object_mut() {
                    obj.insert("history".to_string(), history_value);
                }
            }
        }
    }

    pub(super) fn describe_exec_command(command: &str) -> String {
        Self::infer_search_label(command).unwrap_or_else(|| format!("Ran {}", command))
    }

    pub(super) fn infer_search_label(command: &str) -> Option<String> {
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
            if lower == "rg" || lower == "ripgrep" || lower == "grep" {
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
        }
        None
    }

    pub(super) fn should_skip_token(token: &str) -> bool {
        matches!(token, "&&" | "||" | "|" | ";") || token.starts_with('>') || token.starts_with('<')
    }

    /// Load a spec from a path or goal string and store it in app state.
    /// This should be called after App::new() when --spec flag is used.
    pub fn load_spec(&mut self, path_or_goal: &str) -> Result<()> {
        if let Some(agent) = &self.agent {
            let spec = agent
                .create_spec_sheet(path_or_goal)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to create spec: {}", e))?;
            let orchestrator_spec = spec.clone();
            self.current_spec = Some(spec);
            self.rebuild_step_label_overrides();
            // Don't auto-show spec pane - user can toggle with Shift+S
            // Tool activity is shown via compressed tool view in message stream
            self.spec_pane_selected = 0;
            self.step_tool_calls.clear();
            self.sub_agent_contexts.clear();
            self.expanded_sub_agent = None;
            self.expanded_sub_agent_before_alt_w = None;
            self.mode_before_sub_agent = None;
            self.has_orchestrator_activity = false;
            self.rendering_sub_agent_view = false;
            self.rendering_sub_agent_prefix = None;
            self.active_step_prefix = None;
            self.active_tool_call = None;
            self.next_tool_call_id = 0;
            self.start_orchestrator_run(orchestrator_spec)?;
            Ok(())
        } else {
            Err(color_eyre::eyre::eyre!("Agent not initialized"))
        }
    }

    pub(super) fn format_tool_arguments(_tool_name: &str, arguments_json: &str) -> String {
        // Parse JSON and format all parameters
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments_json) {
            if let Some(obj) = args.as_object() {
                let mut parts = Vec::new();

                // Add all arguments in order
                for (k, v) in obj.iter() {
                    let val_str = match v {
                        serde_json::Value::String(s) => {
                            // Truncate very long strings (using char-based slicing for UTF-8 safety)
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

                if parts.is_empty() {
                    return "".to_string();
                }
                return parts.join(", ");
            }
        }
        "".to_string()
    }

    pub(super) fn format_tool_result(tool_name: &str, result_yaml: &str) -> String {
        // Try parsing as YAML first
        if let Ok(result) = serde_yaml::from_str::<serde_yaml::Value>(result_yaml) {
            if let Some(obj) = result.as_mapping() {
                // Check status
                let status = obj
                    .get(serde_yaml::Value::String("status".to_string()))
                    .and_then(|v| v.as_str());

                if status == Some("Success") {
                    // Extract specific info based on tool
                    match tool_name {
                        "read_file" => {
                            if let Some(content) = obj
                                .get(serde_yaml::Value::String("content".to_string()))
                                .and_then(|v| v.as_str())
                            {
                                let lines = content.lines().count();
                                let chars = content.chars().count();
                                return format!("Read {} lines ({} chars)", lines, chars);
                            }
                        }
                        "get_files" | "get_files_recursive" => {
                            if let Some(files) = obj
                                .get(serde_yaml::Value::String("files".to_string()))
                                .and_then(|v| v.as_sequence())
                            {
                                if files.is_empty() {
                                    return "No files found".to_string();
                                }
                                // Show first few files
                                let file_names: Vec<String> = files
                                    .iter()
                                    .take(3)
                                    .filter_map(|f| f.as_str())
                                    .map(|s| s.to_string())
                                    .collect();
                                if files.len() > 3 {
                                    return format!(
                                        "Found {} files ({}... +{})",
                                        files.len(),
                                        file_names.join(", "),
                                        files.len() - 3
                                    );
                                } else {
                                    return format!(
                                        "Found {} files ({})",
                                        files.len(),
                                        file_names.join(", ")
                                    );
                                }
                            }
                        }
                        "search_files_with_regex" | "grep" => {
                            if let Some(results) = obj
                                .get(serde_yaml::Value::String("results".to_string()))
                                .and_then(|v| v.as_sequence())
                            {
                                if results.is_empty() {
                                    return "No matches found".to_string();
                                }
                                return format!(
                                    "Found {} matches in {} files",
                                    results.len(),
                                    results.iter().filter_map(|r| r.get("file")).count().max(1)
                                );
                            }
                        }
                        "exec_command" => {
                            if let Some(cmd_out) = obj
                                .get(serde_yaml::Value::String("cmd_out".to_string()))
                                .and_then(|v| v.as_str())
                            {
                                let lines = cmd_out.lines().count();
                                // Show first line of output if available
                                if let Some(first_line) = cmd_out.lines().next() {
                                    let preview = if first_line.len() > 50 {
                                        format!("{}...", &first_line[..47])
                                    } else {
                                        first_line.to_string()
                                    };
                                    return format!("{} lines: {}", lines, preview);
                                }
                                return format!("{} lines of output", lines);
                            }
                        }
                        "write_file" => {
                            return "File written successfully".to_string();
                        }
                        _ => return "Success".to_string(),
                    }
                } else if status == Some("Background") {
                    // Background command - show session info
                    if let Some(session_id) = obj
                        .get(serde_yaml::Value::String("session_id".to_string()))
                        .and_then(|v| v.as_str())
                    {
                        return format!("Started in background (session {})", session_id);
                    }
                    return "Started in background".to_string();
                } else if status == Some("orchestration_requested") {
                    // Orchestration tool - don't show inline message
                    // The "Started orchestration..." message is added by orchestration_pending handler
                    return String::new();
                } else if let Some(_err_status) = status {
                    // Get error message
                    if let Some(msg) = obj
                        .get(serde_yaml::Value::String("message".to_string()))
                        .and_then(|v| v.as_str())
                    {
                        // Skip YAML artifacts
                        if !msg.is_empty() && msg != "|+" && msg != "|-" && msg != "|" {
                            return format!("Error: {}", msg);
                        }
                    }
                    return "Failed".to_string();
                }
            }
        }

        // Fallback: try to extract first meaningful line
        let mut skip_yaml_keys = true;
        for line in result_yaml.lines() {
            let trimmed = line.trim();

            // Skip empty lines
            if trimmed.is_empty() {
                continue;
            }

            // Skip YAML document markers and field names
            if trimmed.starts_with("---")
                || trimmed.starts_with("status:")
                || trimmed.starts_with("path:")
                || trimmed.starts_with("message:")
            {
                continue;
            }

            // Skip YAML multiline string indicators
            if trimmed == "|+" || trimmed == "|-" || trimmed == "|" || trimmed == ">" {
                skip_yaml_keys = false; // Next non-empty line is the actual content
                continue;
            }

            // Return first meaningful content line
            if !skip_yaml_keys {
                if trimmed.len() > 60 {
                    return format!("{}...", &trimmed[..57]);
                }
                return trimmed.to_string();
            }
        }

        "Completed".to_string()
    }

    /// Returns true when we should show the full-screen spec plan tree view.
    /// This is now only used for Alt+W session window's constrained area check.
    pub(super) fn should_render_spec_tree(&self, _constrained_area: Option<Rect>) -> bool {
        // No longer used to replace messages - plan tree is now integrated into message stream
        false
    }

    pub(super) fn allow_plan_tree_render(&self) -> bool {
        if !self.rendering_sub_agent_view {
            return true;
        }

        if let Some(prefix) = &self.rendering_sub_agent_prefix {
            return self
                .sub_agent_contexts
                .get(prefix)
                .map(|ctx| ctx.started_orchestration)
                .unwrap_or(false);
        }

        false
    }

    /// Build tool-only plan lines for integration into message stream.
    /// Shows plan steps with tool calls underneath, no metadata.
    pub(super) fn build_tool_only_plan_lines(&self, max_width: usize) -> Vec<Line<'static>> {
        if let Some(spec) = &self.current_spec {
            return spec_ui::build_tool_only_plan_lines(
                spec,
                &self.step_tool_calls,
                self.active_step_prefix.as_deref(),
                max_width,
            );
        }
        Vec::new()
    }

    pub(super) fn orchestration_status_line(&self) -> Option<spec_ui::OrchestrationStatusLine> {
        if !self.orchestration_in_progress {
            return None;
        }

        let current_frame = self.thinking_snowflake_frames[self.get_thinking_loader_frame()];
        let text_with_dots = format!("{}...", self.get_thinking_current_word());
        let color_spans =
            create_thinking_highlight_spans(&text_with_dots, self.get_thinking_position());
        let elapsed_secs = self
            .thinking_start_time
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        Some(spec_ui::OrchestrationStatusLine {
            current_frame: current_frame.to_string(),
            color_spans,
            elapsed_secs,
        })
    }

    pub(super) fn append_tool_plan_view_lines(&self, lines: &mut Vec<Line<'_>>, max_width: usize) {
        if self.current_spec.is_none() || !self.allow_plan_tree_render() {
            return;
        }

        if cfg!(test) {
            let _ = self.build_spec_plan_lines(max_width);
        }

        let plan_lines = self.build_tool_only_plan_lines(max_width);
        spec_ui::compose_tool_plan_view_lines(lines, plan_lines, self.orchestration_status_line());
    }

    pub(super) fn build_spec_plan_lines(&self, max_width: usize) -> Vec<Line<'static>> {
        if let Some(spec) = &self.current_spec {
            let selected_index = self
                .spec_pane_selected
                .min(spec.steps.len().saturating_sub(1));
            return spec_ui::build_spec_plan_lines(
                spec,
                spec_ui::SpecPlanRenderParams {
                    orchestrator_paused: self.orchestrator_paused,
                    selected_index,
                    show_history: self.spec_pane_show_history,
                    step_drawer_open: false,
                    orchestrator_history: &self.orchestrator_history,
                    latest_summaries: &self.latest_summaries,
                    step_tool_calls: &self.step_tool_calls,
                    active_prefix: self.active_step_prefix.as_deref(),
                    include_metadata: false,
                    max_width,
                },
            );
        }
        Vec::new()
    }

    pub(super) fn describe_tool_call(tool_name: &str, arguments_json: &str) -> String {
        let parsed = serde_json::from_str::<serde_json::Value>(arguments_json).ok();
        let friendly = |name: &str| name.replace('_', " ");
        match tool_name {
            "exec_command" => parsed
                .as_ref()
                .and_then(|value| value.get("command"))
                .and_then(|command| command.as_str())
                .map(Self::describe_exec_command)
                .unwrap_or_else(|| "Run shell command".to_string()),
            "read_file" => parsed
                .as_ref()
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Read {}", path))
                .unwrap_or_else(|| "Read file".to_string()),
            "edit_file" => parsed
                .as_ref()
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Edited {}", path))
                .unwrap_or_else(|| "Edited file".to_string()),
            "delete_path" => parsed
                .as_ref()
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {}", path))
                .unwrap_or_else(|| "Deleted path".to_string()),
            "delete_many" => parsed
                .as_ref()
                .and_then(|value| value.get("paths"))
                .and_then(|paths| paths.as_array())
                .and_then(|paths| paths.first())
                .and_then(|path| path.as_str())
                .map(|path| format!("Deleted {} and more", path))
                .unwrap_or_else(|| "Deleted paths".to_string()),
            "get_files" | "get_files_recursive" => parsed
                .as_ref()
                .and_then(|value| value.get("path"))
                .and_then(|path| path.as_str())
                .map(|path| format!("Listed {}", path))
                .unwrap_or_else(|| "Listed files".to_string()),
            "search_files_with_regex" => parsed
                .as_ref()
                .and_then(|value| value.get("pattern"))
                .and_then(|pattern| pattern.as_str())
                .map(|pattern| format!("Searched files for '{}'", pattern))
                .unwrap_or_else(|| "Searched files".to_string()),
            "semantic_search" => parsed
                .as_ref()
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched '{}'", query))
                .unwrap_or_else(|| "Searched".to_string()),
            "web_search" => parsed
                .as_ref()
                .and_then(|value| value.get("query"))
                .and_then(|query| query.as_str())
                .map(|query| format!("Searched web for '{}'", query))
                .unwrap_or_else(|| "Searched web".to_string()),
            "html_to_text" => parsed
                .as_ref()
                .and_then(|value| value.get("url"))
                .and_then(|url| url.as_str())
                .map(|url| format!("Fetched {}", url))
                .unwrap_or_else(|| "Fetched URL".to_string()),
            "TodoWrite" => {
                // Describe the todo action based on what's being done
                if let Some(todos) = parsed
                    .as_ref()
                    .and_then(|v| v.get("todos"))
                    .and_then(|t| t.as_array())
                {
                    // Find the most relevant action to describe
                    // Priority: in_progress > completed > pending (for new todos)
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
            _ => {
                let formatted = Self::format_tool_arguments(tool_name, arguments_json);
                if formatted.is_empty() {
                    friendly(tool_name)
                } else {
                    format!("{} ({})", friendly(tool_name), formatted)
                }
            }
        }
    }

    /// Handle orchestrator events and update TUI state accordingly.
    pub(super) fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
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
            } => {
                if let Some(ref mut spec) = self.current_spec {
                    if spec.id == spec_id {
                        if let Some(step) = spec.steps.iter_mut().find(|s| s.index == step_index) {
                            step.status = status;
                        }
                    }
                }

                session_lifecycle::update_session_for_step(
                    &mut self.orchestrator_sessions,
                    &mut self.session_manager,
                    &spec_id,
                    &spec_title,
                    &prefix,
                    &step_index,
                    &step_title,
                    status,
                    role,
                );

                match status {
                    StepStatus::InProgress => {
                        self.active_step_prefix = Some(prefix.clone());
                    }
                    _ => {
                        if self.active_step_prefix.as_deref() == Some(prefix.as_str()) {
                            self.active_step_prefix = None;
                        }
                    }
                }

                let status_str = match status {
                    StepStatus::Pending => "pending",
                    StepStatus::InProgress => "in progress",
                    StepStatus::Completed => "completed",
                    StepStatus::Failed => "failed",
                };
                self.status_message = Some(format!("Step {}: {}", step_index, status_str));
            }

            OrchestratorEvent::SummaryUpdated { summary } => {
                self.upsert_summary_history(summary.clone());
                let status_str = match summary.verification.status {
                    VerificationStatus::Passed => "passed",
                    VerificationStatus::Failed => "failed",
                    VerificationStatus::Pending => "pending",
                };
                self.status_message = Some(format!(
                    "Step {} summary {}",
                    summary.step_index, status_str
                ));
            }

            OrchestratorEvent::VerifierFailed {
                summary,
                feedback: _,
            } => {
                self.upsert_summary_history(summary.clone());
                // Don't add to messages - visible in Alt+W session view
                self.status_message =
                    Some(format!("Verifier failed on step {}", summary.step_index));
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

            OrchestratorEvent::ChannelClosed {
                task_id: _,
                closed_at: _,
            } => {
                // Channel closed events are internal - don't show in main view
            }

            OrchestratorEvent::Paused => {
                self.orchestrator_paused = true;
                self.status_message = Some("Orchestrator paused".to_string());
            }

            OrchestratorEvent::Resumed => {
                self.orchestrator_paused = false;
                self.status_message = Some("Orchestrator resumed".to_string());
            }

            OrchestratorEvent::Aborted => {
                self.teardown_orchestrator_handles();
                self.reset_orchestrator_views();
                self.orchestration_in_progress = false;
                // Status message for internal state tracking only - not rendered in main view
                self.status_message = Some("Run aborted".to_string());
            }

            OrchestratorEvent::Completed => {
                self.teardown_orchestrator_handles();
                self.reset_orchestrator_views();
                self.orchestration_in_progress = false;
                // Status message for internal state tracking only - not rendered in main view
                self.status_message = Some("Spec completed".to_string());
            }

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
            } => {
                // Create tool call entry for this step
                let planned_label = if prefix.contains('.') {
                    self.step_label_overrides.get(&prefix).cloned()
                } else {
                    None
                };
                let label = planned_label
                    .unwrap_or_else(|| Self::describe_tool_call(&tool_name, &arguments));
                let entry_id = self.next_tool_call_id;
                self.next_tool_call_id = self.next_tool_call_id.saturating_add(1);

                let session_metadata = self.orchestrator_sessions.get(&prefix);
                let role = session_metadata
                    .map(|entry| entry.role.clone())
                    .unwrap_or(SessionRole::Implementor);
                let worktree_branch =
                    session_metadata.and_then(|entry| entry.worktree_branch.clone());
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

            OrchestratorEvent::ToolCallCompleted {
                prefix,
                tool_name: _,
                result: _,
                is_error,
            } => {
                // Find and update the matching tool call entry
                if let Some((active_prefix, entry_id)) = self.active_tool_call.take() {
                    if active_prefix == prefix {
                        if let Some(entries) = self.step_tool_calls.get_mut(&prefix) {
                            if let Some(entry) = entries.iter_mut().find(|e| e.id == entry_id) {
                                entry.status = if is_error {
                                    ToolCallStatus::Error
                                } else {
                                    ToolCallStatus::Completed
                                };
                            }
                        }
                    }
                }
            }

            OrchestratorEvent::AgentMessage { prefix, message } => {
                // Store sub-agent message in the context for this prefix
                let context = self
                    .sub_agent_contexts
                    .entry(prefix.clone())
                    .or_insert_with(|| {
                        // Get step title from current spec if available
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
                            let formatted_args =
                                Self::format_tool_arguments(&tool_name, &arguments);
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

            OrchestratorEvent::StepCancelled { prefix } => {
                if let Some(context) = self.sub_agent_contexts.get_mut(&prefix) {
                    context.ensure_generation_stats_marker();
                }
                self.status_message = Some(format!("Step {} cancelled", prefix));
            }
        }
    }

    /// Find a step by its prefix in a nested step tree.
    pub(super) fn find_step_by_prefix<'a>(
        steps: &'a [SpecStep],
        prefix: &str,
    ) -> Option<&'a SpecStep> {
        let parts: Vec<&str> = prefix.split('.').collect();
        Self::find_step_recursive(steps, &parts, 0)
    }

    pub(super) fn find_step_recursive<'a>(
        steps: &'a [SpecStep],
        parts: &[&str],
        depth: usize,
    ) -> Option<&'a SpecStep> {
        if depth >= parts.len() {
            return None;
        }
        let target_index = parts[depth];
        for step in steps {
            if step.index == target_index {
                if depth == parts.len() - 1 {
                    return Some(step);
                } else if let Some(sub_spec) = &step.sub_spec {
                    return Self::find_step_recursive(&sub_spec.steps, parts, depth + 1);
                }
            }
        }
        None
    }
}
