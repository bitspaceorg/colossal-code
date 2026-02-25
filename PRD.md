# PRD: Cocode Debt Pass v4 (Hardening)

## Context

Debt pass v3 reduced risk, but the remaining warning and complexity surface is still high.

Current first-party baseline for v4:

- `src/main.rs`: ~11,682 LOC
- `cargo check --all-targets`: ~23 warnings
- `cargo clippy --all-targets --no-deps`: ~141 warnings
- `cargo test --all-targets`: passing

This PRD focuses on warning burn-down, tighter module boundaries, and safer ongoing maintenance.

## Goals

1. Reduce `cargo check` warnings to <= 10.
2. Reduce `cargo clippy` warnings to <= 60.
3. Reduce `src/main.rs` to <= 10,000 LOC.
4. Keep behavior stable with no UI regressions.

## Non-Goals

- No feature additions.
- No changes under `external/**`.
- No redesign of UX flow.

## Guardrails

- First-party edits only: `src/**`, first-party `crates/**`, first-party tests/docs.
- Do not edit `external/**`, `.cargo-local/**`, or `target/**`.
- One logical task per commit.
- Preserve existing behavior unless fixing a documented defect in this PRD.

## Definition of Done

- `bash ./scripts/verify_regression.sh` passes.
- `cargo check --all-targets` warning count <= 10.
- `cargo clippy --all-targets --no-deps` warning count <= 60.
- `src/main.rs` <= 10,000 LOC.
- Debt summary updated in `docs/debt/debt-pass-v4.md`.

## Execution Strategy

- Sequential only.
- One unchecked task per cycle.
- Run verification after each cycle; stop on failure and fix immediately.

## Tasks

### Phase V4-0: Baseline Refresh

- [x] V4-00 Refresh debt baseline snapshot.
  Scope: record current warning counts and LOC in `docs/debt/baseline-v4.md`.
  Done when: baseline is committed and reproducible.

### Phase V4-1: High-Signal Warning Burn-Down

- [x] V4-01 Collapse nested conditionals in first-party UI and spec modules.
  Scope: address `clippy::collapsible_if` in `src/main.rs`, `src/spec_ui.rs`, `src/session_manager.rs`, `src/survey.rs`, `src/persistence/conversations.rs`, and `src/ui_message_event.rs`.
  Done when: targeted files have no remaining `collapsible_if` warnings.

- [x] V4-02 Remove `clone_on_copy`, `manual_div_ceil`, `manual_range_contains`, and `manual_range_patterns` in `src/main.rs`.
  Done when: these lint classes are eliminated from `src/main.rs`.

- [x] V4-03 Eliminate remaining unused/dead symbols in first-party app paths.
  Scope: `src/main.rs`, `src/rich_editor.rs`, `src/survey.rs`, and warning hotspots in `crates/agent_core/src/lib.rs`.
  Done when: dead code warnings are reduced and intentional leftovers are documented.

- [x] V4-04 Resolve `too_many_arguments` in `src/spec_ui.rs` with parameter structs.
  Done when: `clippy::too_many_arguments` warnings are removed from spec UI paths.

- [x] V4-05 Clean `wrong_self_convention` and needless-return/borrow patterns in touched files.
  Done when: touched files are clippy-clean for these categories.

### Phase V4-2: Main.rs Surface Reduction

- [x] V4-06 Extract session lifecycle handlers from `src/main.rs` into `src/session_lifecycle.rs`.
  Done when: creation/restore/snapshot lifecycle code paths are module APIs.

- [x] V4-07 Extract command routing side-effects from `src/main.rs` into `src/command_runtime.rs`.
  Done when: slash-command runtime side effects no longer live directly in `main.rs`.

- [x] V4-08 Extract generation stats/thinking animation formatting helpers into `src/ui/thinking.rs`.
  Done when: display formatting logic is outside `main.rs` and covered by tests.

- [x] V4-09 Extract model selection and metadata rendering helpers into `src/ui/model_picker.rs`.
  Done when: model UI composition no longer lives in `main.rs`.

### Phase V4-3: Regression + Reporting

- [x] V4-10 Add regression tests for newly extracted modules.
  Done when: module-level tests pass and cover key behavior.

- [x] V4-11 Write v4 debt pass report.
  Scope: `docs/debt/debt-pass-v4.md` with before/after counts, remaining debt, and follow-up plan.
  Done when: report is committed and references baseline.

## Verification Commands

```bash
bash ./scripts/verify_regression.sh
cargo check --all-targets
cargo clippy --all-targets --no-deps
wc -l src/main.rs
```

## Commit Cadence

- One task = one commit.
- Keep each commit reviewable and behavior-safe.
- Re-run regression checks after each task.
