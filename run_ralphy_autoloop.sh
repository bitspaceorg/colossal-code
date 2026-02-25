#!/usr/bin/env bash
set -euo pipefail

MODEL="${MODEL:-openai/gpt-5.3-codex}"
PRD_FILE="${PRD_FILE:-PRD.md}"
MAX_CYCLES="${MAX_CYCLES:-30}"
CHECK_CMD="${CHECK_CMD:-cargo check -p cocode}"

if ! command -v ralphy >/dev/null 2>&1; then
  echo "ralphy not found in PATH" >&2
  exit 1
fi

if [ ! -f "$PRD_FILE" ]; then
  echo "PRD file not found: $PRD_FILE" >&2
  exit 1
fi

remaining_tasks() {
  grep -E '^- \[ \] ' "$PRD_FILE" | wc -l | tr -d ' '
}

echo "Starting serial Ralphy autoloop"
echo "- model: $MODEL"
echo "- prd:   $PRD_FILE"
echo "- max cycles: $MAX_CYCLES"
echo "- check: $CHECK_CMD"

for ((i=1; i<=MAX_CYCLES; i++)); do
  left_before="$(remaining_tasks)"
  if [ "$left_before" -eq 0 ]; then
    echo "No unchecked PRD tasks remain. Done."
    exit 0
  fi

  echo ""
  echo "=== Cycle $i/$MAX_CYCLES (remaining tasks: $left_before) ==="

  ralphy --opencode --model "$MODEL" --prd "$PRD_FILE" --max-iterations 1

  echo "Running verification: $CHECK_CMD"
  bash -lc "$CHECK_CMD"

  left_after="$(remaining_tasks)"
  if [ "$left_after" -eq 0 ]; then
    echo "All PRD tasks checked off. Loop complete."
    exit 0
  fi

  if [ "$left_after" -ge "$left_before" ]; then
    echo "No PRD progress detected this cycle (before=$left_before, after=$left_after)."
    echo "Stopping to avoid spinning. Inspect changes and rerun."
    exit 2
  fi
done

echo "Reached MAX_CYCLES=$MAX_CYCLES with tasks still remaining ($(remaining_tasks))."
exit 3
