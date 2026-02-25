#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR"

fmt_check_first_party() {
  cargo fmt --manifest-path "$ROOT_DIR/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/agent_core/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/agent_protocol/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/chunker/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/edtui/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/markdown-renderer/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/sandbox/Cargo.toml" -- --check
  cargo fmt --manifest-path "$ROOT_DIR/crates/sessionizer/Cargo.toml" -- --check
}

echo "[1/5] cargo fmt (first-party only) --check"
fmt_check_first_party

echo "[2/5] cargo check --all-targets"
cargo check --all-targets

echo "[3/5] cargo clippy --all-targets --no-deps"
cargo clippy --all-targets --no-deps

echo "[4/5] cargo test --all-targets"
cargo test --all-targets

echo "[5/5] targeted app tests"
cargo test -p cocode -- --nocapture

echo "Regression verification passed."
