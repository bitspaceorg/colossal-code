# Debt Pass V2 Summary

Captured on 2026-02-25 for task `D3-02`.

## Scope

- Baseline source: `docs/debt/baseline.md` (`D0-01`)
- Current command set:
  - `cargo check --all-targets`
  - `cargo clippy --all-targets --no-deps`
  - `wc -l src/main.rs`

## Before vs After

| Metric | Baseline (`D0-01`) | Current (`D3-02`) | Delta |
| --- | ---: | ---: | ---: |
| `cargo check` warnings (`cocode`) | 72 | 13 | -59 |
| `cargo check` warnings (`agent_core`) | 12 | 12 | 0 |
| `cargo check` warnings (total) | 84 | 25 | -59 |
| `cargo clippy` warnings (`cocode`) | 334 | 249 | -85 |
| `cargo clippy` warnings (`agent_core`) | 12 | 12 | 0 |
| `cargo clippy` warnings (total) | 346 | 261 | -85 |
| `src/main.rs` LOC | 12,225 | 11,686 | -539 |

## Current Top Clippy Findings

| Lint | Count |
| --- | ---: |
| `clippy::collapsible_if` | 132 |
| `clippy::needless_return` | 26 |
| `dead_code` | 20 |
| `clippy::needless_borrows_for_generic_args` | 16 |
| `clippy::vec_init_then_push` | 14 |

## Remaining Debt Register

| Priority | Debt Item | Evidence | Suggested Next Action |
| --- | --- | --- | --- |
| P0 | High volume `clippy::collapsible_if` in root crate | 132 warnings dominate clippy output | Batch-fix by subsystem with behavior-preserving rewrites and small reviewable commits |
| P1 | `needless_return` noise still obscures high-signal warnings | 26 warnings in root crate | Normalize return style in touched files during follow-up debt passes |
| P1 | Dead code still present in first-party crates | 20 `dead_code` warnings remain | Remove unused fields/functions or gate with feature flags where retention is intentional |
| P2 | Argument-heavy APIs reduce maintainability | 6 `clippy::too_many_arguments` warnings | Introduce focused parameter structs at module boundaries |
| P2 | Main entrypoint is still large after extraction | `src/main.rs` remains 11,686 LOC | Continue D2-style extraction for session lifecycle and command orchestration paths |

## Reproduction

```bash
cargo check --all-targets --message-format=json
cargo clippy --all-targets --no-deps --message-format=json
wc -l src/main.rs
```
