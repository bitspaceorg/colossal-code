# Qwen Development Guide

## Project Philosophy

This project follows a strict philosophy of writing **high-quality, pragmatic, idiomatic Rust code** that is:

- **Expert-level**: Leverages advanced Rust features appropriately (lifetimes, traits, async, etc.)
- **Modularized**: Well-organized modules with clear separation of concerns
- **Extensible**: Designed for easy addition of new features without major refactors
- **Beautiful**: Clean, readable code that follows Rust community best practices
- **Pragmatic**: Practical solutions that balance perfection with delivery

## Code Quality Standards

### Rust Best Practices

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `clippy` and `rustfmt` for automated code quality enforcement
- Prefer composition over inheritance (Rust doesn't have inheritance anyway)
- Use appropriate error handling with `Result<T, E>` rather than panicking
- Leverage Rust's ownership system to prevent runtime errors at compile time
- Use `Option<T>` for values that may not exist rather than special sentinel values

### Idiomatic Patterns

- Embrace `match` expressions for exhaustive pattern matching
- Use iterators and closures effectively (`map`, `filter`, `fold`, etc.)
- Prefer `impl Trait` for return position when appropriate
- Use newtype pattern for type safety
- Leverage `Deref` coercion judiciously
- Apply RAII (Resource Acquisition Is Initialization) principles

### Architecture Principles

- Single responsibility principle - each module/function should have one clear purpose
- Dependency inversion - high-level modules shouldn't depend on low-level details
- Favor small, focused functions over large monolithic ones
- Use traits for abstraction and polymorphism
- Apply the builder pattern for complex object construction

## Development Process

### The Scratchpad Rule

Every feature and subfeature must be documented in `scratchpad.md` with:

```
- [ ] Main Feature
  - [ ] Subfeature 1
  - [ ] Subfeature 2
    - [ ] Implementation Detail A
    - [ ] Implementation Detail B
```

**Requirements:**

1. Every checkbox must be checked off as it's completed
2. **NO SHORTCUTS** - If something isn't possible, it must be documented with clear reasoning
3. Placeholder implementations must be clearly marked and explained
4. When a feature is partially complete, document what's missing and why

### Example Scratchpad Entry

```markdown
- [x] Semantic Search Sessions
    - [x] Basic session creation and management
    - [x] File watching integration
    - [ ] Advanced query optimization
        - [x] Basic similarity search
        - [ ] Query result ranking
        - [ ] ~~Multi-modal search~~ (Not possible without additional model support)
    - [x] Incremental file indexing
        - [x] Change detection using byte range comparison
        - [x] Affected chunk identification
        - [x] Partial reindexing implementation
        - [ ] ~~Real-time chunk merging~~ (Placeholder: Currently reindexes affected regions separately)
```

### Implementation Rules

1. **No Hidden Technical Debt**
    - If a shortcut is taken, it must be documented in the scratchpad
    - All TODO comments must reference a scratchpad item
    - Deprecated code must be removed, not commented out

2. **Compilation Must Always Succeed**
    - Code should compile without errors (warnings acceptable for unused items)
    - All tests must pass before pushing changes
    - Use `#![deny(warnings)]` in production code

3. **Documentation Requirements**
    - Every public function must have rustdoc comments
    - Complex algorithms must have inline comments explaining the logic
    - Module-level documentation explaining purpose and usage

## Project Structure

```
src/
├── lib.rs          # Library entry point
├── main.rs         # Binary entry point
├── manager.rs      # Session management
├── session.rs      # Session implementations
├── semantic_search_lib.rs  # Semantic search functionality
├── protocol.rs     # Communication protocols
├── types.rs        # Shared data types
├── error.rs        # Error definitions
├── utils.rs        # Utility functions
└── bin/            # Binary targets
    ├── file_watcher_test.rs
    └── ...
```

## Key Components

### Session Management

Handles creation, tracking, and destruction of different session types:

- Command execution sessions
- Persistent shell sessions
- Semantic search sessions

### Semantic Search

Implements code indexing and retrieval using:

- Tree-sitter for code parsing
- Qdrant for vector storage and search
- File watching for incremental updates

### File Watching

Monitors filesystem changes with:

- Debouncing to prevent excessive reindexing
- Efficient change detection algorithms
- Real-time indexing updates

## Non-Functional Requirements

### Performance

- Minimize memory allocations
- Use appropriate data structures for the task
- Leverage async/await for I/O-bound operations
- Profile regularly to identify bottlenecks

### Reliability

- Graceful error handling with meaningful error messages
- Proper resource cleanup (Drop trait implementation)
- Timeout mechanisms for long-running operations
- Recovery from transient failures

### Maintainability

- Clear module boundaries
- Comprehensive test coverage
- Consistent naming conventions
- Regular dependency updates

## Testing Strategy

### Unit Tests

- Test individual functions and methods
- Mock external dependencies
- Cover edge cases and error conditions
- Located in the same files using `#[cfg(test)]` modules

### Integration Tests

- Test interactions between components
- Use real external services when possible
- Located in `tests/` directory

### Property-Based Testing

- Use `proptest` for complex data structure validation
- Test invariants and properties rather than specific values

## Deployment Considerations

### Environment Variables

All configurable parameters should be exposed via environment variables with sensible defaults.

### Logging

Use structured logging with appropriate levels:

- ERROR: System errors requiring immediate attention
- WARN: Unexpected conditions that don't halt execution
- INFO: High-level operational information
- DEBUG: Detailed diagnostic information
- TRACE: Very verbose debugging information

### Monitoring

Instrument key metrics for observability:

- Request/response latencies
- Error rates
- Resource utilization
- Business metrics

## Future Extensibility

### Plugin Architecture

Design core components to be extensible through:

- Trait objects for swappable implementations
- Configuration-driven behavior selection
- Well-defined interfaces for third-party integrations

### Versioning

Follow semantic versioning:

- MAJOR: Breaking changes to public API
- MINOR: New features, backward compatible
- PATCH: Bug fixes, backward compatible

## Contributing Guidelines

1. Always branch from `main` for new features
2. Use descriptive commit messages following conventional commits format
3. Open a pull request before merging
4. Ensure all CI checks pass
5. Have code reviewed by at least one other developer
6. Update documentation and scratchpad as needed
7. Squash commits before merging to maintain clean history

## Common Pitfalls to Avoid

1. **Premature Optimization** - Measure before optimizing
2. **Over-engineering** - Solve the problem at hand, not hypothetical future problems
3. **Ignoring Compiler Warnings** - Treat warnings as errors in production code
4. **Blocking Async Runtime** - Never block the async executor
5. **Memory Leaks** - Use RAII and smart pointers correctly
6. **Race Conditions** - Properly synchronize shared state

## Technology Stack Decisions

### Why Rust?

- Memory safety without garbage collection
- Zero-cost abstractions
- Excellent concurrency support
- Strong ecosystem for systems programming
- Compile-time guarantees reduce runtime errors

### Why Qdrant?

- Native vector search capabilities
- Horizontal scaling support
- Rich filtering and faceting options
- Good Rust client library

### Why Tree-sitter?

- Accurate parsing of source code
- Language-agnostic AST generation
- Incremental parsing support
- Active community and regular updates

This guide serves as the foundation for all development decisions in this project. Any deviation must be documented and justified.
