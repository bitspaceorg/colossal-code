use super::*;
use agent_protocol::types::message::{Message, Role};
use agent_protocol::types::task::TaskMetadata;

#[test]
fn summary_includes_commands_and_artifacts() {
    let mut task = Task::new();
    task.id = "task-1".to_string();
    let mut agent_msg = Message::agent("Done".to_string());
    agent_msg.role = Role::Agent;
    agent_msg.task_id = Some(task.id.clone());
    task.messages.push(agent_msg);

    let mut metadata = TaskMetadata::default();
    metadata.extra.insert("stepIndex".to_string(), json!("1"));
    metadata
        .extra
        .insert("stepInstructions".to_string(), json!("Implement feature"));
    metadata.extra.insert(
        "toolLog".to_string(),
        json!([
            {
                "name": "exec_command",
                "arguments": "cargo test",
                "result": "ok"
            }
        ]),
    );
    task.metadata = Some(metadata);

    let summary = build_summary(&task, Some(&["src/lib.rs".into()]));
    assert!(summary.summary_text.contains("Implement feature"));
    assert!(summary.summary_text.contains("cargo test"));
    assert_eq!(summary.artifacts_touched, vec!["src/lib.rs".to_string()]);
    serde_json::to_string(&summary).expect("summary serializes");
}
