use std::sync::Arc;

use agent_core::{Agent, AgentMessage};
use tokio::sync::mpsc;

use crate::app::App;

impl App {
    pub(crate) fn spawn_agent_runtime(
        agent_arc: Arc<Agent>,
        mut input_rx: mpsc::UnboundedReceiver<AgentMessage>,
        output_tx: mpsc::UnboundedSender<AgentMessage>,
    ) {
        let agent_clone = Arc::clone(&agent_arc);
        let output_tx_clone = output_tx.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async {
                while let Some(msg) = input_rx.recv().await {
                    match msg {
                        AgentMessage::UserInput(user_message) => {
                            let agent = agent_clone.clone();
                            let tx = output_tx_clone.clone();
                            tokio::task::spawn_local(async move {
                                if let Err(e) =
                                    agent.process_message(user_message, tx.clone()).await
                                {
                                    let _ = tx.send(AgentMessage::Error(format!(
                                        "Agent failed to process request: {}",
                                        e
                                    )));
                                    let _ = tx.send(AgentMessage::Done);
                                }
                            });
                        }
                        AgentMessage::Cancel => {
                            agent_clone.request_cancel();
                        }
                        AgentMessage::ClearContext => {
                            let agent_clone = agent_clone.clone();
                            let tx_clone = output_tx_clone.clone();
                            tokio::spawn(async move {
                                agent_clone.clear_conversation().await;
                                let _ = tx_clone.send(AgentMessage::ContextCleared);
                            });
                        }
                        AgentMessage::InjectContext(summary) => {
                            let agent_clone = agent_clone.clone();
                            let tx_clone = output_tx_clone.clone();
                            tokio::spawn(async move {
                                agent_clone.inject_summary_context(&summary).await;
                                let _ = tx_clone.send(AgentMessage::ContextInjected);
                            });
                        }
                        AgentMessage::ReloadModel(model_filename) => {
                            let agent_clone = agent_clone.clone();
                            let tx_clone = output_tx_clone.clone();
                            tokio::task::spawn_local(async move {
                                match agent_clone.reload_model(model_filename).await {
                                    Ok(_) => match agent_clone.initialize_backend().await {
                                        Ok(_) => {
                                            let _ = tx_clone.send(AgentMessage::ModelLoaded);
                                        }
                                        Err(e) => {
                                            let _ = tx_clone.send(AgentMessage::Error(format!(
                                                "Failed to load model: {}",
                                                e
                                            )));
                                        }
                                    },
                                    Err(e) => {
                                        let _ = tx_clone.send(AgentMessage::Error(format!(
                                            "Failed to reload model: {}",
                                            e
                                        )));
                                    }
                                }
                            });
                        }
                        AgentMessage::ApprovalResponse(approved) => {
                            let agent_clone = agent_clone.clone();
                            tokio::task::spawn_local(async move {
                                agent_clone.handle_approval_response(approved).await;
                            });
                        }
                        _ => {}
                    }
                }
            }));
        });
    }
}
