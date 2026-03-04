use super::{
    AgentState, App, AssistantMode, Color, HelpTab, MessageState, MessageType, PersistenceState,
    SafetyState, SubAgentContext, UiState,
};
use crate::model_context;
use crate::submit_message_reducer::{parse_queue_choice, QueueChoiceAction};
use agent_core::{safety_config::SafetyMode, SpecSheet, SpecStep, StepStatus};
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;

fn test_step(
    index: &str,
    title: &str,
    instructions: &str,
    sub_spec: Option<SpecSheet>,
) -> SpecStep {
    SpecStep {
        index: index.to_string(),
        title: title.to_string(),
        instructions: instructions.to_string(),
        acceptance_criteria: Vec::new(),
        required_tools: Vec::new(),
        constraints: Vec::new(),
        dependencies: Vec::new(),
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
        status: StepStatus::Pending,
        sub_spec: sub_spec.map(Box::new),
        completed_at: None,
    }
}

fn test_spec(id: &str, steps: Vec<SpecStep>) -> SpecSheet {
    SpecSheet {
        id: id.to_string(),
        title: format!("Spec {}", id),
        description: "test spec".to_string(),
        steps,
        created_by: "tester".to_string(),
        created_at: Utc::now(),
        metadata: json!({}),
    }
}

#[test]
fn sub_agent_context_uses_typed_ui_messages() {
    let mut context = SubAgentContext::new("1".to_string(), "step".to_string());

    context.add_user_message("hi".to_string());
    context.add_agent_text("hello".to_string());

    assert_eq!(context.messages.len(), 2);
    assert_eq!(context.messages[0].content, "hi");
    assert_eq!(context.messages[0].message_type, MessageType::User);
    assert_eq!(context.messages[0].message_state, MessageState::Sent);
    assert_eq!(context.messages[1].content, "hello");
    assert_eq!(context.messages[1].message_type, MessageType::Agent);
    assert_eq!(context.messages[1].message_state, MessageState::Sent);
}

#[test]
fn sub_agent_context_drops_thinking_placeholder_before_agent_text() {
    let mut context = SubAgentContext::new("1".to_string(), "step".to_string());

    context.start_thinking("".to_string());
    context.add_agent_text("done".to_string());

    assert_eq!(context.messages.len(), 1);
    assert_eq!(context.messages[0].content, "done");
    assert_eq!(context.messages[0].message_type, MessageType::Agent);
}

#[test]
fn sub_agent_context_snapshot_preserves_typed_messages() {
    let mut context = SubAgentContext::new("1".to_string(), "step".to_string());

    context.add_user_message("queued question".to_string());
    context.add_agent_text("answer".to_string());

    let snapshot = context.to_snapshot();
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(
        snapshot.message_types,
        vec![MessageType::User, MessageType::Agent]
    );
}

#[test]
fn grouped_ui_state_defaults_match_previous_behavior() {
    let ui = UiState::default();

    assert!(!ui.show_help);
    assert!(!ui.show_resume);
    assert!(matches!(ui.help_tab, HelpTab::General));
}

#[test]
fn grouped_agent_safety_and_persistence_defaults_are_empty() {
    let agent = AgentState::default();
    let safety = SafetyState::default();
    let persistence = PersistenceState::default();

    assert!(!agent.agent_processing);
    assert!(!agent.agent_interrupted);
    assert!(!agent.is_compacting);
    assert!(!agent.agent_response_started);
    assert!(agent.interrupt_pending.is_none());

    assert!(matches!(safety.assistant_mode, AssistantMode::None));
    assert!(!safety.show_approval_prompt);
    assert!(!safety.show_sandbox_prompt);
    assert!(!safety.sandbox_enabled);
    assert!(safety.approval_prompt_content.is_empty());
    assert!(safety.sandbox_blocked_path.is_empty());

    assert!(!persistence.save_pending);
    assert!(persistence.current_conversation_id.is_none());
    assert!(persistence.current_conversation_path.is_none());
    assert!(persistence.current_forked_from.is_none());
    assert!(persistence.current_forked_at.is_none());
}

#[test]
fn assistant_mode_cycles_through_expected_order() {
    let mut mode = AssistantMode::None;

    mode = mode.next();
    assert!(matches!(mode, AssistantMode::Yolo));

    mode = mode.next();
    assert!(matches!(mode, AssistantMode::Plan));

    mode = mode.next();
    assert!(matches!(mode, AssistantMode::AutoAccept));

    mode = mode.next();
    assert!(matches!(mode, AssistantMode::ReadOnly));

    mode = mode.next();
    assert!(matches!(mode, AssistantMode::None));
}

#[test]
fn assistant_mode_to_safety_mode_preserves_expected_policy() {
    assert_eq!(
        AssistantMode::None.to_safety_mode(),
        Some(SafetyMode::Regular)
    );
    assert_eq!(AssistantMode::Yolo.to_safety_mode(), Some(SafetyMode::Yolo));
    assert_eq!(
        AssistantMode::Plan.to_safety_mode(),
        Some(SafetyMode::Regular)
    );
    assert_eq!(
        AssistantMode::AutoAccept.to_safety_mode(),
        Some(SafetyMode::Regular)
    );
    assert_eq!(
        AssistantMode::ReadOnly.to_safety_mode(),
        Some(SafetyMode::ReadOnly)
    );
}

#[test]
fn assistant_mode_to_display_matches_expected_labels() {
    assert_eq!(AssistantMode::None.to_display(), None);
    assert_eq!(
        AssistantMode::Yolo.to_display(),
        Some(("YOLO mode".to_string(), Color::Red))
    );
    assert_eq!(
        AssistantMode::Plan.to_display(),
        Some(("plan mode".to_string(), Color::Blue))
    );
    assert_eq!(
        AssistantMode::AutoAccept.to_display(),
        Some(("auto-accept edits".to_string(), Color::Green))
    );
    assert_eq!(
        AssistantMode::ReadOnly.to_display(),
        Some(("read-only".to_string(), Color::Yellow))
    );
}

#[test]
fn format_tool_result_parses_success_and_failure_yaml_fields() {
    let read_result = "status: Success\ncontent: |-\n  first\n  second\n";
    assert_eq!(
        App::format_tool_result("read_file", read_result),
        "Read 2 lines (12 chars)"
    );

    let failure_result = "status: Failure\nmessage: permission denied\n";
    assert_eq!(
        App::format_tool_result("write_file", failure_result),
        "Error: permission denied"
    );
}

#[test]
fn parses_queue_choice_actions() {
    assert_eq!(parse_queue_choice("1"), Some(QueueChoiceAction::Queue));
    assert_eq!(parse_queue_choice("2"), Some(QueueChoiceAction::Interrupt));
    assert_eq!(parse_queue_choice("3"), Some(QueueChoiceAction::Cancel));
}

#[test]
fn rejects_invalid_queue_choice_actions() {
    assert_eq!(parse_queue_choice("0"), None);
    assert_eq!(parse_queue_choice("4"), None);
    assert_eq!(parse_queue_choice("interrupt"), None);
    assert_eq!(
        parse_queue_choice(" 2 "),
        Some(QueueChoiceAction::Interrupt)
    );
}

#[test]
fn extract_parameter_count_parses_common_model_sizes() {
    assert_eq!(
        model_context::extract_parameter_count("Qwen2.5-7b-Instruct-Q4_K_M.gguf"),
        Some("7B".to_string())
    );
    assert_eq!(
        model_context::extract_parameter_count("TinyLlama-1.5B-Chat-v1.0.gguf"),
        Some("1.5B".to_string())
    );
    assert_eq!(
        model_context::extract_parameter_count("Meta-Llama-3.1-70B-Instruct.Q8_0.gguf"),
        Some("70B".to_string())
    );
}

#[test]
fn extract_parameter_count_avoids_partial_number_matches() {
    assert_eq!(
        model_context::extract_parameter_count("example-130B-instruct.gguf"),
        None
    );
    assert_eq!(
        model_context::extract_parameter_count("example-314b-chat.gguf"),
        None
    );
}

#[test]
fn estimate_token_count_rounds_up_and_never_zero() {
    assert_eq!(App::estimate_token_count_for_text(""), 1);
    assert_eq!(App::estimate_token_count_for_text("abcd"), 1);
    assert_eq!(App::estimate_token_count_for_text("abcde"), 2);
}

#[test]
fn compose_step_prefix_and_collect_labels_handle_nested_steps() {
    assert_eq!(App::compose_step_prefix("", "1"), "1");
    assert_eq!(App::compose_step_prefix("2", "1"), "2.1");

    let child = test_step("1", "Child", "Validate", None);
    let parent = test_step(
        "2",
        "Parent",
        "Build",
        Some(test_spec("nested", vec![child])),
    );

    let mut labels = HashMap::new();
    App::collect_step_labels(&parent, "", &mut labels);

    assert_eq!(labels.get("2").map(String::as_str), Some("Parent — Build"));
    assert_eq!(
        labels.get("2.1").map(String::as_str),
        Some("Child — Validate")
    );
}

#[test]
fn infer_search_label_detects_search_commands_and_fallbacks() {
    assert_eq!(
        App::infer_search_label("rg -n user src/main.rs"),
        Some("Searched user in src/main.rs".to_string())
    );
    assert_eq!(
        App::infer_search_label("grep --line-number token"),
        Some("Searched token".to_string())
    );
    assert_eq!(App::infer_search_label("ls -la"), None);
    assert_eq!(App::describe_exec_command("ls -la"), "Ran ls -la");
}

#[test]
fn find_step_by_prefix_resolves_nested_sub_specs() {
    let nested = test_step("1", "Nested", "run", None);
    let root = test_step(
        "1",
        "Root",
        "prep",
        Some(test_spec("child-spec", vec![nested])),
    );
    let steps = vec![root];

    assert_eq!(
        App::find_step_by_prefix(&steps, "1").map(|step| step.title.as_str()),
        Some("Root")
    );
    assert_eq!(
        App::find_step_by_prefix(&steps, "1.1").map(|step| step.title.as_str()),
        Some("Nested")
    );
    assert!(App::find_step_by_prefix(&steps, "1.2").is_none());
}
