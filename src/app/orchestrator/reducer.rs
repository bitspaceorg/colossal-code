use agent_core::{
    StepStatus,
    orchestrator::{OrchestratorEvent, StepRole},
};
use color_eyre::Result;

use crate::app::App;
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

    /// Rebuild the Alt+W session viewer from a chat's persisted subagent
    /// child conversations, so resumed chats can navigate into their
    /// sub-chats exactly like live ones.
    pub(crate) fn rehydrate_subagent_sessions(&mut self, parent_conversation_id: &str) {
        use crate::app::orchestrator::session_manager::{
            OrchestratorEntry, SessionRole, SessionStatus,
        };
        use crate::app::persistence::db::reader;
        use crate::app::{MessageType, SubAgentContext};

        let Ok(children) = reader::list_child_conversations(parent_conversation_id) else {
            return;
        };
        if children.is_empty() {
            return;
        }

        for (index, child) in children.iter().enumerate() {
            let Ok(Some(loaded)) = reader::load_conversation(&child.id) else {
                continue;
            };
            let title = child
                .title
                .clone()
                .unwrap_or_else(|| format!("Sub-chat {}", index + 1));
            // Child titles are "<prefix> · <step title>" (subagent_conversation_id).
            let (prefix, step_title) = match title.split_once(" · ") {
                Some((prefix, step_title)) => (prefix.to_string(), step_title.to_string()),
                None => (title.clone(), title),
            };

            let mut context = SubAgentContext::new(prefix.clone(), step_title.clone());
            for message in loaded.messages {
                match message.message_type {
                    MessageType::User => context.add_user_message(message.content),
                    _ => context.add_agent_text(message.content),
                }
            }
            self.sub_agent_contexts.insert(prefix.clone(), context);
            self.orchestrator_sessions.insert(
                prefix.clone(),
                OrchestratorEntry {
                    spec_id: String::new(),
                    spec_title: "restored".to_string(),
                    prefix,
                    step_title,
                    role: SessionRole::Implementor,
                    status: SessionStatus::Completed,
                    started_at: None,
                    completed_at: None,
                    worktree_branch: None,
                    worktree_path: None,
                },
            );
        }
        let snapshot: Vec<OrchestratorEntry> =
            self.orchestrator_sessions.values().cloned().collect();
        self.session_manager.update_from_orchestrator(snapshot);
    }

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
