# Debt Baseline Report

Captured on 2026-02-25 for task `D0-01`.

## Scope

- Workspace: first-party crates in this repository
- Check command: `cargo check --all-targets`
- Clippy command: `cargo clippy --all-targets --no-deps`
- Main file metric: `wc -l src/main.rs`

## `cargo check --all-targets` warnings by crate

| Crate | Warning count |
| --- | ---: |
| `cocode` | 72 |
| `agent_core` | 12 |
| **Total** | **84** |

## `cargo clippy --all-targets --no-deps` top findings

Total warnings: **346**

Top lint categories (by count):

| Lint | Count |
| --- | ---: |
| `clippy::collapsible_if` | 150 |
| `unused_variables` | 32 |
| `dead_code` | 26 |
| `clippy::needless_return` | 26 |
| `clippy::needless_borrows_for_generic_args` | 16 |
| `clippy::vec_init_then_push` | 14 |
| `unused_imports` | 9 |
| `unused_mut` | 6 |
| `clippy::if_same_then_else` | 6 |
| `clippy::manual_range_patterns` | 6 |

Clippy warning totals by crate:

| Crate | Warning count |
| --- | ---: |
| `cocode` | 334 |
| `agent_core` | 12 |

## `src/main.rs` LOC

- `src/main.rs`: **12,225 LOC**

## Reproduction

```bash
cargo check --all-targets --message-format=json
cargo clippy --all-targets --no-deps --message-format=json
wc -l src/main.rs
```
