# PRD: Cocode Debt Pass (Stabilization-v2)

## Context

The initial stabilization/refactor pass is complete, but debt remains materially high.

Current baseline (first-party focus):

- `src/main.rs`: ~12.2k LOC
- `cargo check --all-targets`: passes with ~53 warnings
- `cargo clippy --all-targets --no-deps`: passes with many high-signal warnings
- `cargo test --all-targets`: passes
- External/vendor code has been reverted and is out of scope for this debt pass

This PRD defines a focused debt reduction pass to improve maintainability without changing product behavior.

## Goals

1. Reduce first-party warning volume by at least 70% from current baseline.
2. Shrink `src/main.rs` by extracting cohesive modules (target: <= 8,000 LOC this pass).
3. Remove dead code and stale private-interface mismatches in first-party crates.
4. Preserve runtime behavior and keep CI gates green.

## Non-Goals

- No feature additions.
- No behavioral rewrites of core orchestration logic.
- No edits to `external/**`.

## Guardrails

- Edit only first-party paths: `src/**`, `crates/agent_*/**`, `crates/chunker/**`, `crates/edtui/**`, `crates/markdown-renderer/**`, `crates/sandbox/**`, `crates/sessionizer/**`, docs/tests under first-party.
- Do not edit `external/**`, `.cargo-local/**`, or `target/**`.
- One logical change per commit.
- Keep diffs scoped and reversible.

## Definition of Done

- `cargo fmt --all --check` passes.
- `cargo check --all-targets` passes with warning count reduced by >= 70% vs baseline.
- `cargo clippy --all-targets --no-deps` has no new warnings and reduced existing high-signal warnings.
- `cargo test --all-targets` passes.
- `src/main.rs` <= 8,000 LOC.

## Execution Strategy

- Sequential execution only.
- Each cycle performs exactly one unchecked task, then runs verification.
- Stop on first failing verification and fix before continuing.

## Tasks

### Phase D0: Baseline Lock

- [ ] D0-01 Capture debt baseline report.
  Scope: persist current warning counts by crate, clippy top findings, and `src/main.rs` LOC into `docs/debt/baseline.md`.
  Done when: baseline is committed and referenced by subsequent tasks.

### Phase D1: Warning Burn-Down (High Signal First)

- [ ] D1-01 Eliminate unused imports/vars in `src/main.rs`, `src/rich_editor.rs`, and `src/spec_cli.rs`.
  Scope: clean obvious `unused_imports`, `unused_variables`, `unused_mut` with no behavior changes.
  Done when: warnings for these files are near-zero.

- [ ] D1-02 Resolve private interface mismatches in UI message/state types.
  Scope: align visibilities for `UiMessage`, `MessageState`, `AppSnapshot`, and related APIs.
  Done when: `private_interfaces` warnings are removed.

- [ ] D1-03 Remove dead code in first-party app paths.
  Scope: unused methods/fields flagged in `src/main.rs`, `src/rich_editor.rs`, and `src/survey.rs`.
  Done when: dead-code warnings reduced materially without feature loss.

- [ ] D1-04 Address clippy high-signal patterns in touched first-party files.
  Scope: collapsible `if`, redundant closures/imports, manual pattern simplifications, and similar safe refactors.
  Done when: clippy output for touched files is clean.

### Phase D2: Main.rs Decomposition

- [ ] D2-01 Extract prompt/approval rendering helpers from `src/main.rs` into `src/ui/prompts.rs`.
  Done when: sandbox/approval prompt rendering no longer lives in `main.rs`.

- [ ] D2-02 Extract model metadata/context utilities from `src/main.rs` into `src/model_context.rs` APIs.
  Done when: parameter/context extraction logic is fully outside `main.rs`.

- [ ] D2-03 Extract spec tree and plan view composition from `src/main.rs` into `src/spec_ui.rs`.
  Done when: spec rendering helpers are centralized and tested.

- [ ] D2-04 Extract message/event parsing/formatting into dedicated module.
  Scope: move typed event parsing/serialization from `main.rs` to `src/messages.rs` (or equivalent).
  Done when: `main.rs` uses module APIs only.

### Phase D3: Regression and Docs

- [ ] D3-01 Add regression tests for each extracted module boundary.
  Done when: targeted tests cover moved logic and pass.

- [ ] D3-02 Write debt-pass summary and remaining debt register.
  Scope: `docs/debt/debt-pass-v2.md` with before/after metrics and follow-up backlog.
  Done when: residual debt is explicit and prioritized.

## Verification Commands

```bash
cargo fmt --all --check
cargo check --all-targets
cargo clippy --all-targets --no-deps
cargo test --all-targets
```

## Commit Cadence

- One task = one commit.
- Run relevant checks per task.
- Run full verification at end of each phase.
