use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect("source file should be readable")
}

fn rust_files_under(path: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![repo_root().join(path)];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).expect("directory should be readable");
        for entry in entries {
            let entry = entry.expect("directory entry should be readable");
            let entry_path = entry.path();
            let file_type = entry
                .file_type()
                .expect("directory entry file type should be readable");
            if file_type.is_dir() {
                stack.push(entry_path);
            } else if file_type.is_file()
                && entry_path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
            {
                files.push(entry_path);
            }
        }
    }

    files.sort();
    files
}

fn assert_none_match(path: &str, forbidden_patterns: &[&str]) {
    for file in rust_files_under(path) {
        let contents = fs::read_to_string(&file).expect("source file should be readable");
        let display = file
            .strip_prefix(repo_root())
            .unwrap_or(Path::new(&file))
            .display()
            .to_string();

        for forbidden in forbidden_patterns {
            assert!(
                !contents.contains(forbidden),
                "{display} should not contain forbidden pattern `{forbidden}`"
            );
        }
    }
}

#[test]
fn main_wires_extracted_modules() {
    let main_rs = read("src/main.rs");

    for required in [
        "mod state_domain;",
        "mod commands;",
        "mod config_model_helpers;",
        "mod model_context;",
        "mod slash_command_executor;",
        "mod spec_cli;",
        "mod spec_orchestrator_reducer;",
        "mod submit_message_reducer;",
        "mod status_helpers;",
        "mod ui;",
        "mod ui_message_event;",
        "pub mod spec_ui;",
        "ui::prompts::render_approval_prompt",
        "ui::prompts::render_sandbox_prompt",
        "UiMessageEvent::parse",
    ] {
        assert!(
            main_rs.contains(required),
            "main.rs should keep module boundary wiring for: {required}"
        );
    }

    let spec_orch = read("src/spec_orchestrator_reducer.rs");
    assert!(
        spec_orch.contains("spec_ui::build_spec_plan_lines"),
        "spec_orchestrator_reducer.rs should own spec plan wiring"
    );
    assert!(
        spec_orch.contains("fn handle_orchestrator_event"),
        "spec_orchestrator_reducer.rs should own orchestrator event handler"
    );
    assert!(
        spec_orch.contains("fn load_spec"),
        "spec_orchestrator_reducer.rs should own load_spec"
    );

    let slash_executor = read("src/slash_command_executor.rs");
    assert!(
        slash_executor.contains("dispatch_slash_command"),
        "slash_command_executor.rs should own dispatch_slash_command wiring"
    );
    assert!(
        slash_executor.contains("fn handle_slash_command"),
        "slash_command_executor.rs should own handle_slash_command"
    );

    let config_helpers = read("src/config_model_helpers.rs");
    assert!(
        config_helpers.contains("model_context::detect_context_length"),
        "config_model_helpers.rs should own context token detection wiring"
    );

    let submit_reducer = read("src/submit_message_reducer.rs");
    assert!(
        submit_reducer.contains("fn submit_message"),
        "submit_message_reducer.rs should own submit_message"
    );
    assert!(
        submit_reducer.contains("QueueChoiceAction"),
        "submit_message_reducer.rs should own QueueChoiceAction"
    );
    assert!(
        submit_reducer.contains("fn parse_queue_choice"),
        "submit_message_reducer.rs should own parse_queue_choice"
    );
}

#[test]
fn submit_message_reducer_owns_submit_logic() {
    let main_rs = read("src/main.rs");
    for forbidden in [
        "fn submit_message",
        "fn save_to_history",
        "fn ensure_conversation_id",
        "enum QueueChoiceAction",
        "fn parse_queue_choice",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs should not define extracted submit logic: {forbidden}"
        );
    }

    let reducer = read("src/submit_message_reducer.rs");
    for required in [
        "fn submit_message",
        "fn save_to_history",
        "fn ensure_conversation_id",
        "enum QueueChoiceAction",
        "fn parse_queue_choice",
    ] {
        assert!(
            reducer.contains(required),
            "submit_message_reducer.rs should own: {required}"
        );
    }
}

#[test]
fn state_domain_types_live_in_dedicated_module() {
    let main_rs = read("src/main.rs");
    for forbidden in [
        "pub struct SubAgentContext",
        "pub enum MessageType",
        "pub(crate) enum MessageState",
        "pub(crate) enum UIMessageMetadata",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs should not define extracted state/domain type: {forbidden}"
        );
    }

    let state_domain = read("src/state_domain.rs");
    for required in [
        "pub struct SubAgentContext",
        "pub enum MessageType",
        "pub(crate) enum MessageState",
        "pub(crate) enum UIMessageMetadata",
    ] {
        assert!(
            state_domain.contains(required),
            "state_domain.rs should own extracted state/domain type: {required}"
        );
    }
}

#[test]
fn commands_module_does_not_depend_on_ui_or_persistence_io() {
    assert_none_match(
        "src/commands",
        &[
            "ratatui",
            "ui::",
            "persistence::",
            "std::fs",
            "tokio::fs",
            "File::open",
        ],
    );
}

#[test]
fn persistence_module_does_not_depend_on_command_or_ui_logic() {
    assert_none_match(
        "src/persistence",
        &[
            "dispatch_slash_command",
            "parse_slash_command",
            "SlashCommandDispatch",
            "ratatui",
            "ui::",
        ],
    );
}

#[test]
fn ui_module_does_not_parse_slash_commands_or_write_persistence() {
    assert_none_match(
        "src/ui",
        &[
            "dispatch_slash_command",
            "parse_slash_command",
            "SlashCommandDispatch",
            "std::fs",
            "tokio::fs",
            "File::open",
            "OpenOptions",
        ],
    );
}

#[test]
fn spec_cli_and_model_context_keep_narrow_dependencies() {
    let spec_cli = read("src/spec_cli.rs");
    for forbidden in [
        "dispatch_slash_command",
        "parse_slash_command",
        "SlashCommandDispatch",
        "persistence::",
    ] {
        assert!(
            !spec_cli.contains(forbidden),
            "src/spec_cli.rs should not contain `{forbidden}`"
        );
    }

    let model_context = read("src/model_context.rs");
    for forbidden in [
        "ratatui",
        "ui::",
        "dispatch_slash_command",
        "SlashCommandDispatch",
    ] {
        assert!(
            !model_context.contains(forbidden),
            "src/model_context.rs should not contain `{forbidden}`"
        );
    }
}

#[test]
fn message_event_protocol_tokens_stay_in_ui_message_event_module() {
    let main_rs = read("src/main.rs");
    for token in [
        "[THINKING_ANIMATION]",
        "[COMMAND:",
        "[GEN_STATS:",
        "[TOOL_CALL_STARTED:",
        "[TOOL_CALL_COMPLETED:",
    ] {
        assert!(
            !main_rs.contains(token),
            "main.rs should not parse protocol token directly: {token}"
        );
    }

    let events = read("src/ui_message_event.rs");
    for token in [
        "[THINKING_ANIMATION]",
        "[COMMAND:",
        "[GEN_STATS:",
        "[TOOL_CALL_STARTED:",
        "[TOOL_CALL_COMPLETED:",
    ] {
        assert!(
            events.contains(token),
            "ui_message_event.rs should own protocol token: {token}"
        );
    }
}

#[test]
fn prompt_text_stays_in_ui_prompts_module() {
    let main_rs = read("src/main.rs");
    assert!(
        !main_rs.contains("Interrupt and tell Nite what to do"),
        "main.rs should delegate prompt copy to src/ui/prompts.rs"
    );

    let prompts = read("src/ui/prompts.rs");
    assert!(prompts.contains("Interrupt and tell Nite what to do"));
    assert!(prompts.contains("Add "));
    assert!(prompts.contains(" to writable roots?"));
}

#[test]
fn session_lifecycle_mapping_stays_in_dedicated_module() {
    let main_rs = read("src/main.rs");
    assert!(
        !main_rs.contains("fn session_role_from_step_role"),
        "main.rs should not inline session role mapping logic"
    );
    assert!(
        !main_rs.contains("fn step_status_to_session_status"),
        "main.rs should not inline step status mapping logic"
    );

    // Delegation to session_lifecycle now lives in spec_orchestrator_reducer
    let spec_orch = read("src/spec_orchestrator_reducer.rs");
    assert!(
        spec_orch.contains("session_lifecycle::update_session_for_step"),
        "spec_orchestrator_reducer.rs should delegate session updates to session_lifecycle.rs"
    );

    let lifecycle = read("src/session_lifecycle.rs");
    assert!(lifecycle.contains("fn session_role_from_step_role"));
    assert!(lifecycle.contains("fn step_status_to_session_status"));
}

#[test]
fn spec_plan_rendering_logic_stays_in_spec_ui_module() {
    let main_rs = read("src/main.rs");
    assert!(
        !main_rs.contains("No steps in this spec."),
        "main.rs should not own spec plan fallback copy"
    );
    assert!(
        !main_rs.contains("History: no entries yet."),
        "main.rs should not own spec history copy"
    );

    // spec_ui calls now live in spec_orchestrator_reducer
    let spec_orch = read("src/spec_orchestrator_reducer.rs");
    assert!(spec_orch.contains("spec_ui::build_spec_plan_lines"));
    assert!(spec_orch.contains("spec_ui::build_tool_only_plan_lines"));

    let spec_ui = read("src/spec_ui.rs");
    assert!(spec_ui.contains("No steps in this spec."));
    assert!(spec_ui.contains("History: no entries yet."));
}

#[test]
fn survey_feedback_copy_stays_in_survey_module() {
    let main_rs = read("src/main.rs");
    assert!(
        main_rs.contains("self.survey.check_number_input"),
        "main.rs should route numeric answer checks through Survey"
    );
    assert!(
        main_rs.contains("self.survey.show_thank_you"),
        "main.rs should route thank-you state through Survey"
    );
    assert!(
        !main_rs.contains("Thanks for making Nite better"),
        "main.rs should not own survey thank-you copy"
    );
    assert!(
        !main_rs.contains("(use /feedback to give suggestions or bug reports)"),
        "main.rs should not own survey helper copy"
    );

    let survey = read("src/survey.rs");
    assert!(survey.contains("Thanks for making Nite better"));
    assert!(survey.contains("(use /feedback to give suggestions or bug reports)"));
}
