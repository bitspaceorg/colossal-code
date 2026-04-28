use crate::app::App;

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
    pub(crate) isolated_review_area_idx: Option<usize>,
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
        _has_infobar: bool,
        has_autocomplete: bool,
        has_background_tasks: bool,
        has_help_panel: bool,
        has_resume_panel: bool,
        has_history_panel: bool,
        has_rewind_panel: bool,
        has_isolated_review_panel: bool,
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
        let infobar_area_idx = Some(idx);
        idx += 1;
        let input_area_idx = idx;
        idx += 1;
        let autocomplete_area_idx = next_idx_if(has_autocomplete, &mut idx);
        let background_tasks_area_idx = next_idx_if(has_background_tasks, &mut idx);
        let help_area_idx = next_idx_if(has_help_panel, &mut idx);
        let resume_area_idx = next_idx_if(has_resume_panel, &mut idx);
        let history_panel_area_idx = next_idx_if(has_history_panel, &mut idx);
        let rewind_area_idx = next_idx_if(has_rewind_panel, &mut idx);
        let isolated_review_area_idx = next_idx_if(has_isolated_review_panel, &mut idx);
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
            isolated_review_area_idx,
            todos_area_idx,
            model_selection_area_idx,
            min_areas: idx + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DrawAreaIndices;
    use crate::app::App;

    #[test]
    fn compute_draw_area_indices_uses_baseline_layout() {
        let indices = App::compute_draw_area_indices(
            false, false, false, false, false, false, false, false, false, false, false, false,
            false, false,
        );

        assert_eq!(indices.messages_area_idx, 2);
        assert_eq!(indices.infobar_area_idx, Some(3));
        assert_eq!(indices.input_area_idx, 4);
        assert_eq!(indices.min_areas, 6);
        assert_eq!(indices.queue_choice_area_idx, None);
        assert_eq!(indices.approval_prompt_area_idx, None);
        assert_eq!(indices.sandbox_prompt_area_idx, None);
        assert_eq!(indices.survey_area_idx, None);
        assert_eq!(indices.autocomplete_area_idx, None);
        assert_eq!(indices.background_tasks_area_idx, None);
        assert_eq!(indices.help_area_idx, None);
        assert_eq!(indices.resume_area_idx, None);
        assert_eq!(indices.history_panel_area_idx, None);
        assert_eq!(indices.rewind_area_idx, None);
        assert_eq!(indices.isolated_review_area_idx, None);
        assert_eq!(indices.todos_area_idx, None);
        assert_eq!(indices.model_selection_area_idx, None);
    }

    #[test]
    fn compute_draw_area_indices_orders_optional_sections() {
        let indices = App::compute_draw_area_indices(
            true, true, true, true, true, true, true, true, true, true, true, true, true, true,
        );

        assert_eq!(indices.queue_choice_area_idx, Some(3));
        assert_eq!(indices.approval_prompt_area_idx, Some(4));
        assert_eq!(indices.sandbox_prompt_area_idx, Some(5));
        assert_eq!(indices.survey_area_idx, Some(6));
        assert_eq!(indices.infobar_area_idx, Some(7));
        assert_eq!(indices.input_area_idx, 8);
        assert_eq!(indices.autocomplete_area_idx, Some(9));
        assert_eq!(indices.background_tasks_area_idx, Some(10));
        assert_eq!(indices.help_area_idx, Some(11));
        assert_eq!(indices.resume_area_idx, Some(12));
        assert_eq!(indices.history_panel_area_idx, Some(13));
        assert_eq!(indices.rewind_area_idx, Some(14));
        assert_eq!(indices.isolated_review_area_idx, Some(15));
        assert_eq!(indices.todos_area_idx, Some(16));
        assert_eq!(indices.model_selection_area_idx, Some(17));
        assert_eq!(indices.min_areas, 19);
    }

    #[test]
    fn draw_area_indices_debug_mentions_key_fields() {
        let indices = DrawAreaIndices {
            messages_area_idx: 2,
            queue_choice_area_idx: Some(3),
            approval_prompt_area_idx: Some(4),
            sandbox_prompt_area_idx: None,
            survey_area_idx: None,
            infobar_area_idx: Some(5),
            input_area_idx: 6,
            autocomplete_area_idx: None,
            background_tasks_area_idx: None,
            help_area_idx: None,
            resume_area_idx: None,
            history_panel_area_idx: None,
            rewind_area_idx: None,
            isolated_review_area_idx: None,
            todos_area_idx: None,
            model_selection_area_idx: None,
            min_areas: 8,
        };

        let debug = format!("{:?}", indices);
        assert!(debug.contains("messages_area_idx"));
        assert!(debug.contains("input_area_idx"));
        assert!(debug.contains("queue_choice_area_idx"));
    }
}
