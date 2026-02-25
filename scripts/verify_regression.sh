#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR"

echo "[1/5] cargo fmt --all --check"
cargo fmt --all --check

echo "[2/5] cargo check --all-targets"
cargo check --all-targets

echo "[3/5] cargo clippy --all-targets --no-deps"
cargo clippy --all-targets --no-deps

echo "[4/5] cargo test --all-targets"
cargo test --all-targets

echo "[5/5] targeted app tests"
cargo test -p cocode commands:: persistence:: tests:: ui:: -- --nocapture

echo "Regression verification passed."
