# Nite Configuration System - README

## TL;DR

- ✅ Config checks `./.niterules` **first** (local project)
- ✅ Falls back to `~/.config/.nite/.niterules` (global)
- ✅ Uses built-in default if neither exists
- ✅ Tools are modular - enable/disable based on mode
- ✅ Models stored in `~/.config/.nite/models/`

## What Changed?

### Before
- System prompt hardcoded in `main.rs` (~300 lines)
- All tools always enabled
- No user customization without recompiling

### After
- System prompt in `.niterules` files (user-editable)
- Tools are modular and selectable
- Two-level config: global + project-local
- Clear priority system

## Quick Start

### 1. First Run
The system auto-creates everything you need:
```
~/.config/.nite/
├── .niterules    # Your global config (auto-created)
└── models/       # Put your models here
```

### 2. Customize Globally (Optional)
```bash
vim ~/.config/.nite/.niterules
```
This affects all projects.

### 3. Override for a Project (Optional)
```bash
cd /path/to/your/project
vim ./.niterules
```
This only affects this project.

### 4. Run the Agent
```bash
cargo run --bin tool_agent
```

You'll see:
```
Using local .niterules from: ./.niterules
```
or
```
Using global .niterules from: /home/user/.config/.nite/.niterules
```

## Priority System Explained

```
                           START
                             |
                             v
                    Check ./.niterules
                             |
                    +---------+---------+
                    |                   |
                 EXISTS              DOESN'T EXIST
                    |                   |
                    v                   v
              USE LOCAL      Check ~/.config/.nite/.niterules
                                        |
                                +-------+-------+
                                |               |
                             EXISTS       DOESN'T EXIST
                                |               |
                                v               v
                          USE GLOBAL      USE DEFAULT
```

## File Structure

```
Your Project/
├── .niterules          # Local config (optional, highest priority)
├── src/
└── ...

~/.config/.nite/
├── .niterules          # Global config (fallback)
└── models/             # Your model files
    └── Qwen3-4B.gguf
```

## Template System

Your `.niterules` file supports placeholders:

```
You are an AI assistant...

{tools_section}          # ← Replaced with tool definitions

<user_info>
OS: {os_version}        # ← Replaced with detected OS
Workspace: {workspace_path}  # ← Replaced with current dir
</user_info>
```

## Modular Tools

### In Your Code

```rust
// Full mode - all 8 tools (read + write)
let tools = tools::get_all_tools();

// Read-only mode - 5 safe tools (no exec, no delete)
let tools = tools::get_readonly_tools();

// Custom mode - pick your own
let tools = tools::build_tools(&[
    ToolName::ExecCommand,
    ToolName::ReadFile,
]);

// Then use them
let tools_section = tools::generate_tools_section(&tools);
let system_prompt = template.replace("{tools_section}", &tools_section);
```

### Tool Modes

| Mode | Function | Tools Count | Can Modify Files? |
|------|----------|-------------|-------------------|
| **Full** | `get_all_tools()` | 8 | ✅ Yes (exec, delete) |
| **Read-Only** | `get_readonly_tools()` | 5 | ❌ No (safe analysis) |
| **Custom** | `build_tools(&[...])` | Your choice | Depends on selection |

### Available Tools

| Tool | What It Does |
|------|--------------|
| `ExecCommand` | Run shell commands |
| `DeletePath` | Delete a file/directory |
| `DeleteMany` | Delete multiple paths |
| `GetFiles` | List files in a directory |
| `GetFilesRecursive` | Recursively list files |
| `SearchFilesWithRegex` | Search with regex |
| `ReadFile` | Read file contents |
| `SemanticSearch` | Semantic code search |

## Use Cases

### Global Config (`~/.config/.nite/.niterules`)
**Use for:**
- Your general coding style preferences
- Default behavior across all projects
- Personal AI interaction preferences

**Example:**
```
You are a coding assistant. Always:
- Prefer functional programming
- Write comprehensive tests
- Follow DRY principles
{tools_section}
```

### Local Config (`./.niterules`)
**Use for:**
- Project-specific requirements
- Special coding conventions for this repo
- Team-shared AI instructions (commit to repo)

**Example:**
```
You are working on a Rust TUI application.
- Use ratatui for all UI
- Follow async/await patterns
- Test on multiple terminal emulators
{tools_section}
```

## Common Workflows

### Scenario 1: New User
1. Run agent → auto-creates `~/.config/.nite/.niterules`
2. Edit global config if desired
3. Done!

### Scenario 2: Project-Specific Needs
1. `cd /path/to/special/project`
2. `vim ./.niterules` → customize for this project
3. Run agent → uses local config
4. `cd /other/project` → uses global config again

### Scenario 3: Sharing with Team
1. Create `.niterules` in project root
2. Commit it to git
3. Team members get consistent AI behavior
4. Individual devs can still override with `~/.config/.nite/.niterules`

## Debugging

### Check Which Config Is Active
Run your agent and look for output:
```
Using local .niterules from: ./.niterules
```

### View Current Config
```bash
# Local
cat ./.niterules

# Global
cat ~/.config/.nite/.niterules
```

### Test Priority
```bash
# Create local to test override
echo "Local config test" > ./.niterules

# Run agent - should say "Using local .niterules"

# Remove local to test fallback
rm ./.niterules

# Run agent - should say "Using global .niterules"
```

### Compare Configs
```bash
diff ./.niterules ~/.config/.nite/.niterules
```

## Migration Notes

If you're migrating from the old hardcoded system:

✅ **No action required!**
- Default template matches old hardcoded prompt
- All tools available by default
- Backward compatible

But now you can:
- Edit `.niterules` instead of recompiling
- Use different tool sets for different modes
- Have project-specific AI configurations

## Advanced: Different Modes

```rust
// In main.rs or wherever you initialize

// Read-only mode (safe analysis)
let tools = if std::env::var("READONLY_MODE").is_ok() {
    tools::build_tools(&[
        ToolName::ReadFile,
        ToolName::SemanticSearch,
        ToolName::GetFiles,
    ])
} else {
    // Full mode
    tools::get_all_tools()
};
```

Then:
```bash
# Run in read-only mode
READONLY_MODE=1 cargo run --bin tool_agent

# Run in full mode
cargo run --bin tool_agent
```

## Documentation Files

- **CONFIG_SYSTEM.md** - Detailed technical documentation
- **CHANGES.md** - Summary of what changed
- **USAGE_EXAMPLES.md** - 8+ code examples
- **QUICK_REFERENCE.md** - Quick lookup guide
- **README_CONFIG.md** - This file (overview)

## Help

**Q: Can I use both local and global?**
A: Local completely overrides global. Pick one per project.

**Q: Should I commit .niterules to git?**
A: For project-specific rules, yes! For personal preferences, no (use global).

**Q: What if I mess up the template?**
A: Delete it and run again - system will recreate with defaults.

**Q: How do I add a new tool?**
A: See CONFIG_SYSTEM.md section "Adding New Tools"

**Q: Can I have different .niterules per branch?**
A: Yes! It's just a file, so it can be branch-specific.

## Summary

This system gives you:
1. **Flexibility** - Configure AI per-project or globally
2. **Modularity** - Choose which tools to enable
3. **Convenience** - No recompiling to change behavior
4. **Clarity** - System tells you which config it's using
5. **Backward Compatibility** - Works like before if you don't customize

Enjoy coding with Nite! 🚀
