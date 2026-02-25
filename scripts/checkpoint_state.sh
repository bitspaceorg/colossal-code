#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="$ROOT_DIR/.checkpoints/$STAMP"

mkdir -p "$OUT_DIR"

git -C "$ROOT_DIR" status --short --branch >"$OUT_DIR/status.txt"
git -C "$ROOT_DIR" log --oneline --decorate -n 40 >"$OUT_DIR/log.txt"
git -C "$ROOT_DIR" diff >"$OUT_DIR/working.diff"
git -C "$ROOT_DIR" diff --cached >"$OUT_DIR/staged.diff"
git -C "$ROOT_DIR" ls-files --others --exclude-standard >"$OUT_DIR/untracked.txt"

TAG_NAME="checkpoint/$STAMP"
git -C "$ROOT_DIR" tag "$TAG_NAME" HEAD

printf "Checkpoint created\n"
printf "- dir: %s\n" "$OUT_DIR"
printf "- tag: %s\n" "$TAG_NAME"
