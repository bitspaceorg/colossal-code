use crate::app::orchestrator::spec_command::ParsedSpecCommand;
use crate::app::orchestrator::spec_messages;

use super::control::{SpecAgentBridge, SpecCliContext};

pub(crate) async fn execute(
    ctx: &mut SpecCliContext<'_>,
    agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
    command: ParsedSpecCommand,
) {
    match command {
        ParsedSpecCommand::Show => show_spec(ctx),
        ParsedSpecCommand::Split { index } => split_step(ctx, agent, index).await,
        ParsedSpecCommand::Status => show_spec_status(ctx, agent),
        ParsedSpecCommand::Abort => abort_run(ctx),
        ParsedSpecCommand::Pause => pause_run(ctx),
        ParsedSpecCommand::Resume => resume_run(ctx),
        ParsedSpecCommand::Rerun => rerun_verifiers(ctx),
        ParsedSpecCommand::History => show_history(ctx),
    }
}

fn show_spec(ctx: &mut SpecCliContext<'_>) {
    if let Some(spec) = ctx.current_spec.as_ref() {
        ctx.message_log
            .push(spec_messages::format_spec_overview(spec));
    } else {
        ctx.message_log.push(
            "[SPEC] No spec loaded. Use /spec <path|goal> to load or create one.".to_string(),
        );
    }
}

async fn split_step(
    ctx: &mut SpecCliContext<'_>,
    agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
    index: Option<String>,
) {
    let Some(index) = index else {
        ctx.message_log
            .push("[SPEC ERROR] Usage: /spec split <index>".to_string());
        return;
    };

    let Some(spec) = ctx.current_spec.as_ref() else {
        ctx.message_log.push(
            "[SPEC ERROR] No spec loaded. Load a spec first with /spec <path|goal>".to_string(),
        );
        return;
    };

    let Some(agent) = agent else {
        ctx.message_log
            .push("[SPEC ERROR] Agent not initialized".to_string());
        return;
    };

    if let Err(e) = agent.validate_step_index(spec, &index) {
        ctx.message_log.push(format!("[SPEC ERROR] {}", e));
        return;
    }

    let Some(step) = spec.steps.iter().find(|s| s.index == index) else {
        ctx.message_log
            .push(format!("[SPEC ERROR] Step {} not found", index));
        return;
    };

    match agent.request_split(step).await {
        Ok(child_spec) => {
            if let Some(control) = ctx.orchestrator_control {
                match control.inject_split(index.clone(), child_spec.clone()) {
                    Ok(()) => {
                        ctx.message_log
                            .push(spec_messages::format_split_injected(&index, &child_spec));
                    }
                    Err(e) => {
                        ctx.message_log
                            .push(format!("[SPEC ERROR] Failed to inject split: {}", e));
                    }
                }
            } else {
                ctx.message_log.push(
                    "[SPEC ERROR] No orchestrator control available to inject split".to_string(),
                );
            }
        }
        Err(e) => {
            ctx.message_log
                .push(format!("[SPEC ERROR] Failed to split step: {}", e));
        }
    }
}

fn show_spec_status(
    ctx: &mut SpecCliContext<'_>,
    agent: Option<&(dyn SpecAgentBridge + Send + Sync)>,
) {
    if let Some(spec) = ctx.current_spec.as_ref() {
        if let Some(agent) = agent {
            match agent.get_spec_status(spec) {
                Ok(json) => ctx
                    .message_log
                    .push(format!("[SPEC STATUS]\n```json\n{}\n```", json)),
                Err(e) => ctx
                    .message_log
                    .push(format!("[SPEC ERROR] Failed to serialize spec: {}", e)),
            }
        } else {
            ctx.message_log
                .push("[SPEC ERROR] Agent not initialized".to_string());
        }
    } else {
        ctx.message_log.push("[SPEC] No spec loaded.".to_string());
    }
}

fn abort_run(ctx: &mut SpecCliContext<'_>) {
    if let Some(control) = ctx.orchestrator_control {
        if let Err(e) = control.abort() {
            ctx.message_log
                .push(format!("[SPEC ERROR] Failed to abort: {}", e));
        } else {
            ctx.message_log
                .push("[SPEC] Abort signal sent to orchestrator.".to_string());
            *ctx.status_message = Some("Abort requested".to_string());
        }
    } else {
        ctx.message_log
            .push("[SPEC] No spec running to abort.".to_string());
    }
}

fn pause_run(ctx: &mut SpecCliContext<'_>) {
    if let Some(control) = ctx.orchestrator_control {
        if control.is_paused() {
            ctx.message_log
                .push("[SPEC] Orchestrator is already paused.".to_string());
        } else if let Err(e) = control.pause() {
            ctx.message_log
                .push(format!("[SPEC ERROR] Failed to pause: {}", e));
        } else {
            *ctx.orchestrator_paused = true;
            ctx.message_log
                .push("[SPEC] Orchestrator paused. Use /spec resume to continue.".to_string());
            *ctx.status_message = Some("Paused orchestrator".to_string());
        }
    } else {
        ctx.message_log
            .push("[SPEC] No orchestrator running to pause.".to_string());
    }
}

fn resume_run(ctx: &mut SpecCliContext<'_>) {
    if let Some(control) = ctx.orchestrator_control {
        if !control.is_paused() {
            ctx.message_log
                .push("[SPEC] Orchestrator is not paused.".to_string());
        } else if let Err(e) = control.resume() {
            ctx.message_log
                .push(format!("[SPEC ERROR] Failed to resume: {}", e));
        } else {
            *ctx.orchestrator_paused = false;
            ctx.message_log
                .push("[SPEC] Orchestrator resumed.".to_string());
            *ctx.status_message = Some("Resumed orchestrator".to_string());
        }
    } else {
        ctx.message_log
            .push("[SPEC] No orchestrator running to resume.".to_string());
    }
}

fn rerun_verifiers(ctx: &mut SpecCliContext<'_>) {
    if let Some(control) = ctx.orchestrator_control {
        if let Err(e) = control.rerun_verifiers() {
            ctx.message_log
                .push(format!("[SPEC ERROR] Failed to rerun verifiers: {}", e));
        } else {
            ctx.message_log
                .push("[SPEC] Rerunning verifiers on last step.".to_string());
            *ctx.status_message = Some("Re-running verifiers".to_string());
        }
    } else {
        ctx.message_log
            .push("[SPEC] No orchestrator running.".to_string());
    }
}

fn show_history(ctx: &mut SpecCliContext<'_>) {
    if ctx.orchestrator_history.is_empty() {
        ctx.message_log
            .push("[SPEC] No task history available.".to_string());
        return;
    }

    ctx.message_log
        .push(spec_messages::format_history(ctx.orchestrator_history));
}
