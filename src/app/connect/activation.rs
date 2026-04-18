use color_eyre::Result;

use std::sync::Arc;

use agent_core::{Agent, AgentMessage};
use tokio::sync::mpsc;

use crate::app::App;
use crate::app::init::constructor_backend::BackendConfig;
use crate::app::persistence::auth_store::{AuthStore, StoredConnection, save_auth_store};

impl App {
    pub(crate) fn activate_connection(&mut self, connection: &StoredConnection) -> Result<()> {
        let env = BackendConfig::from_connection(connection).ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "{} is saved, but runtime activation is not implemented for this auth/provider yet",
                connection.provider_name
            )
        })?;

        self.activate_backend_environment(env, connection.model.clone())?;
        let _ = self.load_models();
        Ok(())
    }

    pub(crate) fn activate_local_model(&mut self, model_filename: String) -> Result<()> {
        self.connect.active_connection_id = None;
        let store = AuthStore {
            version: 1,
            active_connection_id: None,
            connections: self.connect.saved_connections.clone(),
        };
        save_auth_store(&store)?;

        let env = BackendConfig::read().into_environment();
        self.activate_backend_environment(env, Some(model_filename))
    }

    pub(crate) fn select_connected_model(
        &mut self,
        connection_id: &str,
        model: String,
    ) -> Result<String> {
        let idx = self
            .connect
            .saved_connections
            .iter()
            .position(|connection| connection.id == connection_id)
            .ok_or_else(|| color_eyre::eyre::eyre!("Connected model source not found"))?;

        self.connect.saved_connections[idx].model = Some(model.clone());
        self.connect.saved_connections[idx].updated_at =
            crate::app::persistence::auth_store::current_unix_timestamp();
        let connection = self.connect.saved_connections[idx].clone();

        let mut store = AuthStore {
            version: 1,
            active_connection_id: Some(connection.id.clone()),
            connections: self.connect.saved_connections.clone(),
        };
        store.upsert_connection(connection.clone());
        save_auth_store(&store)?;
        self.connect.saved_connections = store.connections;
        self.connect.active_connection_id = store.active_connection_id;

        self.activate_connection(&connection)?;
        let _ = self.load_models();
        Ok(connection.provider_name)
    }

    fn activate_backend_environment(
        &mut self,
        env: crate::app::init::constructor_backend::BackendEnvironment,
        model: Option<String>,
    ) -> Result<()> {
        let replace_system_prompt =
            Self::is_claude_code_auth_transition(env.provider_id(), env.auth_kind());
        let conversation = self
            .agent
            .as_ref()
            .and_then(|agent| futures::executor::block_on(agent.export_conversation()));

        Self::apply_backend_environment(&env);
        if let Some(model) = model {
            self.current_model = Some(model);
        }
        self.refresh_context_window();
        let _ = self.save_config();

        self.agent_tx = None;
        self.agent_rx = None;
        self.agent = None;

        let agent = futures::executor::block_on(Agent::new_with_model(self.current_model.clone()))
            .map_err(|e| color_eyre::eyre::eyre!(e.to_string()))?;
        futures::executor::block_on(agent.initialize_backend())
            .map_err(|e| color_eyre::eyre::eyre!(e.to_string()))?;
        if let Some(messages_json) = conversation.as_deref() {
            let restore_result = if replace_system_prompt {
                futures::executor::block_on(
                    agent.restore_conversation_with_replaced_system_prompt(messages_json),
                )
            } else {
                futures::executor::block_on(agent.restore_conversation(messages_json))
            };
            restore_result.map_err(|e| color_eyre::eyre::eyre!(e.to_string()))?;
        }

        let agent_arc = Arc::new(agent);
        let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentMessage>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<AgentMessage>();
        Self::spawn_agent_runtime(Arc::clone(&agent_arc), input_rx, output_tx);
        self.agent = Some(agent_arc);
        self.agent_tx = Some(input_tx);
        self.agent_rx = Some(output_rx);

        Ok(())
    }

    fn is_claude_code_auth_transition(
        target_provider_id: Option<&str>,
        target_auth_kind: Option<&str>,
    ) -> bool {
        let current_provider_id = std::env::var("NITE_HTTP_PROVIDER_ID").ok();
        let current_auth_kind = std::env::var("NITE_HTTP_AUTH_KIND").ok();

        current_provider_id.as_deref() == Some("anthropic")
            && current_auth_kind.as_deref() == Some("claude_code")
            && !(target_provider_id == Some("anthropic") && target_auth_kind == Some("claude_code"))
    }
}
