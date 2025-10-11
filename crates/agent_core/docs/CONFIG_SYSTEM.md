# Nite Configuration System

This document explains the new modular configuration system for Nite.

## Overview

The system consists of three main components:

1. **Config Management** (`src/config.rs`) - Handles `.nite` directory and `.niterules` file
2. **Modular Tools** (`src/tools.rs`) - Allows selective tool inclusion
3. **Main Integration** (`src/main.rs`) - Ties everything together

## Directory Structure

When you run the agent, it will automatically create:

```
~/.config/.nite/
├── .niterules          # Global system prompt template
└── models/             # Directory for model files
```

You can also create a local `.niterules` file in your project directory:

```
./
├── .niterules          # Local system prompt (overrides global)
└── ... your project files
```

### Priority Order

The system checks for `.niterules` in this order:

1. **Local** - `./.niterules` (current directory)
2. **Global** - `~/.config/.nite/.niterules` (user config)
3. **Default** - Built-in template (if no files exist)

## .niterules File

The `.niterules` file is a template for the system prompt. It supports placeholders:

- `{tools_section}` - Replaced with tool definitions based on enabled tools
- `{os_version}` - Replaced with the detected OS version
- `{workspace_path}` - Replaced with the current workspace path

### Example .niterules

```
You are a powerful agentic AI coding assistant...

{tools_section}

<user_info>
The user's OS version is {os_version}.
The absolute path of the user's workspace is {workspace_path}.
</user_info>
```

## Modular Tools System

### Basic Usage

```rust
// Get all tools (default)
let tools = tools::get_all_tools();

// Or build a custom subset
let tools = tools::build_tools(&[
    ToolName::ExecCommand,
    ToolName::ReadFile,
    ToolName::SemanticSearch,
]);

// Generate the tools section for the system prompt
let tools_section = tools::generate_tools_section(&tools);
```

### Available Tools

- `ToolName::ExecCommand` - Execute shell commands
- `ToolName::DeletePath` - Delete a file or directory
- `ToolName::DeleteMany` - Delete multiple paths
- `ToolName::GetFiles` - List files in a directory
- `ToolName::GetFilesRecursive` - Recursively list files
- `ToolName::SearchFilesWithRegex` - Regex search in files
- `ToolName::ReadFile` - Read file contents
- `ToolName::SemanticSearch` - Semantic code search

### Use Cases

#### 1. Full Mode (Default)
All tools available for maximum capability:

```rust
let tools = tools::get_all_tools();
// Includes: exec_command, delete_path, delete_many,
//           get_files, get_files_recursive, search_files_with_regex,
//           read_file, semantic_search
```

#### 2. Read-Only Mode
Safe, read-only tools for analysis (no exec, no delete):

```rust
let tools = tools::get_readonly_tools();
// Includes: get_files, get_files_recursive, search_files_with_regex,
//           read_file, semantic_search
```

#### 3. Custom Mode
Build your own tool selection:

```rust
let tools = tools::build_tools(&[
    ToolName::ReadFile,
    ToolName::SemanticSearch,
]);
```

## How It Works

### Initialization Flow

1. **Config Initialization**
   ```rust
   config::initialize_config()?;
   ```
   - Checks if `~/.config/.nite` exists
   - Creates directory if missing
   - Creates `models/` subdirectory
   - Creates `.niterules` with default template if missing

2. **Tool Selection**
   ```rust
   let tools = tools::get_all_tools(); // or custom subset
   ```

3. **System Prompt Generation**
   ```rust
   let tools_section = tools::generate_tools_section(&tools);
   let system_prompt_template = config::read_system_prompt()?;
   let system_prompt = system_prompt_template
       .replace("{tools_section}", &tools_section)
       .replace("{os_version}", &os_version)
       .replace("{workspace_path}", &workspace_path);
   ```

4. **Model Execution**
   ```rust
   let request_builder = RequestBuilder::new()
       .add_message(TextMessageRole::System, &system_prompt)
       .set_tools(tools)
       // ... rest of configuration
   ```

## Customization

### Editing .niterules

You can customize AI interaction at two levels:

#### Global Configuration (affects all projects)
```bash
vim ~/.config/.nite/.niterules
```

#### Project-Specific Configuration (overrides global)
```bash
# Create a local .niterules in your project directory
vim ./.niterules
```

Changes will be picked up on the next run.

**Example Use Cases:**

- **Global .niterules** - Your general preferences, coding style, default behavior
- **Local .niterules** - Project-specific rules, special instructions for this codebase

The local file completely overrides the global one if present.

### Adding New Tools

To add a new tool:

1. Add the tool variant to `ToolName` enum in `src/tools.rs`
2. Implement the tool definition in `build_tool()` match statement
3. Add it to `get_all_tools()` if it should be included by default

Example:

```rust
// In src/tools.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    // ... existing tools
    MyNewTool,
}

pub fn build_tool(tool_name: ToolName) -> Tool {
    match tool_name {
        // ... existing tools
        ToolName::MyNewTool => Tool {
            tp: ToolType::Function,
            function: Function {
                name: "my_new_tool".to_string(),
                description: Some("Description of my tool".to_string()),
                parameters: Some({
                    let mut params = HashMap::new();
                    // ... define parameters
                    params
                }),
            },
        },
    }
}
```

## Migration from Old System

The old system had hardcoded tools and system prompt in `main.rs`. The new system:

- ✅ Moves configuration to user-editable files
- ✅ Allows tool subset selection
- ✅ Supports dynamic prompt generation
- ✅ Maintains backward compatibility

Old code has been removed from `main.rs` and replaced with modular calls.

## Benefits

1. **User Customization** - Users can edit `.niterules` without recompiling
2. **Modular Tools** - Easy to enable/disable tools based on context
3. **Maintainability** - Tools are defined in one place (`tools.rs`)
4. **Flexibility** - Different modes can have different tool sets
5. **Separation of Concerns** - Config, tools, and business logic are separate

## Future Enhancements

Potential improvements:

- [ ] Support for multiple `.niterules` profiles (e.g., `.niterules.dev`, `.niterules.safe`)
- [ ] Tool capability flags (read-only, write, network, etc.)
- [ ] Per-tool configuration in separate files
- [ ] Tool usage analytics and logging
- [ ] Dynamic tool loading from plugins
