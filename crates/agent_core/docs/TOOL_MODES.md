# Tool Modes Reference

## Two Main Modes

### 1. Full Mode (Default)
**All tools enabled** - AI can read AND modify files

```rust
let tools = tools::get_all_tools();
```

**Tools included (8 total):**
- ✅ `exec_command` - Execute shell commands
- ✅ `delete_path` - Delete a file or directory
- ✅ `delete_many` - Delete multiple paths
- ✅ `get_files` - List files in directory
- ✅ `get_files_recursive` - Recursively list files
- ✅ `search_files_with_regex` - Search with regex
- ✅ `read_file` - Read file contents
- ✅ `semantic_search` - Semantic code search

**Use when:**
- You want full AI capabilities
- You trust the AI to make changes
- You're actively developing and want assistance

---

### 2. Read-Only Mode
**Safe tools only** - AI can analyze but NOT modify files

```rust
let tools = tools::get_readonly_tools();
```

**Tools included (5 total):**
- ✅ `get_files` - List files in directory
- ✅ `get_files_recursive` - Recursively list files
- ✅ `search_files_with_regex` - Search with regex
- ✅ `read_file` - Read file contents
- ✅ `semantic_search` - Semantic code search

**Excluded (for safety):**
- ❌ `exec_command` - Cannot execute commands
- ❌ `delete_path` - Cannot delete files
- ❌ `delete_many` - Cannot delete multiple files

**Use when:**
- You want code analysis only
- You're reviewing or understanding code
- You want a safe mode with no modifications
- You're sharing the agent in a restricted environment

---

## Quick Comparison

| Feature | Full Mode | Read-Only Mode |
|---------|-----------|----------------|
| **Function** | `get_all_tools()` | `get_readonly_tools()` |
| **Tool Count** | 8 | 5 |
| **Can Read Files** | ✅ Yes | ✅ Yes |
| **Can Search Code** | ✅ Yes | ✅ Yes |
| **Can Execute Commands** | ✅ Yes | ❌ No |
| **Can Delete Files** | ✅ Yes | ❌ No |
| **Can Modify Files** | ✅ Yes (via exec) | ❌ No |
| **Safe for Review** | ⚠️ Caution | ✅ Yes |

---

## Implementation Examples

### Example 1: Simple Mode Selection

```rust
use crate::tools;

// Full mode
let tools = tools::get_all_tools();

// OR

// Read-only mode
let tools = tools::get_readonly_tools();
```

### Example 2: Environment Variable

```rust
use crate::tools;

let tools = if std::env::var("READONLY_MODE").is_ok() {
    println!("🔒 Running in READ-ONLY mode");
    tools::get_readonly_tools()
} else {
    println!("🚀 Running in FULL mode");
    tools::get_all_tools()
};
```

**Usage:**
```bash
# Read-only mode
READONLY_MODE=1 cargo run --bin tool_agent

# Full mode
cargo run --bin tool_agent
```

### Example 3: CLI Flag

```rust
use clap::Parser;
use crate::tools;

#[derive(Parser)]
struct Args {
    /// Enable read-only mode (safe, no modifications)
    #[arg(long)]
    readonly: bool,
}

fn main() {
    let args = Args::parse();

    let tools = if args.readonly {
        println!("🔒 READ-ONLY mode enabled");
        tools::get_readonly_tools()
    } else {
        println!("🚀 FULL mode enabled");
        tools::get_all_tools()
    };

    // ... rest of your code
}
```

**Usage:**
```bash
# Read-only mode
cargo run --bin tool_agent -- --readonly

# Full mode
cargo run --bin tool_agent
```

### Example 4: Dynamic Based on Context

```rust
use crate::tools;

fn get_tools_for_context(is_production: bool, is_review: bool) -> Vec<Tool> {
    match (is_production, is_review) {
        (true, _) => {
            // Production - always read-only
            println!("🔒 Production environment: READ-ONLY mode");
            tools::get_readonly_tools()
        },
        (false, true) => {
            // Code review - read-only
            println!("🔍 Code review: READ-ONLY mode");
            tools::get_readonly_tools()
        },
        (false, false) => {
            // Development - full access
            println!("🚀 Development: FULL mode");
            tools::get_all_tools()
        }
    }
}
```

---

## Custom Tool Selection

If neither mode fits your needs, build a custom set:

```rust
use crate::tools::{self, ToolName};

// Example: Allow exec but not delete
let tools = tools::build_tools(&[
    ToolName::ExecCommand,      // Can execute
    ToolName::GetFiles,
    ToolName::GetFilesRecursive,
    ToolName::SearchFilesWithRegex,
    ToolName::ReadFile,
    ToolName::SemanticSearch,
    // Deliberately excluded: DeletePath, DeleteMany
]);
```

---

## What Each Tool Does

### Full Mode Tools

#### Writing/Modifying Tools (excluded in read-only)
1. **exec_command**
   - Execute shell commands
   - Can modify files via shell commands (sed, echo, etc.)
   - Examples: `sed -i`, `echo >> file.txt`, `rm`

2. **delete_path**
   - Delete a single file or directory
   - Permanent operation

3. **delete_many**
   - Delete multiple files or directories
   - Batch deletion

#### Reading/Analysis Tools (included in both modes)
4. **get_files**
   - List files in a directory
   - Non-recursive

5. **get_files_recursive**
   - List all files in directory tree
   - Supports glob patterns

6. **search_files_with_regex**
   - Search file contents with regex
   - Returns matching lines

7. **read_file**
   - Read complete file or byte range
   - Can handle large files

8. **semantic_search**
   - AI-powered code search
   - Finds semantically relevant code

---

## Best Practices

### ✅ Use Full Mode When:
- Actively developing features
- You need the AI to make changes
- Working in a local development environment
- You're comfortable reviewing AI changes

### ✅ Use Read-Only Mode When:
- Reviewing code
- Understanding a new codebase
- Analyzing code for bugs or issues
- Working in shared/production environments
- You want zero risk of modifications
- Teaching or demonstrating code

### ⚠️ Safety Tips:
1. Always review AI-suggested commands before executing
2. Use read-only mode when in doubt
3. Keep backups or use version control
4. Test in development before using in production
5. Consider read-only for shared access scenarios

---

## Summary

**Simple rule of thumb:**

```
Need AI to make changes?
├─ Yes → tools::get_all_tools()
└─ No  → tools::get_readonly_tools()
```

**Read-only mode gives you:**
- All analysis capabilities
- Zero modification risk
- Safe for any environment
- Perfect for code review and understanding

**Full mode gives you:**
- Everything in read-only mode
- Plus ability to execute commands
- Plus ability to delete files
- Complete AI assistance
