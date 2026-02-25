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
        "mod commands;",
        "mod model_context;",
        "mod spec_cli;",
        "mod ui;",
        "mod ui_message_event;",
        "pub mod spec_ui;",
        "dispatch_slash_command",
        "model_context::detect_context_length",
        "spec_ui::build_spec_plan_lines",
        "ui::prompts::render_approval_prompt",
        "ui::prompts::render_sandbox_prompt",
        "UiMessageEvent::parse",
    ] {
        assert!(
            main_rs.contains(required),
            "main.rs should keep module boundary wiring for: {required}"
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
