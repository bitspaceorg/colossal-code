# Tool Agent

A sandboxed AI agent system using local LLM (Qwen) with tool calling capabilities.

## Features

- **Sandboxed Execution**: All file operations restricted by Landlock LSM on Linux
- **Persistent Shell Sessions**: Commands execute in the same shell, preserving environment variables and state
- **Semantic Code Search**: Automatic code indexing with vector embeddings
- **Tool Calling**: Exec commands, file operations, semantic search

## Quick Start

```bash
# Build the project
cargo build --release

# Run the agent
./target/release/tool_agent

# Run with additional sandbox roots
SANDBOX_EXTRA_ROOTS="/path/to/workspace:/another/path" ./target/release/tool_agent
```

## Sandbox Configuration

### Default Behavior

By default, the agent can only access:

- Current working directory (full read/write)
- System directories (read-only): `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`
- Temporary directories: `/tmp`, `$TMPDIR`

### Adding Writable Roots

Use the `SANDBOX_EXTRA_ROOTS` environment variable to grant access to additional directories:

```bash
# Single directory
export SANDBOX_EXTRA_ROOTS="/home/user/projects"

# Multiple directories (colon-separated)
export SANDBOX_EXTRA_ROOTS="/home/user/projects:/home/user/data"

# Run with extra roots
SANDBOX_EXTRA_ROOTS="/path/to/code" cargo run
```

**Security Notes:**

- All paths in `SANDBOX_EXTRA_ROOTS` are granted **recursive** read/write access
- Invalid paths will cause runtime errors when accessed
- Empty paths in the list are ignored
- Be careful not to expose sensitive directories

### Verifying Sandbox Works

```bash
# Should succeed - current directory
ls .

# Should fail - unauthorized directory (without SANDBOX_EXTRA_ROOTS)
cat /etc/passwd
# Expected: Permission denied

# Should succeed - with SANDBOX_EXTRA_ROOTS
SANDBOX_EXTRA_ROOTS="/tmp" ./target/release/tool_agent
# Then try: ls /tmp
```

## Semantic Search

### Current Behavior (Lazy Indexing)

Semantic search indexing happens **on-demand** when the model first uses the `semantic_search` tool:

1. Model calls semantic_search tool
2. System creates Qdrant collection
3. Indexes all Python files in current directory (background task)
4. Waits up to 30 seconds for indexing to complete
5. Executes the search query

**Pros:**

- No wasted resources if semantic search not needed
- Simple implementation

**Cons:**

- First search has significant delay (waiting for indexing)
- User experience is poor on first query

### Future: Proactive Indexing (Not Yet Implemented)

Planned improvement: Start indexing automatically on first tool call.

**When we implement this:**

- Indexing starts in background during first agent action
- By the time model wants to search, index likely ready
- Better user experience
- Slightly higher resource usage

**To enable in future:**

```rust
// In get_or_create_shell_session(), after creating shell:
let _ = state.manager.create_semantic_search_session(
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    state.sandbox_policy.clone(),
    None,
).await;
```

### Semantic Search Configuration

Currently hardcoded:

- **File types**: Only Python files (`.py`)
- **Concurrency**: 64 concurrent embedding requests
- **Collection**: One per directory (hash-based ID)
- **Qdrant URL**: `http://localhost:6334`

## Multi-Agent Orchestration

The agent system supports multi-step task orchestration with verification, allowing complex workflows to be broken down and executed systematically.

### Spec Creation → Execution → Verification Flow

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│  Create Spec    │───▶│  Execute Steps  │───▶│   Verification  │
│  (from goal or  │    │  (depth-first)  │    │  (verifier      │
│   JSON file)    │    │                 │    │   chain)        │
└─────────────────┘    └────────┬────────┘    └────────┬────────┘
                                │                      │
                                ▼                      ▼
                       ┌────────────────┐     ┌────────────────┐
                       │ Recursive Split│     │ Retry on Fail  │
                       │ (sub-specs)    │     │ (with feedback)│
                       └────────────────┘     └────────────────┘
```

### CLI Usage

```bash
# Load spec from file
nite --spec /path/to/spec.json

# Create spec from goal
nite --spec "Implement user authentication with JWT tokens"

# Target a different workspace directory (worktrees + commands run there)
nite --workspace-root ~/rust/todo --spec "Build a Rust CLI todo app"
```

You can also set the `NITE_WORKSPACE_ROOT` environment variable before launching the TUI if you prefer an env-based override. When provided, all orchestrator worktrees and default shell commands run relative to that directory instead of the CLI's current working directory.

> **Important:** the workspace root must be a git repository with at least one commit (e.g., run `git init`, add files, and create an initial commit). The orchestrator branches from the current HEAD, so it needs a base history to clone and merge.

### Slash Commands

| Command               | Description                             |
| --------------------- | --------------------------------------- |
| `/spec`               | Show current spec status                |
| `/spec <path\|goal>`  | Load spec from file or create from goal |
| `/spec split <index>` | Split a step into sub-steps             |
| `/spec status`        | Show detailed JSON snapshot             |
| `/spec abort`         | Abort the current orchestration run     |

### SpecSheet Structure

```json
{
    "id": "spec-1234567890",
    "title": "Implement Feature X",
    "description": "Detailed description of the feature",
    "steps": [
        {
            "index": "1",
            "title": "Setup project structure",
            "instructions": "Create necessary directories and files",
            "acceptance_criteria": ["Directory exists", "Files created"],
            "status": "Completed",
            "dependencies": []
        },
        {
            "index": "2",
            "title": "Implement core logic",
            "instructions": "Write the main functionality",
            "status": "InProgress",
            "dependencies": ["1"]
        }
    ],
    "created_by": "cli",
    "created_at": "2024-01-01T00:00:00Z",
    "metadata": {}
}
```

### TaskSummary Structure

```json
{
    "task_id": "task-abc123",
    "step_index": "1",
    "summary_text": "Completed step 1: Setup project structure",
    "artifacts_touched": ["src/main.rs", "Cargo.toml"],
    "tests_run": [{ "name": "build", "passed": true }],
    "verification": {
        "status": "Passed",
        "feedback": []
    }
}
```

### Orchestrator Control

The TUI can control the orchestrator using the `OrchestratorControl` handle:

```rust
use agent_core::orchestrator::{Orchestrator, OrchestratorControl, OrchestratorEvent};

// Create orchestrator with control handle
let (event_tx, event_rx) = mpsc::unbounded_channel();
let (orchestrator, control) = Orchestrator::new_with_control(
    main_agent,
    sub_agent_factory,
    verifier_chain,
    spec,
    event_tx,
);

// Control the orchestrator
control.pause()?;     // Pause after current step
control.resume()?;    // Resume execution
control.abort()?;     // Stop execution immediately
control.rerun_verifiers()?;  // Re-verify last step
control.inject_split("1".to_string(), child_spec)?;  // Inject sub-spec
```

### Event Types

The orchestrator emits events for TUI updates:

| Event               | Description                                              |
| ------------------- | -------------------------------------------------------- |
| `StepStatusChanged` | Step transitioned to new status                          |
| `SummaryUpdated`    | Task summary updated                                     |
| `VerifierFailed`    | Verifier rejected step, retry pending                    |
| `ChildSpecPushed`   | Sub-spec added to execution stack                        |
| `Paused`            | Orchestrator paused                                      |
| `Resumed`           | Orchestrator resumed                                     |
| `Aborted`           | Orchestrator aborted                                     |
| `Completed`         | All steps completed                                      |
| `ChannelClosed`     | close_task_channel finished, SSE subscribers should drop |

### Manual Verification Checklist

Before landing orchestration changes, run the same workflow we expect operators to validate in the TUI:

#### Build & Test Validation

```bash
# All must pass before merging
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

#### TUI Spec Commands (Interactive Validation)

1. Launch the TUI and load a spec:

    ```bash
    nite --spec /path/to/spec.json
    # or create from goal:
    nite
    > /spec "Implement feature X with tests"
    ```

2. Exercise all slash commands against a live run:
    - `/spec` - view current spec status
    - `/spec pause` - pause execution after current step
    - `/spec resume` - resume paused execution
    - `/spec rerun` - re-run verifiers on last step
    - `/spec history` - show task history summaries
    - `/spec split <index>` - split step into sub-steps
    - `/spec abort` - abort current run

#### Spec Pane Keybindings (inside spec pane when visible)

| Key            | Action                                       |
| -------------- | -------------------------------------------- |
| `P`            | Toggle pause/resume orchestrator             |
| `R`            | Rerun verifiers on last step                 |
| `A`            | Abort current orchestrator run               |
| `H`            | Toggle history view in spec pane             |
| `Enter`        | Toggle step drawer (summary, diff, feedback) |
| `↑/↓` or `k/j` | Navigate steps                               |

#### Session Window & View Toggles

- Toggle spec pane: `Shift+S` (capital S)
- Toggle condensed tool-call view: `Ctrl+W`
- Toggle dedicated history panel: `Ctrl+Shift+H`
- Toggle session window (Alt+W): Shows agent tree + embedded live UI

#### SSE / Channel Cleanup Verification

- After a spec run completes or aborts, capture logs
- Confirm `ChannelClosed` event emitted after `close_task_channel`
- Verify SSE subscribers receive the event and unsubscribe cleanly

### Coding Standards & Async Conventions

#### Rust 2024 Edition

- Use Rust 2024 edition lints (set in `Cargo.toml`: `edition = "2024"`)
- Enable workspace-wide clippy lints
- Format with `rustfmt` before committing

#### Async Best Practices

```rust
// BAD: Blocking inside async context
async fn process_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)?  // Blocks executor!
}

// GOOD: Use spawn_blocking for file IO
async fn process_file(path: &Path) -> Result<String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(&path)
    }).await?
}
```

#### Synchronization Guidelines

- **Prefer channels over mutexes**: Use `tokio::sync::mpsc` for cross-task communication
- **Arc<Mutex<\_>> exceptions**: Only when a mutable aggregate genuinely requires shared ownership
- **Event-driven updates**: TUI state updated via `OrchestratorEvent` channel, not shared state

```rust
// GOOD: Channel-based state updates
let (event_tx, event_rx) = mpsc::unbounded_channel();
let (orchestrator, control) = Orchestrator::new_with_control(..., event_tx);

// In TUI event loop:
while let Ok(event) = event_rx.try_recv() {
    self.handle_orchestrator_event(event);
}

// AVOID: Direct shared state access
// let shared = Arc::new(Mutex::new(state));
```

#### Sub-Agent Factory Pattern

Sub-agents are created via a factory closure for flexible nesting:

```rust
let sub_agent_factory = Arc::new(|step: &SpecStep| -> Arc<dyn OrchestratorAgent> {
    // Create lightweight sub-agent based on step requirements
    match step.required_tools.first().map(|s| s.as_str()) {
        Some("code") => Arc::new(CodeAgent::new()),
        Some("test") => Arc::new(TestAgent::new()),
        _ => Arc::new(GeneralAgent::new()),
    }
});
```

### Extending Verifiers, CLI, and TUI Controls

#### Registering New Verifiers

Register new verifiers by pushing boxed implementations into the `VerifierChain`:

```rust
let chain = VerifierChain::new(vec![
    Box::new(CommandVerifier::new(vec!["cargo", "check"])),
    Box::new(LintVerifier::new()),
    Box::new(CustomVerifier::new()),  // Add your verifier here
]);
```

Emit rich `FeedbackEntry` messages so `/spec history` and the spec pane drawer can surface them:

```rust
Err(FeedbackEntry {
    author: "my-verifier".to_string(),
    message: "Detailed failure reason with actionable guidance".to_string(),
    timestamp: Utc::now(),
})
```

#### Adding CLI Commands

CLI handlers are centralized in `SpecCliHandler`. To add a new command:

1. Add command matching in `SpecCliHandler::execute()`
2. Implement handler method (keep mutations synchronous for testability)
3. Add unit test in `spec_cli.rs` tests module

```rust
// In spec_cli.rs
pub async fn execute(&mut self, agent: Option<&dyn SpecAgentBridge>, command: &str) -> SpecCommandResult {
    if cmd_lower == "/spec mycommand" {
        self.handle_my_command();
        return SpecCommandResult::Handled;
    }
    // ...
}
```

#### Adding TUI Panel Sections

When adding new panels or commands, ensure consistency across views:

1. Update `SessionManager` tree for Alt+W session window
2. Update condensed breadcrumb log for Ctrl+W view
3. Handle orchestrator events appropriately

```rust
fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
    match event {
        OrchestratorEvent::MyNewEvent { data } => {
            // Update UI state
            self.update_my_panel(data);
            // Update session manager
            self.update_session_entries();
            // Add to condensed log if relevant
            self.add_tool_breadcrumb(...);
        }
        // ...
    }
}
```

### TUI View Modes & Behaviors

#### Condensed Tool-Call View (Ctrl+W)

Toggle between the full chat stream and a condensed breadcrumb-only view showing tool calls:

```
┌─────────────────────────────────────────────────────────────────────┐
│ [✓] main › exec_command                                             │
│ [✓] main › read_file                                                │
│ [◐] main › 1.2 › exec_command                                       │
│ [✗] main › 1.2 › edit_file (Error: file not found)                  │
│ [✓] main › 1.2 › edit_file                                          │
└─────────────────────────────────────────────────────────────────────┘

Format: [status] agent_path › tool_name [inline error if failed]
```

The condensed view:

- Shows only tool calls with status indicators
- Preserves agent path breadcrumbs (e.g., `main › 1.2` for nested sub-agent)
- Includes inline errors for failed calls
- Updates in real-time as orchestration progresses

#### Session Window (Alt+W)

Opens a dedicated view showing the agent tree and embedded live UI:

```
┌─ Agent sessions ─────────────────────────────────────────────┐
│ ◐ Spec: Feature Implementation                               │
│   ● 1 › Setup project structure                    · 2m 30s  │
│   ◐ 2 › Implement core logic                       · 45s     │
│     ○ 2.1 › Child task one                                   │
│     ○ 2.2 › Child task two                                   │
│   ○ 3 › Write tests                                          │
├──────────────────────────────────────────────────────────────┤
│ Live UI: 2 › Implement core logic                            │
│ ┌──────────────────────────────────────────────────────────┐ │
│ │ ⌨ focus is shared with selected agent                   │ │
│ └──────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘

Status icons:
  ○ Pending    ◐ In Progress    ● Completed    ✗ Failed
```

Navigation:

- `↑/↓` - Navigate the session tree
- `Enter` - Select session
- `d` - Detach session
- `x` - Kill session (not available for orchestrator sessions)
- `Esc` - Close session window

#### Spec Pane (Shift+S)

The spec pane shows current spec status with interactive controls:

```
┌─ Spec metadata ──────────────────────────────────────────────┐
│ 📋 Feature Implementation [PAUSED]                           │
│ Created by cli • 2024-01-15 14:30                            │
│ Controls: P Pause · R Rerun · A Abort · H History · Enter    │
├─ Steps ──────────────────────────────────────────────────────┤
│ #    State  Title                    Details                 │
│ 1    ●      Setup structure          Criteria 1 | —          │
│ 2    ◐      Core logic               Criteria 2 | ↳ 1        │
│ 3    ○      Write tests              Criteria 3 | ↳ 2        │
├─ Step drawer (Enter to toggle) ──────────────────────────────┤
│ Step 2: Core logic                                           │
│ Instructions: Implement the main functionality               │
│ Acceptance criteria:                                         │
│   • Code compiles                                            │
│   • Tests pass                                               │
│ Depends on: 1                                                │
└──────────────────────────────────────────────────────────────┘
```

When drawer is open after failure, shows:

- Latest summary text
- Test results
- Artifacts touched
- Verification feedback for review before resuming

#### History Panel (Ctrl+Shift+H)

Dedicated scrollable history panel (separate from spec pane):

```
┌─ Spec History ───────────────────────────────────────────────┐
│ ✓ Step 1 · Completed setup: created directory structure      │
│   Tests: cargo test(Pass)                                    │
│   Artifacts: src/main.rs, Cargo.toml                         │
│ ✓ Step 2 · Implemented core feature with validation          │
│   Tests: cargo test(Pass), integration(Pass)                 │
│   Artifacts: src/lib.rs, src/core.rs                         │
│ ✗ Step 3 · Failed verification                               │
│   Feedback lint: Missing documentation on public API         │
└──────────────────────────────────────────────────────────────┘
```

### Custom Verifiers

Implement the `Verifier` trait to add custom verification logic:

```rust
use agent_core::orchestrator::{Verifier, VerifierChain};
use agent_protocol::types::spec::{FeedbackEntry, TaskSummary};

struct CustomVerifier;

#[async_trait]
impl Verifier for CustomVerifier {
    async fn verify(&self, summary: &TaskSummary) -> Result<(), FeedbackEntry> {
        // Custom verification logic
        if some_condition {
            Ok(())
        } else {
            Err(FeedbackEntry {
                source: "custom".to_string(),
                message: "Verification failed".to_string(),
                severity: Severity::Error,
                timestamp: Utc::now(),
            })
        }
    }
}

// Use in verifier chain
let chain = VerifierChain::new(vec![
    Box::new(CommandVerifier::new(vec!["cargo", "check"])),
    Box::new(LintVerifier::new()),
    Box::new(CustomVerifier),
]);
```

## Architecture

### Sandbox Implementation

- **Linux**: Landlock LSM (kernel 5.13+)
- **macOS**: Seatbelt (partial support)
- **Other**: No sandboxing

The sandbox is applied at the thread level and inherited by child processes. All file operations are restricted by the kernel.

**Critical Implementation Details:**

- Sequential code chunking (not parallel) to avoid Rayon thread pool bypass
- Spawned async tasks explicitly apply Landlock
- File watcher is disabled (events never processed)

See `SECURITY.md` for detailed security documentation.

### Session Management

1. **Persistent Shell Session**: Created on first tool call, reused for all commands
2. **Semantic Search Sessions**: Created per directory (lazy, on-demand)
3. **Global State**: Single sandbox policy applied to all sessions

### Tool Execution

All tools execute through the sandboxed system:

- `exec_command`: Run shell commands in persistent session
- `delete_path`: Delete a single file or directory
- `delete_many`: Delete multiple files/directories at once
- `get_files`: List directory contents (non-recursive)
- `get_files_recursive`: Recursively list all files with glob patterns
- `search_files_with_regex`: Search file contents with regex patterns
- `read_file`: Read complete file or byte range
- `edit_file`: Find and replace text in files
- `semantic_search`: Search codebase with vector embeddings
- `web_search`: Search the web using DuckDuckGo
- `html_to_text`: Extract plain text from web URLs

Error handling: Tool failures are sent back to the model as JSON error messages, allowing retry/recovery.

## Requirements

- **Rust**: 2024 edition
- **Linux kernel**: 5.13+ for Landlock support
- **Qdrant**: Running on `localhost:6334` for semantic search
- **CUDA**: Optional, for GPU-accelerated model inference

## Development

```bash
# Run tests
cargo test

# Check for issues
cargo clippy

# Format code
cargo fmt

# Build documentation
cargo doc --open
```

## Files

- `src/main.rs`: Agent entry point, tool definitions, generation loop
- `crates/sessionizer/`: Sandboxed session management
- `crates/sessionizer/src/landlock.rs`: Landlock enforcement
- `crates/sessionizer/src/semantic_search_lib.rs`: Code indexing and search
- `SECURITY.md`: Detailed security documentation
- `SANDBOX.md`: Sandbox configuration guide
- `FIXES_SUMMARY.md`: Security fixes applied

## Known Issues

- File watcher is created but never processes events (intentionally disabled)
- Network restrictions not supported (Landlock ABI V3 limitation)
- macOS sandbox support is incomplete

## Performance

- **Code chunking**: Sequential (secure but slower than parallel)
- **Embedding generation**: 64 concurrent requests (network-bound bottleneck)
- **Shell commands**: Fast, persistent session avoids spawn overhead

For large codebases, first semantic search may take 10-30 seconds for initial indexing.

## License

[Add license information]

## Contributing

[Add contribution guidelines]
