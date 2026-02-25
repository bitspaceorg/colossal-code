# Debt Pass V4 Summary

Captured on 2026-02-25 for task `V4-11`.

## Scope

- Baseline reference: `docs/debt/baseline-v4.md` (`V4-00`)
- Current command set:
  - `cargo check --all-targets`
  - `cargo clippy --all-targets --no-deps`
  - `wc -l src/main.rs`

## Before vs After

| Metric | Baseline (`V4-00`) | Current (`V4-11`) | Delta |
| --- | ---: | ---: | ---: |
| `cargo check` warnings (`cocode`) | 13 | 0 | -13 |
| `cargo check` warnings (`agent_core`) | 12 | 12 | 0 |
| `cargo check` warnings (total) | 25 | 12 | -13 |
| `cargo clippy` warnings (`cocode`) | 249 | 162 | -87 |
| `cargo clippy` warnings (`agent_core`) | 12 | 12 | 0 |
| `cargo clippy` warnings (total) | 261 | 174 | -87 |
| `src/main.rs` LOC | 11,682 | 11,190 | -492 |

## Current Top Clippy Findings

| Lint | Count |
| --- | ---: |
| `clippy::collapsible_if` | 120 |
| `clippy::vec_init_then_push` | 14 |
| `dead_code` | 7 |
| `clippy::redundant_closure` | 4 |
| `clippy::if_same_then_else` | 4 |

## Remaining Debt Register

| Priority | Debt Item | Evidence | Suggested Next Action |
| --- | --- | --- | --- |
| P0 | High-volume nested-condition cleanup | `clippy::collapsible_if` still accounts for 120 warnings | Continue module-by-module collapsible-if cleanup with behavior-preserving snapshots |
| P1 | Vector construction style noise | 14 `clippy::vec_init_then_push` warnings remain | Convert push-after-init patterns to `vec![..]` in touched files |
| P1 | Residual dead code in first-party crates | 7 `dead_code` warnings remain | Remove unused fields/functions or gate them when intentionally retained |
| P2 | Main entrypoint still broad after extractions | `src/main.rs` remains 11,190 LOC | Continue targeted extraction for CLI flow orchestration and command dispatch |

## Follow-up Plan

1. Run a focused P0 pass on `clippy::collapsible_if` in `src/` and submit small reviewable commits.
2. In the same touched files, clear `vec_init_then_push` and `dead_code` warnings to reduce low-signal lint noise.
3. Land one additional `src/main.rs` extraction slice and re-baseline debt metrics after merge.

## Reproduction

```bash
cargo check --all-targets --message-format=json
cargo clippy --all-targets --no-deps --message-format=json
wc -l src/main.rs
```
