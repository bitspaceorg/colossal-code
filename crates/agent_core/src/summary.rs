use crate::{Agent, shell_session};
use agent_protocol::types::message::Role;
use agent_protocol::types::spec::{
    SpecSheet, SpecStep, StepStatus, TaskSummary, TaskVerification, TestRun, VerificationStatus,
};
use agent_protocol::types::task::Task;
use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use std::process::Command;

pub fn build_summary(task: &Task, artifact_override: Option<&[String]>) -> TaskSummary {
    let (step_index, instructions) = extract_step_context(task);
    let artifacts = gather_artifacts(artifact_override);
    let commands = extract_tool_commands(task);
    let agent_response = extract_agent_message(task);
    let command_summary = if commands.is_empty() {
        "none".to_string()
    } else {
        commands.join(" | ")
    };
    let artifact_summary = if artifacts.is_empty() {
        "none".to_string()
    } else {
        artifacts.join(", ")
    };

    let summary_text = format!(
        "Step {step_index} summary:\nInstructions: {instructions}\nCommands: {command_summary}\nArtifacts: {artifact_summary}\nAgent result: {agent_response}"
    );

    TaskSummary {
        task_id: task.id.clone(),
        step_index,
        summary_text,
        artifacts_touched: artifacts,
        tests_run: Vec::<TestRun>::new(),
        verification: TaskVerification {
            status: VerificationStatus::Pending,
            feedback: vec![],
        },
        worktree: None,
    }
}

fn gather_artifacts(artifact_override: Option<&[String]>) -> Vec<String> {
    if let Some(values) = artifact_override {
        return values.iter().map(|value| value.to_string()).collect();
    }
    gather_git_changes()
}

fn gather_git_changes() -> Vec<String> {
    match Command::new("git").args(["status", "--short"]).output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    let path: String = trimmed.chars().skip(3).collect::<String>();
                    let path = path.trim();
                    if path.is_empty() {
                        None
                    } else {
                        Some(path.to_string())
                    }
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn extract_tool_commands(task: &Task) -> Vec<String> {
    task.metadata
        .as_ref()
        .and_then(|metadata| metadata.extra.get("toolLog"))
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("tool");
                    let args = entry
                        .get("arguments")
                        .and_then(|value| value.as_str())
                        .unwrap_or("{}");
                    let result = entry
                        .get("result")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let mut line = format!("{} -> {}", name, args);
                    if !result.is_empty() {
                        line.push_str(&format!(" = {}", result));
                    }
                    Some(line)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_agent_message(task: &Task) -> String {
    task.messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Agent)
        .map(|message| message.text_content())
        .unwrap_or_else(|| "No agent response".to_string())
}

fn extract_step_context(task: &Task) -> (String, String) {
    if let Some(metadata) = &task.metadata {
        let index = metadata
            .extra
            .get("stepIndex")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let instructions = metadata
            .extra
            .get("stepInstructions")
            .and_then(|value| value.as_str())
            .unwrap_or("(no instructions)")
            .to_string();
        return (index, instructions);
    }
    ("unknown".to_string(), "(no instructions)".to_string())
}

pub fn build_split_spec(step: &SpecStep) -> Result<SpecSheet> {
    let created_at = Utc::now();
    let mut child_steps: Vec<SpecStep> = step
        .instructions
        .split(|ch| ch == '\n')
        .map(|fragment| fragment.trim())
        .filter(|fragment| !fragment.is_empty())
        .enumerate()
        .map(|(idx, fragment)| SpecStep {
            index: (idx + 1).to_string(),
            title: format!("{} - subtask {}", step.title, idx + 1),
            instructions: fragment.to_string(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        })
        .collect();

    if child_steps.is_empty() {
        child_steps.push(SpecStep {
            index: "1".to_string(),
            title: format!("{} - detail", step.title),
            instructions: step.instructions.clone(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        });
    }

    let child_spec = SpecSheet {
        id: format!("{}::split", step.index),
        title: format!("{} (split)", step.title),
        description: format!("Split from parent step {}", step.index),
        steps: child_steps,
        created_by: "nite-agent".to_string(),
        created_at,
        metadata: json!({"source": "split"}),
    };

    child_spec.validate()?;
    Ok(child_spec)
}

pub fn build_split_summary(task: &Task, step: &SpecStep, child: &SpecSheet) -> TaskSummary {
    TaskSummary {
        task_id: task.id.clone(),
        step_index: step.index.clone(),
        summary_text: format!(
            "Step {} split into spec {} with {} steps",
            step.index,
            child.id,
            child.steps.len()
        ),
        artifacts_touched: Vec::new(),
        tests_run: Vec::new(),
        verification: TaskVerification {
            status: VerificationStatus::Pending,
            feedback: vec![],
        },
        worktree: None,
    }
}

pub fn build_spec_from_goal(goal: &str) -> Result<SpecSheet> {
    let created_at = Utc::now();
    let spec_id = format!("spec-{}", created_at.timestamp_millis());

    let lines: Vec<&str> = goal
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    let steps: Vec<SpecStep> = if lines.is_empty() {
        vec![SpecStep {
            index: "1".to_string(),
            title: truncate_title(goal, 50),
            instructions: goal.to_string(),
            acceptance_criteria: Vec::new(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: Vec::new(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        }]
    } else if lines.len() == 1 {
        decompose_goal_into_steps(goal)
    } else {
        lines
            .iter()
            .enumerate()
            .map(|(idx, line)| SpecStep {
                index: (idx + 1).to_string(),
                title: truncate_title(line, 50),
                instructions: line.to_string(),
                acceptance_criteria: Vec::new(),
                required_tools: Vec::new(),
                constraints: Vec::new(),
                dependencies: if idx > 0 {
                    vec![idx.to_string()]
                } else {
                    Vec::new()
                },
                is_parallel: false,
                requires_verification: true,
                max_parallelism: None,
                status: StepStatus::Pending,
                sub_spec: None,
                completed_at: None,
            })
            .collect()
    };

    let title = if lines.is_empty() {
        truncate_title(goal, 80)
    } else {
        truncate_title(lines[0], 80)
    };

    let spec = SpecSheet {
        id: spec_id,
        title,
        description: goal.to_string(),
        steps,
        created_by: "cli".to_string(),
        created_at,
        metadata: json!({}),
    };

    spec.validate().map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(spec)
}

fn truncate_title(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}

fn decompose_goal_into_steps(goal: &str) -> Vec<SpecStep> {
    let goal_lower = goal.to_lowercase();
    let mut steps = Vec::new();

    fn make_step(
        index: usize,
        title: &str,
        instructions: &str,
        criteria: Vec<&str>,
        deps: Vec<usize>,
    ) -> SpecStep {
        SpecStep {
            index: index.to_string(),
            title: title.to_string(),
            instructions: instructions.to_string(),
            acceptance_criteria: criteria.iter().map(|s| s.to_string()).collect(),
            required_tools: Vec::new(),
            constraints: Vec::new(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            is_parallel: false,
            requires_verification: true,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        }
    }

    steps.push(make_step(
        1,
        "Initialize project structure",
        &format!("Set up the project structure for: {}\n\nCreate necessary directories, initialize Cargo.toml with required dependencies, and set up the basic module structure.", goal),
        vec!["Project compiles with `cargo check`", "All dependencies declared in Cargo.toml"],
        vec![],
    ));

    steps.push(make_step(
        2,
        "Define core data models",
        "Create the core data structures and types needed for the application. Define structs, enums, and implement basic traits (Debug, Clone, Serialize/Deserialize as needed).",
        vec!["All core types defined", "Types implement required traits"],
        vec![1],
    ));

    let mut next_idx = 3;

    if goal_lower.contains("sqlite")
        || goal_lower.contains("database")
        || goal_lower.contains("storage")
        || goal_lower.contains("persist")
    {
        steps.push(make_step(
            next_idx,
            "Implement storage layer",
            "Create the database/storage layer. Set up SQLite connection, define schema, implement CRUD operations for all entities.",
            vec!["Database schema created", "CRUD operations work correctly", "Data persists across restarts"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    steps.push(make_step(
        next_idx,
        "Implement core functionality",
        &format!("Implement the main business logic for: {}\n\nThis includes all core operations and algorithms.", goal),
        vec!["Core features work as specified", "Error handling is proper"],
        vec![next_idx - 1],
    ));
    next_idx += 1;

    if goal_lower.contains("cli")
        || goal_lower.contains("command")
        || goal_lower.contains("terminal")
    {
        let colored = if goal_lower.contains("color") {
            " with colored output"
        } else {
            ""
        };
        steps.push(make_step(
            next_idx,
            "Build CLI interface",
            &format!("Create the command-line interface{}. Parse arguments, implement subcommands, format output nicely.", colored),
            vec!["CLI parses all required commands", "Help text is clear", "Output is well-formatted"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    if goal_lower.contains("test") {
        steps.push(make_step(
            next_idx,
            "Write tests",
            "Write comprehensive tests for the application. Include unit tests for core logic and integration tests for the full workflow.",
            vec!["All tests pass", "Core functionality is covered", "Edge cases are tested"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    if goal_lower.contains("readme")
        || goal_lower.contains("documentation")
        || goal_lower.contains("doc")
    {
        steps.push(make_step(
            next_idx,
            "Create documentation",
            "Write README.md with installation instructions, usage examples, and feature documentation.",
            vec!["README is complete", "Examples are clear", "Installation steps work"],
            vec![next_idx - 1],
        ));
        next_idx += 1;
    }

    steps.push(make_step(
        next_idx,
        "Final integration and polish",
        "Ensure all components work together. Run full test suite, fix any issues, clean up code.",
        vec![
            "All tests pass",
            "cargo clippy has no warnings",
            "Application works end-to-end",
        ],
        vec![next_idx - 1],
    ));

    steps
}
