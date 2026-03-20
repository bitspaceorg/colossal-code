use super::{
    AgentState, App, AssistantMode, Color, HelpTab, PersistenceState, SafetyState, UiState,
};
use agent_core::safety_config::SafetyMode;

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
        App::format_tool_result("read_file", read_result, None),
        "Read 2 lines (12 chars)"
    );

    let failure_result = "status: Failure\nmessage: permission denied\n";
    assert_eq!(
        App::format_tool_result("write_file", failure_result, None),
        "Error: permission denied"
    );
}
