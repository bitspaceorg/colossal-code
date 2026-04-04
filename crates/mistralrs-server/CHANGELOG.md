# Changelog

## [Unreleased]

### Added

- **Server Config**: Comprehensive TOML configuration with environment variable overrides.
- **API**: Full OpenAI-compatible Chat Completion API (`/v1/chat/completions`).
- **API**: Custom endpoints for generation, embeddings, and model management.
- **Scheduling**: LRU eviction, concurrency limits, and paged attention support.
- **Admin**: Dynamic model loading/unloading and config reloading.
- **Observability**: Prometheus metrics and structured JSON logging.
- **Testing**: Integration test suite with mock manager support.

### Changed

- Refactored server into modular crates (`api`, `core`, `config`, `scheduler`).
- Updated `mistralrs-server` binary to use the new modular architecture.

### Fixed

- Standardized error handling across all API endpoints.
- Improved SSE streaming robustness.
