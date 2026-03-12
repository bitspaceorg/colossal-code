use crate::app::App;
use agent_core::StepStatus;

impl App {
    pub(crate) fn update_active_step_prefix_for_status(
        &mut self,
        prefix: &str,
        status: StepStatus,
    ) {
        match status {
            StepStatus::InProgress => {
                self.active_step_prefix = Some(prefix.to_string());
            }
            _ => {
                if self.active_step_prefix.as_deref() == Some(prefix) {
                    self.active_step_prefix = None;
                }
            }
        }
    }

    pub(crate) fn status_label_for_step(status: StepStatus) -> &'static str {
        match status {
            StepStatus::Pending => "pending",
            StepStatus::InProgress => "in progress",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
        }
    }
}
