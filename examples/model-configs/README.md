# Model Configuration with model.yaml

This directory contains real `model.yaml` configuration files from the LMStudio registry (`~/.lmstudio/hub/models/`).

These are actual production configs used by LMStudio, not mock examples.

## Overview

The model configuration system allows you to:
- **Define thinking/reasoning capabilities** for models
- **Configure custom thinking tags** (opening and closing tags)
- **Set summary intervals** for rendering reasoning tree lines
- **Specify model metadata** like context length, tool use support, etc.

## Configuration Location

Model configurations should be placed in:
```
~/.config/.nite/models/<model-name>/model.yaml
```

For example:
```
~/.config/.nite/models/qwen3-thinking/model.yaml
~/.config/.nite/models/deepseek-r1/model.yaml
```

## How It Works

### 1. Automatic Detection
When you load a model, the system:
1. Extracts the model name from the model path
2. Looks for `~/.config/.nite/models/<model-name>/model.yaml`
3. If found, loads the configuration
4. If not found, falls back to filename-based detection (models with "thinking", "reasoning", or "thought" in the filename)

### 2. Thinking Tags
Different models use different tags to denote their reasoning/thinking sections:

- **Qwen3, DeepSeek**: `<think>...</think>`
- **Custom models**: May use `<reasoning>...</reasoning>`, `<thought>...</thought>`, etc.

The `thinkingTags` section in model.yaml allows you to configure these:

```yaml
thinkingTags:
  openTag: "<think>"       # Opening tag to detect
  closeTag: "</think>"     # Closing tag to detect
  summaryInterval: 200     # Generate summary every N tokens
```

### 3. Summary Interval
The `summaryInterval` controls how often (in tokens) the system generates summary tree lines during reasoning:

- `200` (default): Summary every 200 tokens - balanced
- `100`: More frequent summaries - more detailed tree
- `300`: Less frequent summaries - cleaner tree

## Real Examples from LMStudio Registry

This directory contains actual `model.yaml` files from the LMStudio registry:

### Example 1: GPT-OSS-20B (Reasoning Model)

**File**: `gpt-oss-20b.yaml`

This is a real reasoning model from the LMStudio registry. Key features:
- `reasoning: true` in metadataOverrides
- Custom field `reasoningEffort` with select options (low/medium/high)
- Uses Jinja variable `reasoning_effort` for controlling reasoning behavior

```yaml
model: openai/gpt-oss-20b
metadataOverrides:
  reasoning: true
  trainedForToolUse: true
customFields:
  - key: reasoningEffort
    displayName: Reasoning Effort
    description: Controls how much reasoning the model should perform.
    type: select
    options:
      - value: low
      - value: medium
      - value: high
```

### Example 2: Qwen3-Coder-30B

**File**: `qwen3-coder-30b.yaml`

A standard model configuration (non-reasoning) showing:
- Multiple base model sources (GGUF and MLX variants)
- `reasoning: false` explicitly set
- Operation config with temperature and sampling parameters

### Adding Thinking Tags to Existing Configs

To add custom thinking tag support to these models, extend them with `thinkingTags`:

```yaml
model: your-model/name
metadataOverrides:
  reasoning: true

# Add custom thinking tags configuration
thinkingTags:
  openTag: "<think>"      # or "<reasoning>", "<thought>", etc.
  closeTag: "</think>"
  summaryInterval: 200    # Adjust as needed (100-300 typical range)
```

## Full Schema

Based on LMStudio's `model.yaml` specification:

```yaml
# Model identifier
model: "organization/model-name"

# Optional: Base model reference
base: "base-model-id"

# Optional: Tags for categorization
tags:
  - thinking
  - reasoning

# Optional: Metadata overrides
metadataOverrides:
  domain: text
  architectures:
    - transformer
  reasoning: true  # or false, or "mixed"
  trainedForToolUse: true
  vision: false
  contextLengths:
    - 8192
    - 32768

# Optional: Custom fields (for UI controls)
customFields:
  - key: enableThinking
    displayName: Enable Thinking
    description: Enable the model to think before answering
    type: boolean
    defaultValue: true
    effects:
      - type: setJinjaVariable
        variable: enable_thinking

# Custom: Thinking tags configuration
thinkingTags:
  openTag: "<think>"
  closeTag: "</think>"
  summaryInterval: 200
```

## Creating Configurations

### Method 1: Manual Creation

1. Create the directory:
   ```bash
   mkdir -p ~/.config/.nite/models/my-model
   ```

2. Create `model.yaml`:
   ```bash
   cat > ~/.config/.nite/models/my-model/model.yaml << 'EOF'
   model: "my-org/my-model"
   metadataOverrides:
     reasoning: true
   thinkingTags:
     openTag: "<think>"
     closeTag: "</think>"
     summaryInterval: 200
   EOF
   ```

### Method 2: Copy Examples

Copy one of the example configs from this directory:
```bash
cp examples/model-configs/qwen-thinking.yaml ~/.config/.nite/models/my-model/model.yaml
```

Then edit it to match your model's configuration.

## Troubleshooting

### Model not detected as thinking model
- Check that the config file exists at the correct path
- Verify the YAML syntax is correct
- Ensure either:
  - `metadataOverrides.reasoning: true` is set, OR
  - `thinkingTags` section exists, OR
  - Model filename contains "thinking", "reasoning", or "thought"

### Tags not working correctly
- Verify the `openTag` and `closeTag` match what your model actually outputs
- Check the model's chat template to see what tags it uses
- Try enabling debug logging to see what content is being streamed

### Wrong summary interval
- Check the `summaryInterval` value in your model.yaml
- Default is 200 tokens if not specified
- Lower values = more frequent summaries (more tree lines)
- Higher values = less frequent summaries (cleaner output)

## References

- [LMStudio model.yaml documentation](https://lmstudio.ai/docs/app/modelyaml)
- [model.yaml specification](https://modelyaml.org/)
- [LMStudio model.yaml schema](https://github.com/lmstudio-ai/lmstudio-js/blob/main/packages/lms-shared-types/src/VirtualModelDefinition.ts)
