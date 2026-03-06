use super::App;
use crate::app::init::model_context;
use agent_core::{SpecSheet, SpecStep, StepStatus};
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
