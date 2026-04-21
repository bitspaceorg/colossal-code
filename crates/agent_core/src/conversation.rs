use crate::Agent;
use crate::message_helpers::request_builder_from_serialized_messages;
use anyhow::Result;
use mistralrs::{RequestLike, TextMessageRole};
use serde_json::Value;

pub async fn clear_conversation(agent: &Agent) {
    let mut conversation_guard = agent.conversation.lock().await;
    *conversation_guard = None;
}

impl Agent {
    pub async fn clear_conversation(&self) {
        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = None;
    }

    pub async fn inject_summary_context(&self, summary: &str) {
        let tools = {
            let tools_guard = self.tools.lock().await;
            tools_guard.clone()
        };

        let system_prompt_content = {
            let system_prompt_guard = self.system_prompt.lock().await;
            system_prompt_guard.clone()
        };

        let system_msg = "You are Nite 3, a coding agent deployed in the best TUI colossal code. You live inside the terminal, running lean, fast, and sharp. Your role is to serve as the developer's right hand.";

        let full_context_msg = format!(
            "{}\n\n\
             This session is being continued from a previous conversation that ran out of context. \
             The previous conversation has been summarized below:\n\n{}",
            system_prompt_content, summary
        );

        use mistralrs::{RequestBuilder, TextMessageRole, ToolChoice};

        let request_builder = RequestBuilder::new()
            .add_message(TextMessageRole::System, system_msg)
            .add_message(TextMessageRole::User, &full_context_msg)
            .set_tools(tools)
            .set_tool_choice(ToolChoice::Auto)
            .enable_thinking(true);

        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(request_builder);
    }

    pub async fn restore_conversation(&self, messages_json: &str) -> Result<()> {
        let messages: Vec<Value> = serde_json::from_str(messages_json)?;
        self.restore_serialized_conversation(&messages, None).await
    }

    pub async fn restore_conversation_with_replaced_system_prompt(
        &self,
        messages_json: &str,
    ) -> Result<()> {
        let messages: Vec<Value> = serde_json::from_str(messages_json)?;
        let system_prompt = {
            let system_prompt_guard = self.system_prompt.lock().await;
            system_prompt_guard.clone()
        };
        self.restore_serialized_conversation(&messages, Some(system_prompt))
            .await
    }

    async fn restore_serialized_conversation(
        &self,
        messages: &[Value],
        replacement_system_prompt: Option<String>,
    ) -> Result<()> {
        let tools = {
            let tools_guard = self.tools.lock().await;
            tools_guard.clone()
        };
        let request_builder = request_builder_from_serialized_messages(
            messages,
            tools,
            replacement_system_prompt.as_deref(),
        )?;

        let mut conversation_guard = self.conversation.lock().await;
        *conversation_guard = Some(request_builder);

        Ok(())
    }

    pub async fn export_conversation(&self) -> Option<String> {
        let conversation_guard = self.conversation.lock().await;
        if let Some(request_builder) = conversation_guard.as_ref() {
            let messages = request_builder.messages_ref();
            if messages.is_empty() {
                None
            } else {
                serde_json::to_string_pretty(&messages).ok()
            }
        } else {
            None
        }
    }

    pub async fn inject_system_reminder(&self, reminder: &str) -> Result<()> {
        let mut conversation_guard = self.conversation.lock().await;
        if let Some(ref mut request_builder) = *conversation_guard {
            *request_builder = request_builder
                .clone()
                .add_message(TextMessageRole::System, reminder);
        }
        Ok(())
    }
}
