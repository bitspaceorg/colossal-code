# Debt Baseline V4 Report

Captured on 2026-02-25 for task `V4-00`.

## Scope

- Workspace: first-party crates in this repository
- Check command: `cargo check --all-targets`
- Clippy command: `cargo clippy --all-targets --no-deps`
- Main file metric: `wc -l src/main.rs`

## `cargo check --all-targets` warnings by crate

| Crate | Warning count |
| --- | ---: |
| `cocode` | 13 |
| `agent_core` | 12 |
| **Total** | **25** |

## `cargo clippy --all-targets --no-deps` top findings

Total warnings: **261**

Top lint categories (by count):

| Lint | Count |
| --- | ---: |
| `clippy::collapsible_if` | 132 |
| `clippy::needless_return` | 26 |
| `dead_code` | 20 |
| `clippy::needless_borrows_for_generic_args` | 16 |
| `clippy::vec_init_then_push` | 14 |
| `clippy::if_same_then_else` | 6 |
| `clippy::too_many_arguments` | 6 |
| `clippy::clone_on_copy` | 4 |
| `clippy::redundant_closure` | 4 |
| `clippy::wrong_self_convention` | 4 |

Clippy warning totals by crate:

| Crate | Warning count |
| --- | ---: |
| `cocode` | 249 |
| `agent_core` | 12 |

## `src/main.rs` LOC

- `src/main.rs`: **11,682 LOC**

## Reproduction

```bash
cargo check --all-targets --message-format=json
cargo clippy --all-targets --no-deps --message-format=json
wc -l src/main.rs
```
