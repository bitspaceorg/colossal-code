use agent_core::{
    SpecSheet, SpecStep,
    orchestrator::{Orchestrator, OrchestratorAgent, OrchestratorEvent},
};
use color_eyre::Result;
use std::sync::Arc;
use tokio::{sync::mpsc, task};

use crate::app::App;

impl App {
    pub(crate) fn start_orchestrator_run(&mut self, spec: SpecSheet) -> Result<()> {
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

    /// Load a spec from a path or goal string and store it in app state.
    /// This should be called after App::new() when --spec flag is used.
    pub(crate) fn load_spec_impl(&mut self, path_or_goal: &str) -> Result<()> {
        if let Some(agent) = &self.agent {
            let spec = agent
                .create_spec_sheet(path_or_goal)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to create spec: {}", e))?;
            let orchestrator_spec = spec.clone();
            self.current_spec = Some(spec);
            self.rebuild_step_label_overrides();
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

    /// Find a step by its prefix in a nested step tree.
    pub(crate) fn find_step_by_prefix<'a>(
        steps: &'a [SpecStep],
        prefix: &str,
    ) -> Option<&'a SpecStep> {
        let parts: Vec<&str> = prefix.split('.').collect();
        Self::find_step_recursive(steps, &parts, 0)
    }

    pub(crate) fn find_step_recursive<'a>(
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
