use crate::{AgentMessage, orchestrator::OrchestratorEvent};
use anyhow::Result;
use chrono::Utc;
use std::time::Instant;
use tokio::sync::mpsc;

pub async fn execute_step_simple(
    step: crate::SpecStep,
    spec: &crate::SpecSheet,
) -> Result<agent_protocol::Task> {
    use agent_protocol::{Task, TaskState, TaskVerification, VerificationStatus};

    let prompt = format!(
        "## Current Task: {}\n\n\
        **Spec:** {}\n\n\
        **Step {} of {}:** {}\n\n\
        **Instructions:**\n{}\n\n\
        **Acceptance Criteria:**\n{}",
        step.title,
        spec.title,
        step.index,
        spec.steps.len(),
        step.title,
        step.instructions,
        step.acceptance_criteria
            .iter()
            .map(|c| format!("- {}", c))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let mut task = Task::new();
    task.context_id = Some(spec.id.clone());

    let summary = crate::TaskSummary {
        task_id: task.id.clone(),
        step_index: step.index.clone(),
        summary_text: format!(
            "Step {} completed: {}\nInstructions: {}",
            step.index, step.title, step.instructions
        ),
        artifacts_touched: Vec::new(),
        tests_run: Vec::new(),
        verification: TaskVerification {
            status: VerificationStatus::Passed,
            feedback: Vec::new(),
        },
        worktree: None,
    };

    let mut metadata = agent_protocol::TaskMetadata::default();
    metadata.summary = Some(serde_json::to_value(&summary)?);
    metadata.extra.insert(
        "step_index".to_string(),
        serde_json::Value::String(step.index.clone()),
    );
    metadata
        .extra
        .insert("prompt".to_string(), serde_json::Value::String(prompt));
    metadata.extra.insert(
        "step_title".to_string(),
        serde_json::Value::String(step.title.clone()),
    );
    task.metadata = Some(metadata);

    task.set_state(
        TaskState::Completed,
        Some(format!("Step {} completed: {}", step.index, step.title)),
    );

    Ok(task)
}

pub async fn execute_step_with_events(
    agent: &crate::Agent,
    step: crate::SpecStep,
    spec: &crate::SpecSheet,
    prefix: &str,
    event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
) -> Result<agent_protocol::Task> {
    execute_step_with_events_and_cancel(agent, step, spec, prefix, event_tx, None).await
}

pub async fn execute_step_with_events_and_cancel(
    agent: &crate::Agent,
    step: crate::SpecStep,
    spec: &crate::SpecSheet,
    prefix: &str,
    event_tx: Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    mut cancel_rx: Option<mpsc::UnboundedReceiver<()>>,
) -> Result<agent_protocol::Task> {
    use agent_protocol::{Task, TaskState, TaskVerification, VerificationStatus};
    use tokio::runtime::Builder;

    agent.clear_conversation().await;

    let worktree_context = if let Some(ref cwd) = agent.working_directory {
        format!(
            "\n\n**IMPORTANT - Working Directory:**\n\
            You are working in an isolated git worktree at: {}\n\
            ALL file operations (create, edit, read) MUST use this directory as the base.\n\
            Use relative paths from this directory, or absolute paths starting with this path.\n\
            Do NOT use paths like ~/rust/todo or /home/user/... - use the worktree path instead.",
            cwd.display()
        )
    } else {
        String::new()
    };

    let prompt = format!(
        "## Current Task: {}\n\n\
        **Spec:** {}\n\n\
        **Step {} of {}:** {}\n\n\
        **Instructions:**\n{}{}\n\n\
        **Acceptance Criteria:**\n{}",
        step.title,
        spec.title,
        step.index,
        spec.steps.len(),
        step.title,
        step.instructions,
        worktree_context,
        step.acceptance_criteria
            .iter()
            .map(|c| format!("- {}", c))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let mut task = Task::new();
    task.context_id = Some(spec.id.clone());

    let (tx, mut rx) = mpsc::unbounded_channel();
    let agent_clone = agent.clone();
    let prompt_clone = prompt.clone();

    let blocking = tokio::task::spawn_blocking(move || {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create agent runtime: {e}"))?;
        runtime.block_on(async move { agent_clone.process_message(prompt_clone, tx).await })
    });

    let mut latest_response = String::new();
    let mut error_message: Option<String> = None;
    let mut tool_log: Vec<serde_json::Value> = Vec::new();
    let prefix_owned = prefix.to_string();
    let mut thinking_start_time = Instant::now();

    if let Some(ref tx) = event_tx {
        let _ = tx.send(OrchestratorEvent::AgentMessage {
            prefix: prefix_owned.clone(),
            message: crate::orchestrator::SubAgentMessage::UserPrompt {
                content: prompt.clone(),
            },
        });
    }

    let mut cancel_requested = false;

    loop {
        if cancel_requested || cancel_rx.is_none() {
            match rx.recv().await {
                Some(agent_msg) => {
                    if handle_agent_message(
                        agent_msg,
                        &mut latest_response,
                        &mut tool_log,
                        &mut thinking_start_time,
                        &mut error_message,
                        &event_tx,
                        &prefix_owned,
                    ) {
                        break;
                    }
                }
                None => break,
            }
        } else {
            let mut cancel_stream = cancel_rx.take().unwrap();
            tokio::select! {
                msg = rx.recv() => {
                    cancel_rx = Some(cancel_stream);
                    match msg {
                        Some(agent_msg) => {
                            if handle_agent_message(
                                agent_msg,
                                &mut latest_response,
                                &mut tool_log,
                                &mut thinking_start_time,
                                &mut error_message,
                                &event_tx,
                                &prefix_owned,
                            ) {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                signal = cancel_stream.recv() => {
                    if signal.is_some() {
                        cancel_requested = true;
                        agent.request_cancel();
                    }
                }
            }
        }
    }

    let was_cancelled = cancel_requested;

    match blocking.await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            if error_message.is_none() {
                error_message = Some(err.to_string());
            }
        }
        Err(err) => {
            if error_message.is_none() {
                error_message = Some(err.to_string());
            }
        }
    }

    if was_cancelled {
        let mut task = Task::new();
        task.context_id = Some(spec.id.clone());
        task.status = agent_protocol::TaskStatus {
            state: TaskState::Cancelled,
            timestamp: Some(Utc::now()),
            message: Some("Step cancelled by user".to_string()),
            error: None,
        };
        return Ok(task);
    }

    if let Some(err) = error_message {
        task.set_state(TaskState::Failed, Some(err));
    } else {
        let summary = crate::TaskSummary {
            task_id: task.id.clone(),
            step_index: step.index.clone(),
            summary_text: if latest_response.is_empty() {
                format!("Step {} completed: {}", step.index, step.title)
            } else {
                latest_response.clone()
            },
            artifacts_touched: Vec::new(),
            tests_run: Vec::new(),
            verification: TaskVerification {
                status: VerificationStatus::Passed,
                feedback: Vec::new(),
            },
            worktree: None,
        };

        let mut metadata = agent_protocol::TaskMetadata::default();
        metadata.summary = Some(serde_json::to_value(&summary)?);
        metadata.extra.insert(
            "step_index".to_string(),
            serde_json::Value::String(step.index.clone()),
        );
        metadata
            .extra
            .insert("prompt".to_string(), serde_json::Value::String(prompt));
        metadata.extra.insert(
            "step_title".to_string(),
            serde_json::Value::String(step.title.clone()),
        );
        metadata
            .extra
            .insert("toolLog".to_string(), serde_json::Value::Array(tool_log));
        task.metadata = Some(metadata);
        task.set_state(
            TaskState::Completed,
            Some(format!("Step {} completed", step.index)),
        );
    }

    Ok(task)
}

pub fn handle_agent_message(
    agent_msg: AgentMessage,
    latest_response: &mut String,
    tool_log: &mut Vec<serde_json::Value>,
    thinking_start_time: &mut Instant,
    error_message: &mut Option<String>,
    event_tx: &Option<mpsc::UnboundedSender<OrchestratorEvent>>,
    prefix_owned: &String,
) -> bool {
    match agent_msg {
        AgentMessage::AgentResponse(content, _) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::Text {
                        content: content.clone(),
                    },
                });
            }
            *latest_response = content;
        }
        AgentMessage::ThinkingContent(content, _) => {
            *thinking_start_time = Instant::now();
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::Thinking {
                        content,
                        duration_secs: 0,
                    },
                });
            }
        }
        AgentMessage::ThinkingComplete(_) => {
            let duration = thinking_start_time.elapsed().as_secs();
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::Thinking {
                        content: String::new(),
                        duration_secs: duration,
                    },
                });
            }
        }
        AgentMessage::Error(err) => {
            *error_message = Some(err.clone());
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::Error {
                        message: err.clone(),
                    },
                });
            }
            return true;
        }
        AgentMessage::ToolCallStarted(name, args) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::ToolCallStarted {
                    prefix: prefix_owned.clone(),
                    tool_name: name.clone(),
                    arguments: args.clone(),
                });
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::ToolCall {
                        tool_name: name.clone(),
                        arguments: args.clone(),
                        result: None,
                        is_error: false,
                    },
                });
            }
            tool_log.push(serde_json::json!({
                "name": name,
                "arguments": args,
                "result": serde_json::Value::Null,
            }));
        }
        AgentMessage::ToolCallCompleted(name, result) => {
            let is_error = result.starts_with("Error:")
                || result.starts_with("error:")
                || result.starts_with("Failed:")
                || result.starts_with("failed:")
                || result.starts_with("Permission denied")
                || result.starts_with("No such file")
                || result.starts_with("Command failed")
                || (result.len() < 500 && result.contains("\"error\""))
                || (result.len() < 500 && result.contains("\"is_error\": true"))
                || result.contains("status: Failure");

            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::ToolCallCompleted {
                    prefix: prefix_owned.clone(),
                    tool_name: name.clone(),
                    result: result.clone(),
                    is_error,
                });
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::ToolCall {
                        tool_name: name.clone(),
                        arguments: String::new(),
                        result: Some(result.clone()),
                        is_error,
                    },
                });
            }

            if let Some(entry) = tool_log.iter_mut().rev().find(|e| {
                e.get("name").and_then(|n| n.as_str()) == Some(&name)
                    && e.get("result").map(|r| r.is_null()).unwrap_or(false)
            }) {
                entry["result"] = serde_json::Value::String(result);
            }
        }
        AgentMessage::GenerationStats(stats) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::GenerationStats {
                        tokens_per_sec: stats.avg_completion_tok_per_sec,
                        input_tokens: stats.prompt_tokens,
                        output_tokens: stats.completion_tokens,
                    },
                });
            }
        }
        AgentMessage::Done => {
            if let Some(tx) = event_tx {
                let _ = tx.send(OrchestratorEvent::AgentMessage {
                    prefix: prefix_owned.clone(),
                    message: crate::orchestrator::SubAgentMessage::Done,
                });
            }
            return true;
        }
        _ => {}
    }

    false
}
