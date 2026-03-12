use crate::app::{App, TodoItem};

/// Outcome flags produced by draining the agent message channel.
/// These are deferred effects that must be applied after releasing the channel borrow.
pub(crate) struct AgentStreamOutcome {
    pub process_queued: bool,
    pub process_interrupt: Option<String>,
    pub pending_todos: Option<Vec<TodoItem>>,
    pub create_rewind: bool,
    pub pending_file_change: Option<(String, String, String)>,
    pub check_auto_summarize: bool,
    pub trigger_mid_stream_auto_summarize: bool,
    pub schedule_resume_prompt: bool,
}

impl AgentStreamOutcome {
    pub(super) fn new() -> Self {
        Self {
            process_queued: false,
            process_interrupt: None,
            pending_todos: None,
            create_rewind: false,
            pending_file_change: None,
            check_auto_summarize: false,
            trigger_mid_stream_auto_summarize: false,
            schedule_resume_prompt: false,
        }
    }
}

impl App {
    /// Drain the agent message channel, updating UI state for each message.
    /// Returns deferred outcome flags that the caller must apply after this borrow ends.
    pub(crate) fn drain_agent_rx(&mut self) -> AgentStreamOutcome {
        super::agent_stream_handlers::drain_agent_rx_impl(self)
    }
}

#[cfg(test)]
mod tests {
    use super::AgentStreamOutcome;

    #[test]
    fn agent_stream_outcome_defaults_to_no_deferred_effects() {
        let outcome = AgentStreamOutcome::new();

        assert!(!outcome.process_queued);
        assert!(outcome.process_interrupt.is_none());
        assert!(outcome.pending_todos.is_none());
        assert!(!outcome.create_rewind);
        assert!(outcome.pending_file_change.is_none());
        assert!(!outcome.check_auto_summarize);
        assert!(!outcome.trigger_mid_stream_auto_summarize);
        assert!(!outcome.schedule_resume_prompt);
    }
}
