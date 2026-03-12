use ratatui::{Frame, layout::Layout, text::Text, widgets::Paragraph};

use crate::app::{App, Mode, Phase};

impl App {
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        self.draw_internal(frame, None);
    }

    pub(crate) fn draw_internal(
        &mut self,
        frame: &mut Frame,
        constrained_area: Option<ratatui::layout::Rect>,
    ) {
        if constrained_area.is_none()
            && let Some(prefix) = self.expanded_sub_agent.clone()
            && let Some(context) = self.sub_agent_contexts.get(&prefix)
        {
            self.render_sub_agent_fullscreen(frame, context.clone());
            return;
        }

        if self.mode == Mode::SessionWindow && constrained_area.is_none() {
            self.render_session_window_with_agent_ui(frame);
            return;
        }

        let render_area = constrained_area.unwrap_or_else(|| frame.area());
        let spec_tree_view_active = self.should_render_spec_tree(constrained_area);

        if let Some((_, flash_time)) = &self.flash_highlight
            && flash_time.elapsed().as_millis() >= 50
        {
            self.flash_highlight = None;
        }

        if let Some(press_time) = self.ctrl_c_pressed
            && press_time.elapsed().as_millis() >= 500
        {
            self.ctrl_c_pressed = None;
        }

        let constraints = self.startup_layout_constraints(render_area);
        let areas = Layout::vertical(constraints).split(render_area);
        self.render_startup_chrome(frame, &areas);

        let status_area = areas[areas.len() - 1];
        let has_queue_choice = self.show_queue_choice;
        let has_approval_prompt = self.safety_state.show_approval_prompt;
        let has_sandbox_prompt = self.safety_state.show_sandbox_prompt;
        let has_survey_or_thanks = self.survey.is_active() || self.survey.has_thank_you();
        let has_infobar = self.ctrl_c_pressed.is_some() || !self.queued_messages.is_empty();
        let has_autocomplete = self.autocomplete_active && self.mode == Mode::Normal;
        let area_indices = Self::compute_draw_area_indices(
            has_queue_choice,
            has_approval_prompt,
            has_sandbox_prompt,
            has_survey_or_thanks,
            has_infobar,
            has_autocomplete,
            self.show_background_tasks || self.viewing_task.is_some(),
            self.ui_state.show_help,
            self.ui_state.show_resume,
            self.show_history_panel,
            self.show_rewind,
            self.show_todos,
            self.show_model_selection,
        );
        let messages_area_idx = area_indices.messages_area_idx;
        let min_areas = area_indices.min_areas;

        let (mode, cursor_row, cursor_col, scroll_offset) = if self.phase == Phase::Input
            && areas.len() >= min_areas
        {
            if spec_tree_view_active
                || self.mode == Mode::Normal
                || self.mode == Mode::SessionWindow
            {
                (Mode::Normal, 0, 0, 0)
            } else {
                let cursor_row = self.editor.state.cursor.row;
                let cursor_col = self.editor.state.cursor.col;
                let messages_area = areas[messages_area_idx];
                let visible_lines = messages_area.height as usize;
                let max_width = messages_area.width.saturating_sub(4) as usize;
                let message_lines = self.compose_main_message_lines(max_width, true, false);

                let total_lines = message_lines.len();
                let scroll = if total_lines <= visible_lines {
                    0
                } else if cursor_row < visible_lines / 2 {
                    0
                } else if cursor_row >= total_lines.saturating_sub(visible_lines / 2) {
                    total_lines.saturating_sub(visible_lines)
                } else {
                    cursor_row.saturating_sub(visible_lines / 2)
                };
                (self.mode, cursor_row, cursor_col, scroll)
            }
        } else {
            (Mode::Normal, 0, 0, 0)
        };

        self.render_status_bar(
            frame,
            status_area,
            mode,
            cursor_row,
            cursor_col,
            scroll_offset,
        );

        if self.phase == Phase::Input && areas.len() >= min_areas {
            let messages_area = areas[messages_area_idx];
            let input_area = areas[area_indices.input_area_idx];
            if spec_tree_view_active
                || self.mode == Mode::Normal
                || self.mode == Mode::SessionWindow
            {
                let max_width = messages_area.width.saturating_sub(4) as usize;
                let append_plan = self.current_spec.is_some() && self.allow_plan_tree_render();
                let message_lines =
                    self.compose_main_message_lines(max_width, append_plan, true);

                let total_lines = message_lines.len();
                let visible_lines = messages_area.height as usize;
                let scroll_offset = if spec_tree_view_active {
                    0
                } else {
                    total_lines.saturating_sub(visible_lines)
                };
                let messages_widget =
                    Paragraph::new(Text::from(message_lines)).scroll((scroll_offset as u16, 0));
                frame.render_widget(messages_widget, messages_area);
                self.render_normal_input_area(frame, input_area);
            } else {
                self.render_navigation_mode_view(frame, messages_area, input_area);
            }

            self.render_input_top_right_indicator(frame, input_area);
            self.render_optional_draw_sections(frame, &areas, area_indices);
        }
    }
}
