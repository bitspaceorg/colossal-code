use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{App, Mode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrawAreaIndices {
    pub(crate) messages_area_idx: usize,
    pub(crate) queue_choice_area_idx: Option<usize>,
    pub(crate) approval_prompt_area_idx: Option<usize>,
    pub(crate) sandbox_prompt_area_idx: Option<usize>,
    pub(crate) survey_area_idx: Option<usize>,
    pub(crate) infobar_area_idx: Option<usize>,
    pub(crate) input_area_idx: usize,
    pub(crate) autocomplete_area_idx: Option<usize>,
    pub(crate) background_tasks_area_idx: Option<usize>,
    pub(crate) help_area_idx: Option<usize>,
    pub(crate) resume_area_idx: Option<usize>,
    pub(crate) history_panel_area_idx: Option<usize>,
    pub(crate) rewind_area_idx: Option<usize>,
    pub(crate) todos_area_idx: Option<usize>,
    pub(crate) model_selection_area_idx: Option<usize>,
    pub(crate) min_areas: usize,
}

impl App {
    pub(crate) fn compute_draw_area_indices(
        has_queue_choice: bool,
        has_approval_prompt: bool,
        has_sandbox_prompt: bool,
        has_survey_or_thanks: bool,
        has_infobar: bool,
        has_autocomplete: bool,
        has_background_tasks: bool,
        has_help_panel: bool,
        has_resume_panel: bool,
        has_history_panel: bool,
        has_rewind_panel: bool,
        has_todos_panel: bool,
        has_model_selection_panel: bool,
    ) -> DrawAreaIndices {
        fn next_idx_if(enabled: bool, idx: &mut usize) -> Option<usize> {
            if enabled {
                let value = *idx;
                *idx += 1;
                Some(value)
            } else {
                None
            }
        }

        let messages_area_idx = 2;
        let mut idx = messages_area_idx + 1;
        let queue_choice_area_idx = next_idx_if(has_queue_choice, &mut idx);
        let approval_prompt_area_idx = next_idx_if(has_approval_prompt, &mut idx);
        let sandbox_prompt_area_idx = next_idx_if(has_sandbox_prompt, &mut idx);
        let survey_area_idx = next_idx_if(has_survey_or_thanks, &mut idx);
        let infobar_area_idx = next_idx_if(has_infobar, &mut idx);
        let input_area_idx = idx;
        idx += 1;
        let autocomplete_area_idx = next_idx_if(has_autocomplete, &mut idx);
        let background_tasks_area_idx = next_idx_if(has_background_tasks, &mut idx);
        let help_area_idx = next_idx_if(has_help_panel, &mut idx);
        let resume_area_idx = next_idx_if(has_resume_panel, &mut idx);
        let history_panel_area_idx = next_idx_if(has_history_panel, &mut idx);
        let rewind_area_idx = next_idx_if(has_rewind_panel, &mut idx);
        let todos_area_idx = next_idx_if(has_todos_panel, &mut idx);
        let model_selection_area_idx = next_idx_if(has_model_selection_panel, &mut idx);

        DrawAreaIndices {
            messages_area_idx,
            queue_choice_area_idx,
            approval_prompt_area_idx,
            sandbox_prompt_area_idx,
            survey_area_idx,
            infobar_area_idx,
            input_area_idx,
            autocomplete_area_idx,
            background_tasks_area_idx,
            help_area_idx,
            resume_area_idx,
            history_panel_area_idx,
            rewind_area_idx,
            todos_area_idx,
            model_selection_area_idx,
            min_areas: idx + 1,
        }
    }

    pub(crate) fn render_input_top_right_indicator(&self, frame: &mut Frame, input_area: Rect) {
        let indicator_y = input_area.y.saturating_sub(1);

        if (self.mode == Mode::Navigation || self.mode == Mode::Search)
            && !self.editor.state.search_matches().is_empty()
        {
            let num_results = self.editor.state.search_matches().len();
            let cursor_pos = self.editor.state.cursor;
            let current_line = cursor_pos.row + 1;
            let total_lines = self.editor.state.lines.len();
            let search_info = format!("{} results [{}/{}]", num_results, current_line, total_lines);

            self.render_indicator_text(frame, input_area, indicator_y, &search_info, Color::Cyan);
            return;
        }

        if let Some((mode_text, mode_color)) = self.safety_state.assistant_mode.to_display() {
            let cycle_hint = "(shift + tab to cycle)";
            let full_text = format!("{} {}", mode_text, cycle_hint);
            let total_width = full_text.len() as u16;
            let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);

            self.render_indicator_text_at(frame, start_x, indicator_y, &mode_text, mode_color);
            let cycle_start_x = start_x + mode_text.len() as u16;
            self.render_indicator_text_at(
                frame,
                cycle_start_x,
                indicator_y,
                &format!(" {}", cycle_hint),
                Color::DarkGray,
            );
        }
    }

    fn render_indicator_text(
        &self,
        frame: &mut Frame,
        input_area: Rect,
        y: u16,
        text: &str,
        color: Color,
    ) {
        let total_width = text.len() as u16;
        let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);
        self.render_indicator_text_at(frame, start_x, y, text, color);
    }

    fn render_indicator_text_at(
        &self,
        frame: &mut Frame,
        start_x: u16,
        y: u16,
        text: &str,
        color: Color,
    ) {
        let frame_area = frame.area();
        let mut current_x = start_x;
        for ch in text.chars() {
            if current_x < frame_area.width && y < frame_area.height {
                if let Some(cell) = frame.buffer_mut().cell_mut((current_x, y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(color));
                }
                current_x += 1;
            }
        }
    }

    pub(crate) fn render_optional_draw_sections(
        &mut self,
        frame: &mut Frame,
        areas: &[Rect],
        area_indices: DrawAreaIndices,
    ) {
        if let Some(idx) = area_indices.queue_choice_area_idx {
            let queue_widget = Paragraph::new(self.render_queue_choice_popup());
            frame.render_widget(queue_widget, areas[idx]);
        }

        if let Some(idx) = area_indices.approval_prompt_area_idx {
            frame.render_widget(
                Paragraph::new(self.render_approval_prompt_lines()),
                areas[idx],
            );
        }

        if let Some(idx) = area_indices.sandbox_prompt_area_idx {
            frame.render_widget(
                Paragraph::new(self.render_sandbox_prompt_lines()),
                areas[idx],
            );
        }

        if let Some(idx) = area_indices.survey_area_idx {
            frame.render_widget(Paragraph::new(self.survey.render()), areas[idx]);
        }

        if let Some(idx) = area_indices.infobar_area_idx {
            let infobar_text = if !self.queued_messages.is_empty() {
                let count = self.queued_messages.len();
                let plural = if count == 1 { "message" } else { "messages" };
                format!(
                    "{} {} in queue • ↑ to edit • Ctrl+C to cancel",
                    count, plural
                )
            } else if self.ctrl_c_pressed.is_some() {
                "Press Ctrl+C again to quit".to_string()
            } else {
                String::new()
            };
            let infobar_widget = Paragraph::new(Line::from(Span::styled(
                infobar_text,
                Style::default().fg(Color::Rgb(172, 172, 212)),
            )));
            frame.render_widget(infobar_widget, areas[idx]);
        }

        if let Some(idx) = area_indices.autocomplete_area_idx {
            self.render_autocomplete(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.background_tasks_area_idx {
            if self.viewing_task.is_some() {
                self.render_task_viewer(frame, areas[idx]);
            } else {
                self.render_background_tasks(frame, areas[idx]);
            }
        }

        if let Some(idx) = area_indices.help_area_idx {
            self.render_help_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.resume_area_idx {
            self.render_resume_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.history_panel_area_idx {
            self.render_history_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.rewind_area_idx {
            self.render_rewind_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.todos_area_idx {
            self.render_todos_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.model_selection_area_idx {
            self.render_model_selection_panel(frame, areas[idx]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DrawAreaIndices;
    use crate::App;

    #[test]
    fn compute_draw_area_indices_uses_baseline_layout() {
        let indices = App::compute_draw_area_indices(
            false, false, false, false, false, false, false, false, false, false, false, false,
            false,
        );

        assert_eq!(
            indices,
            DrawAreaIndices {
                messages_area_idx: 2,
                queue_choice_area_idx: None,
                approval_prompt_area_idx: None,
                sandbox_prompt_area_idx: None,
                survey_area_idx: None,
                infobar_area_idx: None,
                input_area_idx: 3,
                autocomplete_area_idx: None,
                background_tasks_area_idx: None,
                help_area_idx: None,
                resume_area_idx: None,
                history_panel_area_idx: None,
                rewind_area_idx: None,
                todos_area_idx: None,
                model_selection_area_idx: None,
                min_areas: 5,
            }
        );
    }

    #[test]
    fn compute_draw_area_indices_advances_for_enabled_sections() {
        let indices = App::compute_draw_area_indices(
            true, true, false, true, true, true, false, true, false, true, false, true, true,
        );

        assert_eq!(indices.messages_area_idx, 2);
        assert_eq!(indices.queue_choice_area_idx, Some(3));
        assert_eq!(indices.approval_prompt_area_idx, Some(4));
        assert_eq!(indices.sandbox_prompt_area_idx, None);
        assert_eq!(indices.survey_area_idx, Some(5));
        assert_eq!(indices.infobar_area_idx, Some(6));
        assert_eq!(indices.input_area_idx, 7);
        assert_eq!(indices.autocomplete_area_idx, Some(8));
        assert_eq!(indices.background_tasks_area_idx, None);
        assert_eq!(indices.help_area_idx, Some(9));
        assert_eq!(indices.resume_area_idx, None);
        assert_eq!(indices.history_panel_area_idx, Some(10));
        assert_eq!(indices.rewind_area_idx, None);
        assert_eq!(indices.todos_area_idx, Some(11));
        assert_eq!(indices.model_selection_area_idx, Some(12));
        assert_eq!(indices.min_areas, 14);
    }
}
