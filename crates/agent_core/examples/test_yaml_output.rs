use agent_core::{Agent, AgentMessage};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Testing YAML output from agent tools...\n");

    // Create agent with defaults
    let agent = Agent::new_with_defaults().await?;
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Test 1: get_files tool
    println!("=== Test 1: get_files tool ===");
    let test_message = "Please list the files in the current directory using get_files tool with path '.' and limit 5";

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        let _ = agent.process_message(test_message.to_string(), tx_clone).await;
    });

    // Collect tool results
    let mut tool_results = Vec::new();
    let mut done = false;

    while !done {
        if let Some(msg) = rx.recv().await {
            match msg {
                AgentMessage::ToolCallStarted(tool_name) => {
                    println!("Tool called: {}", tool_name);
                },
                AgentMessage::ToolCallCompleted(tool_name, result) => {
                    println!("\nTool: {}", tool_name);
                    println!("Result format check:");

                    // Check if it's YAML (starts with '-' or has key: value patterns)
                    let is_yaml = result.contains(":\n") ||
                                  result.starts_with('-') ||
                                  result.contains("- name:") ||
                                  result.contains("status:") ||
                                  (result.contains(':') && result.contains('\n'));

                    // Check if it's JSON (starts with '{' or '[')
                    let is_json = result.trim().starts_with('{') || result.trim().starts_with('[');

                    if is_yaml {
                        println!("✅ Output is YAML format");
                    } else if is_json {
                        println!("❌ Output is JSON format (should be YAML!)");
                    } else {
                        println!("⚠️  Output format unclear");
                    }

                    println!("\nFirst 500 chars of output:");
                    println!("{}", result.chars().take(500).collect::<String>());
                    println!("\n{}", "=".repeat(60));

                    tool_results.push((tool_name, result));
                },
                AgentMessage::Done => {
                    done = true;
                    println!("\n✅ Agent finished processing");
                },
                AgentMessage::Error(err) => {
                    println!("❌ Error: {}", err);
                    done = true;
                },
                _ => {}
            }
        }
    }

    // Summary
    println!("\n=== Summary ===");
    println!("Total tools called: {}", tool_results.len());
    for (tool, result) in &tool_results {
        let format = if result.trim().starts_with('{') || result.trim().starts_with('[') {
            "JSON ❌"
        } else if result.contains(":\n") || result.starts_with('-') {
            "YAML ✅"
        } else {
            "UNKNOWN ⚠️"
        };
        println!("{}: {}", tool, format);
    }

    Ok(())
}
