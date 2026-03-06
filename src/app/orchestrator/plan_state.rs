use agent_core::{SpecStep, TaskSummary};
use ratatui::{layout::Rect, text::Line};
use std::collections::HashMap;

use crate::app::orchestrator::plan_view as spec_ui;
use crate::app::render::thinking::create_thinking_highlight_spans;
use crate::App;

impl App {
    pub(crate) fn reset_orchestrator_views(&mut self) {
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

    pub(crate) fn compose_step_prefix(parent_prefix: &str, index: &str) -> String {
        if parent_prefix.is_empty() {
            index.to_string()
        } else {
            format!("{}.{}", parent_prefix, index)
        }
    }

    pub(crate) fn rebuild_step_label_overrides(&mut self) {
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

    pub(crate) fn collect_step_labels(
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

    pub(crate) fn teardown_orchestrator_handles(&mut self) {
        if let Some(handle) = self.orchestrator_task.take() {
            handle.abort();
        }
        self.orchestrator_control = None;
        self.orchestrator_event_rx = None;
        self.orchestrator_paused = false;
    }

    pub(crate) fn upsert_summary_history(&mut self, summary: TaskSummary) {
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

    pub(crate) fn sync_spec_history_metadata(&mut self) {
        if let Some(spec) = self.current_spec.as_mut()
            && let Ok(history_value) = serde_json::to_value(&self.orchestrator_history)
        {
            if !spec.metadata.is_object() {
                spec.metadata = serde_json::Value::Object(serde_json::Map::new());
            }
            if let Some(obj) = spec.metadata.as_object_mut() {
                obj.insert("history".to_string(), history_value);
            }
        }
    }

    /// Returns true when we should show the full-screen spec plan tree view.
    /// This is now only used for Alt+W session window's constrained area check.
    pub(crate) fn should_render_spec_tree(&self, _constrained_area: Option<Rect>) -> bool {
        false
    }

    pub(crate) fn allow_plan_tree_render(&self) -> bool {
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

    pub(crate) fn build_tool_only_plan_lines(&self, max_width: usize) -> Vec<Line<'static>> {
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

    pub(crate) fn orchestration_status_line(&self) -> Option<spec_ui::OrchestrationStatusLine> {
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

    pub(crate) fn append_tool_plan_view_lines(&self, lines: &mut Vec<Line<'_>>, max_width: usize) {
        if self.current_spec.is_none() || !self.allow_plan_tree_render() {
            return;
        }

        if cfg!(test) {
            let _ = self.build_spec_plan_lines(max_width);
        }

        let plan_lines = self.build_tool_only_plan_lines(max_width);
        spec_ui::compose_tool_plan_view_lines(lines, plan_lines, self.orchestration_status_line());
    }

    pub(crate) fn build_spec_plan_lines(&self, max_width: usize) -> Vec<Line<'static>> {
        if let Some(spec) = &self.current_spec {
            let selected_index = self.spec_pane_selected.min(spec.steps.len().saturating_sub(1));
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
}
