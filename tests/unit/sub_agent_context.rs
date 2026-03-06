use super::{MessageState, MessageType, SubAgentContext};

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
