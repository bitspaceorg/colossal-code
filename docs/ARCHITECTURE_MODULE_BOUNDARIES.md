# Architecture: Module Boundaries

This document defines the boundaries introduced in the `src/` split so future
changes keep responsibilities clear.

## Goals

- Keep `src/main.rs` focused on application wiring and top-level state.
- Move feature-specific logic into dedicated modules with narrow interfaces.
- Avoid cyclical dependencies between feature modules.

## Boundary Map

### `src/main.rs`

- Owns top-level app state, startup/shutdown, and event loop orchestration.
- Delegates feature logic to the modules below.
- Should not accumulate new feature-specific parsing or rendering logic.

### `src/commands/`

- Owns slash command parsing and dispatch.
- Converts command input into explicit app actions.
- Does not render UI or persist data directly.

### `src/persistence/`

- Owns filesystem-backed state: config, history, conversations, and todos.
- Exposes read/write helpers used by the app.
- Does not parse slash commands or perform screen rendering.

### `src/ui/`

- Owns reusable UI rendering helpers and panel-specific presentation logic.
- Consumes app state but does not mutate persistence directly.
- Does not parse slash commands.

### `src/spec_cli.rs`

- Owns `/spec*` command handling and orchestration command messaging.
- Bridges app state and `agent_core` spec/orchestrator APIs.
- Avoids direct persistence and generic slash command routing.

### `src/model_context.rs`

- Owns model context-length detection and metadata extraction.
- Handles GGUF metadata probing and config-based fallback lookup.
- Does not depend on UI or slash command logic.

## Dependency Rules

- `main.rs` may depend on `commands`, `persistence`, `ui`, `spec_cli`, and
  `model_context`.
- Feature modules should not depend on each other unless there is a clear,
  one-directional data flow.
- Cross-cutting behavior should be extracted as small shared helpers instead of
  creating implicit coupling.

## Change Checklist

When adding a new feature:

1. Pick the owning module first.
2. Expose a minimal interface from that module.
3. Keep rendering, command parsing, and persistence concerns separated.
4. Update this document if a new boundary is introduced.
