use agent_core::orchestrator::OrchestratorEvent;
use color_eyre::Result;
use ratatui::{DefaultTerminal, crossterm::event};

use crate::app::App;

impl App {
    pub(crate) async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.exit {
            self.runtime_tick(&mut terminal).await?;
        }

        if self.persistence_state.save_pending {
            if let Err(_e) = self.save_conversation().await {
                // eprintln!("[ERROR] Failed to save conversation on exit: {}", e);
            }
        }

        Ok(())
    }

    async fn runtime_tick(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.update_animation();
        self.survey.update();
        self.reconcile_message_vectors();

        let outcome = self.drain_agent_rx();

        let orchestrator_events: Vec<OrchestratorEvent> =
            if let Some(rx) = &mut self.orchestrator_event_rx {
                let mut events = Vec::new();
                while let Ok(event) = rx.try_recv() {
                    events.push(event);
                }
                events
            } else {
                Vec::new()
            };
        for event in orchestrator_events {
            self.handle_orchestrator_event(event);
        }

        self.process_pending_actions(outcome).await;

        self.clear_startup_screen_if_ready(terminal)?;
        terminal.draw(|frame| self.draw(frame))?;

        let poll_duration = self.startup_poll_duration();
        if event::poll(poll_duration)? {
            let runtime_event = event::read()?;
            self.handle_runtime_event(runtime_event)?;
        }

        Ok(())
    }
}
