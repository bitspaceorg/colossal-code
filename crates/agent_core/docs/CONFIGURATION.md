# Tool Agent Configuration Guide

## Production-Ready Configuration

The tool agent is now production-ready with no hardcoded paths. All configuration is dynamic and environment-based.

## Environment Variables

### Required

None - the agent works out of the box with sensible defaults.

### Optional

#### `SANDBOX_EXTRA_ROOTS`

Add additional writable roots to the sandbox policy (colon-separated paths).

```bash
export SANDBOX_EXTRA_ROOTS="/path/to/data:/another/path"
```

#### `DISABLE_LANDLOCK`

Disable Landlock sandboxing if you encounter issues on your system.

```bash
export DISABLE_LANDLOCK=1
```

**Note**: Some systems (e.g., Fedora 42 with kernel 6.14) may have Landlock issues where child processes get Permission Denied even with proper rules. Set this to `1` to bypass Landlock if needed.

## Working Directory

The agent uses the **current working directory** where you run the binary as the workspace:

```bash
# Run from your project directory
cd /path/to/your/project
tool_agent

# The agent will use /path/to/your/project as the workspace
```

## Sandbox Policy

### Automatic Writable Roots

The agent automatically grants sandbox access to:

1. **Current working directory** - The directory where you run the binary
2. **Parent directory** - Allows navigation up one level
3. **Tools binary directory** - Where the sessionizer tools binary is located

### Custom Writable Roots

Add custom paths via `SANDBOX_EXTRA_ROOTS`:

```bash
export SANDBOX_EXTRA_ROOTS="/home/user/data:/opt/resources"
tool_agent
```

## Shell Sessions

### Persistent Shell

- Automatically created on first use
- Maintains environment variables and working directory across commands
- Automatically cleaned up on exit

### Working Directory

- Shell starts in the current working directory
- Stays synchronized with the workspace

## Semantic Search

### Vector Database

- Uses Qdrant for semantic code search
- Collections are automatically created and indexed
- Collections are automatically deleted on cleanup

### File Watching

- Automatically watches the workspace for file changes
- Updates the index in real-time
- Stopped and cleaned up on exit

## Resource Cleanup

All resources are automatically cleaned up when the agent exits:

- ✅ Persistent shell sessions terminated
- ✅ Semantic search sessions stopped
- ✅ File watchers stopped
- ✅ Background indexing tasks aborted
- ✅ Vector DB collections deleted
- ✅ All child processes killed

## Examples

### Basic Usage

```bash
cd /path/to/your/codebase
tool_agent
```

### With Extra Sandbox Access

```bash
cd /path/to/your/codebase
export SANDBOX_EXTRA_ROOTS="/home/user/shared:/opt/libs"
tool_agent
```

### Disable Landlock (if needed)

```bash
cd /path/to/your/codebase
export DISABLE_LANDLOCK=1
tool_agent
```

## Testing

Run the test examples to verify functionality:

```bash
# Test shell persistence
cargo run --example test_shell

# Test agent flow
cargo run --example test_agent_flow
```

## Deployment

### Binary Installation

```bash
# Build release binary
cargo build --release

# Install to system path
sudo cp target/release/tool_agent /usr/local/bin/

# Run from any directory
cd /path/to/project
tool_agent
```

### System Requirements

- **OS**: Linux (primary), macOS (limited support)
- **Kernel**: 5.13+ for Landlock support (optional, can be disabled)
- **Dependencies**:
    - Qdrant server (for semantic search)
    - GPU with CUDA support (for model inference)

### Model Configuration

The agent uses **mistral.rs** with the following optimizations:

- ✅ **FlashAttention V2/V3** - Memory-efficient attention computation
- ✅ **PagedAttention** - Efficient KV cache management (uses 95% of GPU memory)
- ✅ **GGUF quantized models** - Reduced model size with minimal quality loss

**Memory management**:

- PagedAttention allocates 95% of available GPU VRAM for KV cache
- Block size: 32 tokens per block
- Helps prevent CUDA OOM errors during long conversations

## Troubleshooting

### Permission Denied Errors

If you get "Permission denied" errors:

1. **Check Landlock compatibility**:

    ```bash
    export DISABLE_LANDLOCK=1
    tool_agent
    ```

2. **Add paths to sandbox**:
    ```bash
    export SANDBOX_EXTRA_ROOTS="/path/that/needs/access"
    tool_agent
    ```

### Semantic Search Not Working

Ensure Qdrant is running:

```bash
# Check if Qdrant is accessible
curl http://localhost:6333/
```

### Model Loading Issues

Ensure CUDA is available:

```bash
nvidia-smi
```

## Security

### Sandboxing

- **Landlock**: Linux kernel-level filesystem sandboxing (optional)
- **Restricted Paths**: Only workspace and explicitly allowed paths are accessible
- **No Network by Default**: Unless specifically enabled in tools

### Recommendations

1. Run in isolated environments when working with untrusted code
2. Use `SANDBOX_EXTRA_ROOTS` sparingly - only add paths you trust
3. Keep Landlock enabled unless you have compatibility issues
4. Review shell commands before execution in production

## Production Checklist

- [ ] Remove test/example code from deployment
- [ ] Set up Qdrant service
- [ ] Configure GPU/CUDA environment
- [ ] Test with your actual codebase
- [ ] Set up monitoring/logging
- [ ] Configure resource limits (memory, CPU)
- [ ] Review security policies
- [ ] Document team-specific workflows
