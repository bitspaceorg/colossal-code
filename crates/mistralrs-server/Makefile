.PHONY: test test-mock test-real fmt lint run-dev

# Default test runs the mock manager tests
test: test-mock

# Run integration tests with the mock manager (no GPU required)
test-mock:
	cargo test -p mistralrs-server-tests --features mock-manager

# Run integration tests with the real manager (simulates engine, tests scheduler)
test-real:
	cargo test -p mistralrs-server-tests --features real-manager

# Run all unit tests in the workspace
test-all:
	cargo test --workspace

# Format code
fmt:
	cargo fmt --all

# Lint code
lint:
	cargo clippy --workspace --all-targets --features mock-manager -- -D warnings

# Run the server in dev mode with mock manager
run-dev:
	cargo run --bin mistralrs-server --features mock-manager -- --config config/dev.toml --mock-manager
