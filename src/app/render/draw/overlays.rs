use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::App;

use super::navigation::DrawAreaIndices;

impl App {
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

        if let Some(idx) = area_indices.isolated_review_area_idx {
            self.render_isolated_changes_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.todos_area_idx {
            self.render_todos_panel(frame, areas[idx]);
        }

        if let Some(idx) = area_indices.model_selection_area_idx {
            self.render_model_selection_panel(frame, areas[idx]);
        }

        self.render_connect_modal(frame);
    }
}
