use std::time::SystemTime;

use agent_core::{
    Agent, SpecSheet, SpecStep, TaskSummary, orchestrator::OrchestratorControl,
};
use anyhow::Result;
use async_trait::async_trait;

use crate::app::{MessageState, MessageType, UIMessageMetadata};

use super::spec_command;
use super::spec_executor;

#[async_trait]
pub trait SpecAgentBridge: Send + Sync {
    async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet>;
    fn validate_step_index(&self, spec: &SpecSheet, index: &str) -> Result<()>;
    fn get_spec_status(&self, spec: &SpecSheet) -> Result<String>;
}

#[async_trait]
impl SpecAgentBridge for Agent {
    async fn request_split(&self, step: &SpecStep) -> Result<SpecSheet> {
        Agent::request_split(self, step).await
    }

    fn validate_step_index(&self, spec: &SpecSheet, index: &str) -> Result<()> {
        Agent::validate_step_index(self, spec, index)
    }

    fn get_spec_status(&self, spec: &SpecSheet) -> Result<String> {
        Agent::get_spec_status(self, spec)
    }
}

pub enum SpecCommandResult {
    Handled,
    Unhandled,
}

pub struct MessageLog<'a> {
    pub messages: &'a mut Vec<String>,
    pub types: &'a mut Vec<MessageType>,
    pub states: &'a mut Vec<MessageState>,
    pub metadata: &'a mut Vec<Option<UIMessageMetadata>>,
    pub timestamps: &'a mut Vec<SystemTime>,
}

impl<'a> MessageLog<'a> {
    pub fn push(&mut self, message: String) {
        self.messages.push(message);
        self.types.push(MessageType::Agent);
        self.states.push(MessageState::Sent);
        self.metadata.push(None);
        self.timestamps.push(SystemTime::now());
    }
}

pub struct SpecCliContext<'a> {
    pub current_spec: &'a mut Option<SpecSheet>,
    pub orchestrator_control: Option<&'a OrchestratorControl>,
    pub orchestrator_history: &'a [TaskSummary],
    pub orchestrator_paused: &'a mut bool,
    pub status_message: &'a mut Option<String>,
    pub message_log: MessageLog<'a>,
}

pub struct SpecCliHandler<'a> {
    ctx: SpecCliContext<'a>,
}

impl<'a> SpecCliHandler<'a> {
    pub fn new(ctx: SpecCliContext<'a>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &mut self,
        agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
        command: &str,
    ) -> SpecCommandResult {
        let Some(parsed) = spec_command::parse(command) else {
            return SpecCommandResult::Unhandled;
        };

        spec_executor::execute(&mut self.ctx, agent, parsed).await;
        SpecCommandResult::Handled
    }
}
