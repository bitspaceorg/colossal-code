# Quick Reference - Nite Configuration System

## .niterules Priority

```
1. ./.niterules              (local project - HIGHEST)
   ↓ not found
2. ~/.config/.nite/.niterules (global user)
   ↓ not found
3. Built-in default          (fallback)
```

## File Locations

| File | Location | Purpose |
|------|----------|---------|
| Local .niterules | `./.niterules` | Project-specific AI instructions |
| Global .niterules | `~/.config/.nite/.niterules` | Your default AI preferences |
| Models directory | `~/.config/.nite/models/` | Store model files |

## Quick Commands

```bash
# Edit global .niterules
vim ~/.config/.nite/.niterules

# Create local .niterules (overrides global)
vim ./.niterules

# Check which config is active (run your agent and look for output):
# "Using local .niterules from: ./.niterules"
# OR
# "Using global .niterules from: /home/user/.config/.nite/.niterules"

# View config directory
ls -la ~/.config/.nite/
```

## Template Placeholders

| Placeholder | Replaced With |
|-------------|---------------|
| `{tools_section}` | Generated XML tool definitions |
| `{os_version}` | Detected operating system version |
| `{workspace_path}` | Current working directory path |

## Tool Modes (in code)

```rust
// Full mode (all tools - read and write)
let tools = tools::get_all_tools();

// Read-only mode (safe - no exec, no delete)
let tools = tools::get_readonly_tools();

// Custom selection
let tools = tools::build_tools(&[
    ToolName::ExecCommand,
    ToolName::ReadFile,
]);
```

### Tool Modes Explained

| Mode | Function | Tools Included |
|------|----------|----------------|
| **Full Mode** | `get_all_tools()` | All 8 tools (exec, delete, read, search) |
| **Read-Only Mode** | `get_readonly_tools()` | 5 tools (get_files, get_files_recursive, search_files_with_regex, read_file, semantic_search) |
| **Custom** | `build_tools(&[...])` | Your selection |

## Available Tools

- `ToolName::ExecCommand` - Execute shell commands
- `ToolName::DeletePath` - Delete a file/directory
- `ToolName::DeleteMany` - Delete multiple paths
- `ToolName::GetFiles` - List files in directory
- `ToolName::GetFilesRecursive` - Recursively list files
- `ToolName::SearchFilesWithRegex` - Regex search
- `ToolName::ReadFile` - Read file contents
- `ToolName::SemanticSearch` - Semantic code search

## Common Use Cases

### Global Preferences
**Edit**: `~/.config/.nite/.niterules`
**Use for**: Your general coding style, default behavior, common rules

### Project-Specific Rules
**Create**: `./.niterules` in your project
**Use for**: Special instructions for this codebase, project conventions

### Examples

#### Prefer Global When:
- You want consistent behavior across all projects
- Setting up general coding standards
- Defining your personal AI preferences

#### Prefer Local When:
- Working on a unique project with special requirements
- Need different tools/behavior for this specific codebase
- Collaborating and want to share project AI rules (commit .niterules)

## Workflow

1. **First time**: System creates `~/.config/.nite/.niterules` automatically
2. **Customize global**: Edit `~/.config/.nite/.niterules` for your preferences
3. **Override for project**: Create `./.niterules` when needed
4. **Run agent**: It will automatically use the right config
5. **Check output**: Look for "Using local/global .niterules from..."

## Debugging

```bash
# Check if local .niterules exists
test -f ./.niterules && echo "Local exists" || echo "No local"

# Check if global .niterules exists
test -f ~/.config/.nite/.niterules && echo "Global exists" || echo "No global"

# View current local .niterules
cat ./.niterules

# View global .niterules
cat ~/.config/.nite/.niterules

# Compare local vs global
diff ./.niterules ~/.config/.nite/.niterules
```

## Tips

✅ **DO**:
- Use global for general preferences
- Use local for project-specific rules
- Commit `.niterules` to your repo if you want to share project AI configuration
- Test changes by running the agent

❌ **DON'T**:
- Don't forget placeholders `{tools_section}`, `{os_version}`, `{workspace_path}`
- Don't edit both files and wonder why only local is used (priority!)
- Don't hardcode paths or system-specific info (use placeholders)

## Migration from Old System

Old: System prompt was hardcoded in `main.rs`
New: System prompt is in `.niterules` files

**No action needed** - default template matches old behavior!
