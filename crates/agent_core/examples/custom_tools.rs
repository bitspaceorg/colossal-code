/// Example showing how to use custom tool subsets
///
/// This demonstrates how to create different tool configurations
/// for different contexts (e.g., "tools mode" with limited tools)
use std::collections::HashMap;

// This would normally come from the tools module
#[derive(Debug, Clone, Copy)]
enum ToolName {
    ExecCommand,
    ReadFile,
    SemanticSearch,
    // ... other tools
}

fn main() {
    println!("=== Example: Using Modular Tools ===\n");

    // Example 1: Full tool set (default)
    println!("1. Full tool set (all tools available):");
    let all_tools = vec![
        ToolName::ExecCommand,
        ToolName::ReadFile,
        ToolName::SemanticSearch,
    ];
    println!("   Tools: {:?}\n", all_tools);

    // Example 2: Tools mode (limited to safe tools only)
    println!("2. Tools mode (safe read-only tools):");
    let tools_mode = vec![ToolName::ReadFile, ToolName::SemanticSearch];
    println!("   Tools: {:?}\n", tools_mode);

    // Example 3: Custom selection based on user preference
    println!("3. Custom tool selection:");
    let custom_tools = vec![ToolName::ExecCommand, ToolName::ReadFile];
    println!("   Tools: {:?}\n", custom_tools);

    println!("\n=== How to Use ===");
    println!("In your main.rs, you would:");
    println!("1. Decide which tools to enable based on context/mode");
    println!("2. Use tools::build_tools(&[ToolName::Foo, ToolName::Bar])");
    println!("3. Generate system prompt with tools::generate_tools_section(&tools)");
    println!("4. The .niterules template will have {{tools_section}} replaced");
}
