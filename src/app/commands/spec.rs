use crate::app::orchestrator::control::{
    MessageLog, SpecAgentBridge, SpecCliContext, SpecCliHandler, SpecCommandResult,
};
use crate::app::{App, MessageState, MessageType};

impl App {
    /// Handle /spec commands: /spec, /spec split <index>, /spec status, /spec abort
    pub(crate) async fn handle_spec_command(&mut self, command: &str) {
        let mut handler = SpecCliHandler::new(SpecCliContext {
            current_spec: &mut self.current_spec,
            orchestrator_control: self.orchestrator_control.as_ref(),
            orchestrator_history: &self.orchestrator_history,
            orchestrator_paused: &mut self.orchestrator_paused,
            status_message: &mut self.status_message,
            message_log: MessageLog {
                messages: &mut self.messages,
                types: &mut self.message_types,
                states: &mut self.message_states,
                metadata: &mut self.message_metadata,
                timestamps: &mut self.message_timestamps,
            },
        });

        let agent_ref = self
            .agent
            .as_deref()
            .map(|agent| agent as &(dyn SpecAgentBridge + Send + Sync));

        if let SpecCommandResult::Handled = handler.execute(agent_ref, command).await {
            return;
        }

        let parts: Vec<&str> = command.split_whitespace().collect();

        if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("/spec") {
            let path_or_goal = parts[1..].join(" ");
            if let Err(e) = self.load_spec(&path_or_goal) {
                self.messages.push(format!("Failed to load spec: {}", e));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(std::time::SystemTime::now());
            }
        } else {
            self.messages.push("[SPEC] Unknown spec command. Available: /spec, /spec split <index>, /spec status, /spec abort, /spec pause, /spec resume, /spec rerun, /spec history".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            self.message_metadata.push(None);
            self.message_timestamps.push(std::time::SystemTime::now());
        }
    }
}
