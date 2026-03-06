use super::{App, MessageState, MessageType};
use crate::app::commands::submit::{parse_queue_choice, QueueChoiceAction};

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
fn reconcile_message_vectors_fills_missing_entries_with_safe_defaults() {
    let messages = vec!["user message".to_string(), "agent message".to_string()];
    let mut types = vec![MessageType::User];
    let mut states = vec![MessageState::Sent];
    let mut metadata = vec![];
    let mut timestamps = vec![];

    App::reconcile_message_vectors_fields(
        &messages,
        &mut types,
        &mut states,
        &mut metadata,
        &mut timestamps,
    );

    assert_eq!(types.len(), messages.len());
    assert_eq!(states.len(), messages.len());
    assert_eq!(metadata.len(), messages.len());
    assert_eq!(timestamps.len(), messages.len());
    assert_eq!(types[0], MessageType::User);
    assert_eq!(types[1], MessageType::User);
}

#[test]
fn reconcile_message_vectors_truncates_extra_entries() {
    let messages = vec!["only one".to_string()];
    let mut types = vec![MessageType::User, MessageType::Agent];
    let mut states = vec![MessageState::Sent, MessageState::Queued];
    let mut metadata = vec![None, None];
    let mut timestamps = vec![std::time::SystemTime::now(), std::time::SystemTime::now()];

    App::reconcile_message_vectors_fields(
        &messages,
        &mut types,
        &mut states,
        &mut metadata,
        &mut timestamps,
    );

    assert_eq!(types.len(), 1);
    assert_eq!(states.len(), 1);
    assert_eq!(metadata.len(), 1);
    assert_eq!(timestamps.len(), 1);
}
