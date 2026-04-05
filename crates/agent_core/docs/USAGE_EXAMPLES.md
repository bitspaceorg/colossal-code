# Usage Examples

## Example 1: Using All Tools (Default)

```rust
use crate::tools;
use crate::config;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize config (creates ~/.config/.nite if needed)
    config::initialize_config()?;

    // Get all available tools
    let tools = tools::get_all_tools();

    // Read and prepare system prompt
    let tools_section = tools::generate_tools_section(&tools);
    let system_prompt_template = config::read_system_prompt()?;
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", "Linux")
        .replace("{workspace_path}", "/home/user/project");

    // Use with your model...
    let request_builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, &system_prompt)
        .set_tools(tools);

    // ... rest of your code
}
```

## Example 2: Read-Only Mode (Safe, No Modifications)

Perfect for when you want the AI to analyze code without making changes:

```rust
use crate::tools;
use crate::config;

#[tokio::main]
async fn main() -> Result<()> {
    config::initialize_config()?;

    // Only allow safe, read-only tools
    // This gives you: get_files, get_files_recursive, search_files_with_regex, read_file, semantic_search
    let tools = tools::get_readonly_tools();

    let tools_section = tools::generate_tools_section(&tools);
    let system_prompt_template = config::read_system_prompt()?;
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", "Linux")
        .replace("{workspace_path}", "/home/user/project");

    let request_builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, &system_prompt)
        .set_tools(tools);

    // ... rest of your code
}
```

## Example 3: Minimal Mode (Just Search)

For semantic search only:

```rust
use crate::tools::{self, ToolName};
use crate::config;

#[tokio::main]
async fn main() -> Result<()> {
    config::initialize_config()?;

    // Only semantic search
    let tools = tools::build_tools(&[
        ToolName::SemanticSearch,
    ]);

    let tools_section = tools::generate_tools_section(&tools);
    let system_prompt_template = config::read_system_prompt()?;
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", "Linux")
        .replace("{workspace_path}", "/home/user/project");

    let request_builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, &system_prompt)
        .set_tools(tools);

    // ... rest of your code
}
```

## Example 4: Execution Mode (Commands + Reading)

For when you want to execute commands and read files:

```rust
use crate::tools::{self, ToolName};
use crate::config;

#[tokio::main]
async fn main() -> Result<()> {
    config::initialize_config()?;

    let tools = tools::build_tools(&[
        ToolName::ExecCommand,
        ToolName::ReadFile,
        ToolName::GetFiles,
    ]);

    let tools_section = tools::generate_tools_section(&tools);
    let system_prompt_template = config::read_system_prompt()?;
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", "Linux")
        .replace("{workspace_path}", "/home/user/project");

    let request_builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, &system_prompt)
        .set_tools(tools);

    // ... rest of your code
}
```

## Example 5: Dynamic Tool Selection Based on Mode

```rust
use crate::tools::{self, ToolName};
use crate::config;

enum Mode {
    Full,
    ReadOnly,
    SearchOnly,
}

fn get_tools_for_mode(mode: Mode) -> Vec<Tool> {
    match mode {
        Mode::Full => tools::get_all_tools(),
        Mode::ReadOnly => tools::get_readonly_tools(),
        Mode::SearchOnly => tools::build_tools(&[
            ToolName::SemanticSearch,
        ]),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    config::initialize_config()?;

    // Select mode based on environment variable or CLI flag
    let mode = match std::env::var("AGENT_MODE").as_deref() {
        Ok("readonly") => Mode::ReadOnly,
        Ok("search") => Mode::SearchOnly,
        _ => Mode::Full,
    };

    let tools = get_tools_for_mode(mode);
    let tools_section = tools::generate_tools_section(&tools);
    let system_prompt_template = config::read_system_prompt()?;
    let system_prompt = system_prompt_template
        .replace("{tools_section}", &tools_section)
        .replace("{os_version}", "Linux")
        .replace("{workspace_path}", "/home/user/project");

    let request_builder = RequestBuilder::new()
        .add_message(TextMessageRole::System, &system_prompt)
        .set_tools(tools);

    // ... rest of your code
}
```

## Example 6: Customizing .niterules

The system supports two-level configuration:

### Global Configuration (All Projects)

Edit the global configuration that applies to all projects:

```bash
# Open in your editor
vim ~/.config/.nite/.niterules

# Or use nano
nano ~/.config/.nite/.niterules

# Or use VS Code
code ~/.config/.nite/.niterules
```

### Project-Specific Configuration (Override Global)

Create a local `.niterules` in your project to override global settings:

```bash
# Create local .niterules in current project
vim ./.niterules

# This will override ~/.config/.nite/.niterules for this project only
```

### Example Global .niterules

Your general preferences:

```
You are a powerful agentic AI coding assistant...

<custom_behavior>
When reviewing code, always check for:
1. Security vulnerabilities
2. Performance issues
3. Code style consistency
</custom_behavior>

{tools_section}

<user_info>
The user's OS version is {os_version}.
The absolute path of the user's workspace is {workspace_path}.
</user_info>
```

### Example Local .niterules

Project-specific instructions:

```
You are working on a Rust TUI application using ratatui.

<project_specific>
This is a terminal user interface for an AI coding agent.
- Follow Rust best practices and idiomatic patterns
- Use tokio for async operations
- Ensure all UI updates are smooth and responsive
- Test all terminal rendering before suggesting changes
</project_specific>

{tools_section}

<user_info>
The user's OS version is {os_version}.
The absolute path of the user's workspace is {workspace_path}.
</user_info>
```

### Priority System

The system checks in this order:

1. `./.niterules` (current directory) - **highest priority**
2. `~/.config/.nite/.niterules` (user home)
3. Built-in default template - **fallback**

You'll see which file is being used when you start:

```
Using local .niterules from: ./.niterules
```

or

```
Using global .niterules from: /home/user/.config/.nite/.niterules
```

or

```
No .niterules file found, using default template
```

## Example 7: Checking What Tools Are Available

```rust
use crate::tools::ToolName;

// Print all available tool names
println!("Available tools:");
for tool_name in [
    ToolName::ExecCommand,
    ToolName::DeletePath,
    ToolName::DeleteMany,
    ToolName::GetFiles,
    ToolName::GetFilesRecursive,
    ToolName::SearchFilesWithRegex,
    ToolName::ReadFile,
    ToolName::SemanticSearch,
] {
    println!("  - {:?}", tool_name);
}
```

## Example 8: Environment-Based Configuration

```rust
use crate::tools;
use crate::config;

#[tokio::main]
async fn main() -> Result<()> {
    config::initialize_config()?;

    // Check if we're in a restricted environment
    let is_restricted = std::env::var("READONLY_MODE").is_ok();

    let tools = if is_restricted {
        // Read-only mode - no exec, no delete
        println!("Running in READ-ONLY mode");
        tools::get_readonly_tools()
    } else {
        // Full access - all tools available
        println!("Running in FULL mode");
        tools::get_all_tools()
    };

    println!("Loaded {} tools", tools.len());

    let tools_section = tools::generate_tools_section(&tools);
    // ... rest of initialization
}
```

Run with:

```bash
# Read-only mode
READONLY_MODE=1 cargo run --bin tool_agent

# Full mode (default)
cargo run --bin tool_agent
```

## Testing Your Configuration

```bash
# Run with default tools
cargo run --bin tool_agent

# Run in read-only mode
RESTRICTED_MODE=1 cargo run --bin tool_agent

# Run in specific mode
AGENT_MODE=search cargo run --bin tool_agent

# Check what config was created
ls -la ~/.config/.nite/
cat ~/.config/.nite/.niterules
```

## Troubleshooting

### .niterules Not Found

If you get an error about .niterules not found, it means initialization failed:

```rust
// Manually initialize
if let Err(e) = config::initialize_config() {
    eprintln!("Failed to initialize config: {}", e);
    // Falls back to default template
}
```

### Tools Not Working

Make sure you're passing the tools to both places:

```rust
let tools = tools::get_all_tools();

// 1. Generate the section for the prompt
let tools_section = tools::generate_tools_section(&tools);

// 2. Pass tools to the request builder
let request_builder = RequestBuilder::new()
    .set_tools(tools)  // ← Don't forget this!
```

### Custom .niterules Not Loading

Check file permissions and path:

```bash
# Check it exists
test -f ~/.config/.nite/.niterules && echo "Found" || echo "Not found"

# Check permissions
ls -l ~/.config/.nite/.niterules

# Test reading it
cat ~/.config/.nite/.niterules
```
