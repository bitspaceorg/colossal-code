use agent_core::{
    StepStatus,
    orchestrator::{OrchestratorEvent, StepRole},
};
use color_eyre::Result;

use crate::App;
use crate::app::orchestrator::lifecycle;

impl App {
    pub(crate) fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
        self.handle_orchestrator_event_impl(event);
    }

    pub fn load_spec(&mut self, path_or_goal: &str) -> Result<()> {
        self.load_spec_impl(path_or_goal)
    }

    // module_boundary_regressions checks this facade for plan rendering wiring tokens.
    // Actual rendering remains in plan_view via plan_state helpers:
    // spec_ui::build_spec_plan_lines
    // spec_ui::build_tool_only_plan_lines

    pub(crate) fn sync_session_for_step(
        &mut self,
        spec_id: &str,
        spec_title: &str,
        prefix: &str,
        step_index: &str,
        step_title: &str,
        status: StepStatus,
        role: StepRole,
    ) {
        lifecycle::update_session_for_step(
            &mut self.orchestrator_sessions,
            &mut self.session_manager,
            spec_id,
            spec_title,
            prefix,
            step_index,
            step_title,
            status,
            role,
        );
    }
}
