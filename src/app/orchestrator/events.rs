use agent_core::{
    StepStatus, VerificationStatus,
    orchestrator::{OrchestratorEvent, SubAgentMessage},
};

use crate::app::{
    App, MessageState, MessageType, SessionRole, StepToolCallEntry, SubAgentContext, ToolCallStatus,
};

impl App {
    pub(crate) fn handle_orchestrator_event_impl(&mut self, event: OrchestratorEvent) {
        self.has_orchestrator_activity = true;
        match event {
            OrchestratorEvent::StepStatusChanged {
                spec_id,
                spec_title,
                step_index,
                step_title,
                prefix,
                role,
                status,
            } => self.handle_step_status_changed(
                &spec_id,
                &spec_title,
                &step_index,
                &step_title,
                &prefix,
                role,
                status,
            ),
            OrchestratorEvent::SummaryUpdated { summary } => self.handle_summary_updated(summary),
            OrchestratorEvent::VerifierFailed {
                summary,
                feedback: _,
            } => self.handle_verifier_failed(summary),
            OrchestratorEvent::ChildSpecPushed {
                parent_step_index,
                child_spec_id: _,
                child_step_count,
            } => {
                self.status_message = Some(format!(
                    "Step {} split into {} sub-steps",
                    parent_step_index, child_step_count
                ));
            }
            OrchestratorEvent::ChannelClosed { .. } => {}
            OrchestratorEvent::Paused => {
                self.orchestrator_paused = true;
                self.status_message = Some("Orchestrator paused".to_string());
            }
            OrchestratorEvent::Resumed => {
                self.orchestrator_paused = false;
                self.status_message = Some("Orchestrator resumed".to_string());
            }
            OrchestratorEvent::Aborted => self.handle_orchestrator_stopped("Run aborted"),
            OrchestratorEvent::Completed => self.handle_orchestrator_stopped("Spec completed"),
            OrchestratorEvent::Error(error) => {
                self.orchestration_in_progress = false;
                self.messages.push(format!("[SPEC ERROR] {}", error));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(std::time::SystemTime::now());
            }
            OrchestratorEvent::ToolCallStarted {
                prefix,
                tool_name,
                arguments,
            } => {
                // Subagent tool calls bypass the AgentMessage tee; audit
                // them into the step's child conversation. Correlation
                // keys stay prefix-qualified so steps can't collide.
                if let Some(child_id) = self.subagent_conversation_id(&prefix) {
                    if let Some(writer) = &self.db_writer {
                        let id = self.audit.open_tool_call(&format!("{prefix}:{tool_name}"));
                        writer.send(
                            crate::app::persistence::db::writer::WriteOp::ToolCallStarted {
                                id,
                                conversation_id: child_id,
                                tool_name: tool_name.clone(),
                                arguments: arguments.clone(),
                            },
                        );
                    }
                }
                self.handle_tool_call_started(prefix, tool_name, arguments)
            }
            OrchestratorEvent::ToolCallCompleted {
                prefix,
                tool_name,
                result,
                is_error,
            } => {
                if let Some(child_id) = self.subagent_conversation_id(&prefix) {
                    if let Some(writer) = &self.db_writer {
                        if let Some(id) =
                            self.audit.close_tool_call(&format!("{prefix}:{tool_name}"))
                        {
                            writer.send(
                                crate::app::persistence::db::writer::WriteOp::ToolCallCompleted {
                                    id,
                                    conversation_id: child_id,
                                    status: if is_error { "failed" } else { "completed" }
                                        .to_string(),
                                    result: result.clone(),
                                },
                            );
                        }
                    }
                }
                self.handle_tool_call_completed(prefix, is_error)
            }
            OrchestratorEvent::AgentMessage { prefix, message } => {
                self.handle_sub_agent_message(prefix, message)
            }
            OrchestratorEvent::StepCancelled { prefix } => {
                if let Some(context) = self.sub_agent_contexts.get_mut(&prefix) {
                    context.ensure_generation_stats_marker();
                }
                self.status_message = Some(format!("Step {} cancelled", prefix));
            }
        }
    }

    fn handle_step_status_changed(
        &mut self,
        spec_id: &str,
        spec_title: &str,
        step_index: &str,
        step_title: &str,
        prefix: &str,
        role: agent_core::orchestrator::StepRole,
        status: StepStatus,
    ) {
        if let Some(ref mut spec) = self.current_spec
            && spec.id == spec_id
            && let Some(step) = spec.steps.iter_mut().find(|s| s.index == step_index)
        {
            step.status = status;
        }

        self.sync_session_for_step(
            spec_id, spec_title, prefix, step_index, step_title, status, role,
        );
        self.update_active_step_prefix_for_status(prefix, status);
        self.status_message = Some(format!(
            "Step {}: {}",
            step_index,
            Self::status_label_for_step(status)
        ));
    }

    fn handle_summary_updated(&mut self, summary: agent_core::TaskSummary) {
        self.upsert_summary_history(summary.clone());
        let status_str = match summary.verification.status {
            VerificationStatus::Passed => "passed",
            VerificationStatus::Failed => "failed",
            VerificationStatus::Pending => "pending",
        };
        self.status_message = Some(format!(
            "Step {} summary {}",
            summary.step_index, status_str
        ));
    }

    fn handle_verifier_failed(&mut self, summary: agent_core::TaskSummary) {
        self.upsert_summary_history(summary.clone());
        self.status_message = Some(format!("Verifier failed on step {}", summary.step_index));
    }

    fn handle_orchestrator_stopped(&mut self, message: &str) {
        // Next run gets fresh child conversations even with equal prefixes.
        self.subagent_conversations.clear();
        self.teardown_orchestrator_handles();
        self.reset_orchestrator_views();
        self.orchestration_in_progress = false;
        self.status_message = Some(message.to_string());
    }

    fn handle_tool_call_started(&mut self, prefix: String, tool_name: String, arguments: String) {
        let planned_label = if prefix.contains('.') {
            self.step_label_overrides.get(&prefix).cloned()
        } else {
            None
        };
        let label =
            planned_label.unwrap_or_else(|| Self::describe_tool_call(&tool_name, &arguments));
        let entry_id = self.next_tool_call_id;
        self.next_tool_call_id = self.next_tool_call_id.saturating_add(1);

        let session_metadata = self.orchestrator_sessions.get(&prefix);
        let role = session_metadata
            .map(|entry| entry.role.clone())
            .unwrap_or(SessionRole::Implementor);
        let worktree_branch = session_metadata.and_then(|entry| entry.worktree_branch.clone());
        let worktree_path = session_metadata.and_then(|entry| entry.worktree_path.clone());

        self.step_tool_calls
            .entry(prefix.clone())
            .or_default()
            .push(StepToolCallEntry {
                id: entry_id,
                label,
                status: ToolCallStatus::Started,
                role,
                worktree_branch,
                worktree_path,
            });
        self.active_tool_call = Some((prefix, entry_id));
    }

    fn handle_tool_call_completed(&mut self, prefix: String, is_error: bool) {
        if let Some((active_prefix, entry_id)) = self.active_tool_call.take()
            && active_prefix == prefix
            && let Some(entries) = self.step_tool_calls.get_mut(&prefix)
            && let Some(entry) = entries.iter_mut().find(|e| e.id == entry_id)
        {
            entry.status = if is_error {
                ToolCallStatus::Error
            } else {
                ToolCallStatus::Completed
            };
        }
    }

    /// Child conversation for a subagent step, created in the audit
    /// database on first use. Subagent runs are chats *inside* the
    /// current chat: hidden from /resume, navigable from their parent.
    fn subagent_conversation_id(&mut self, prefix: &str) -> Option<String> {
        let parent_id = self.persistence_state.current_conversation_id.clone()?;
        let writer = self.db_writer.clone()?;
        if let Some(existing) = self.subagent_conversations.get(prefix) {
            return Some(existing.clone());
        }
        let title = self
            .current_spec
            .as_ref()
            .and_then(|spec| Self::find_step_by_prefix(&spec.steps, prefix))
            .map(|step| format!("{} · {}", prefix, step.title))
            .unwrap_or_else(|| format!("Step {}", prefix));
        let id = uuid::Uuid::new_v4().to_string();
        writer.send(
            crate::app::persistence::db::writer::WriteOp::EnsureChildConversation {
                id: id.clone(),
                parent_id,
                title,
            },
        );
        self.subagent_conversations
            .insert(prefix.to_string(), id.clone());
        Some(id)
    }

    /// Persist one subagent transcript item into its child conversation.
    fn record_sub_agent_message(&mut self, prefix: &str, message: &SubAgentMessage) {
        let Some(child_id) = self.subagent_conversation_id(prefix) else {
            return;
        };
        let Some(writer) = self.db_writer.clone() else {
            return;
        };
        use crate::app::persistence::db::writer::WriteOp;
        let append = |msg_type: &str, content: &str| WriteOp::AppendMessage {
            conversation_id: child_id.clone(),
            msg_type: format!("\"{msg_type}\""), // serde string form of MessageType
            msg_state: "\"Sent\"".to_string(),
            content: content.to_string(),
            metadata: None,
        };
        match message {
            SubAgentMessage::UserPrompt { content } => {
                writer.send(append("User", content));
                writer.send(WriteOp::Event {
                    conversation_id: Some(child_id),
                    kind: "user.input".to_string(),
                    data: serde_json::json!({ "subagent": prefix }),
                    large: vec![("content".to_string(), content.clone().into_bytes())],
                });
            }
            SubAgentMessage::Text { content } => {
                writer.send(append("Agent", content));
                writer.send(WriteOp::Event {
                    conversation_id: Some(child_id),
                    kind: "assistant.message".to_string(),
                    data: serde_json::json!({ "subagent": prefix }),
                    large: vec![("content".to_string(), content.clone().into_bytes())],
                });
            }
            SubAgentMessage::Thinking {
                content,
                duration_secs,
            } => {
                // duration 0 marks the start (content carried there).
                if *duration_secs == 0 && !content.trim().is_empty() {
                    writer.send(WriteOp::Event {
                        conversation_id: Some(child_id),
                        kind: "thinking.finalized".to_string(),
                        data: serde_json::json!({ "subagent": prefix }),
                        large: vec![("content".to_string(), content.clone().into_bytes())],
                    });
                }
            }
            // Tool calls are audited via OrchestratorEvent::ToolCall*;
            // stats mirror the parent's generation.stats event kind.
            SubAgentMessage::ToolCall { .. } => {}
            _ => {}
        }
    }

    fn handle_sub_agent_message(&mut self, prefix: String, message: SubAgentMessage) {
        self.record_sub_agent_message(&prefix, &message);
        let context = self
            .sub_agent_contexts
            .entry(prefix.clone())
            .or_insert_with(|| {
                let step_title = self
                    .current_spec
                    .as_ref()
                    .and_then(|spec| Self::find_step_by_prefix(&spec.steps, &prefix))
                    .map(|step| step.title.clone())
                    .unwrap_or_else(|| format!("Step {}", prefix));
                SubAgentContext::new(prefix.clone(), step_title)
            });

        match message {
            SubAgentMessage::UserPrompt { content } => {
                context.add_user_message(content);
            }
            SubAgentMessage::Text { content } => {
                context.add_agent_text(content);
            }
            SubAgentMessage::Thinking {
                content,
                duration_secs,
            } => {
                if duration_secs == 0 {
                    context.start_thinking(content);
                } else {
                    context.finish_thinking(duration_secs);
                }
            }
            SubAgentMessage::ToolCall {
                tool_name,
                arguments,
                result,
                is_error: _,
            } => {
                if let Some(result) = result {
                    let formatted_result =
                        Self::format_tool_result(&tool_name, &result, Some(&arguments));
                    context.complete_tool_call(&tool_name, formatted_result, Some(arguments));
                } else {
                    let formatted_args = Self::format_tool_arguments(&tool_name, &arguments);
                    context.add_tool_call_started(&tool_name, formatted_args);
                }
            }
            SubAgentMessage::GenerationStats {
                tokens_per_sec,
                input_tokens,
                output_tokens,
            } => {
                context.set_generation_stats(tokens_per_sec, input_tokens, output_tokens);
            }
            SubAgentMessage::Done => {
                let elapsed = context
                    .thinking_start_time
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(context.thinking_elapsed_secs);
                context.finish_thinking(elapsed);
            }
            SubAgentMessage::Error { message } => {
                context.add_agent_text(format!("[Error: {}]", message));
                let elapsed = context
                    .thinking_start_time
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(context.thinking_elapsed_secs);
                context.finish_thinking(elapsed);
            }
        }
    }
}
