# cocode (Nite TUI)

`cocode` is a Rust terminal UI for running an agent-driven coding workflow with local or external backends.

## Features

- Terminal-first chat and coding workflow.
- Slash command system (`/help`, `/spec`, `/review`, `/rewind`, `/resume`, `/model`, `/safety`, `/todos`, etc.).
- Spec/orchestrator flow with step planning, status tracking, and session lifecycle updates.
- Rewind and conversation persistence (`~/.config/.nite/nite.conf` and saved conversation data).
- Optional CUDA/Flash-Attention/CuDNN acceleration through feature flags.

## Repository Layout

- `src/main.rs`: composition root and startup wiring.
- `src/app/`: application modules (commands, init, input, orchestrator, persistence, render, runtime, state).
- `crates/`: internal crates used by the binary (`agent_core`, `edtui`, `sessionizer`, etc.).
- `tests/`: integration and boundary regression tests.

## Prerequisites

- Rust toolchain with Cargo (edition 2024 compatible toolchain).
- Git submodules initialized (this repo depends on external submodules).
- Linux terminal environment (project currently tuned for Linux workflows).

For GPU builds:

- CUDA Toolkit 12.9
- CuDNN libraries
- GCC/G++ 14 toolchain (host compiler for NVCC)

## Installation and Build

### 1) Clone and initialize submodules

```bash
git clone <your-repo-url> tui
cd tui
git submodule update --init --recursive
```

### 2) Build and run (CPU/default)

```bash
cargo run --release -- --workspace-root ~/rust/todo
```

### 3) Build and run (CUDA + flash-attn + cudnn)

Use the environment setup you shared before compilation:

```bash
export PATH=/usr/local/cuda-12.9/bin:$PATH
export LD_LIBRARY_PATH=/usr/local/cuda-12.9/targets/x86_64-linux/lib:$LD_LIBRARY_PATH
export LD_LIBRARY_PATH=/usr/local/cuda-12.9/lib64:$LD_LIBRARY_PATH
export CPATH=/usr/local/cuda-12.9/targets/x86_64-linux/include:$CPATH
export CUDA_HOME=/usr/local/cuda-12.9
export CUDNN_PATH=/usr/lib64

export CC=/usr/bin/gcc-14
export CXX=/usr/bin/g++-14
export NVCC_CCBIN=/usr/bin/g++-14
export CUDAHOSTCXX=/usr/bin/g++-14

cargo run --release --features "cuda flash-attn cudnn" -- --workspace-root ~/rust/todo
```

## Runtime CLI Arguments

- `--workspace-root <path>`: set workspace root used by the app/agent.
- `--spec <path>`: preload a spec file at startup.

## Configuration

Config file path:

- `~/.config/.nite/nite.conf`

Common keys:

- `backend` (`local`, `external`, `http`)
- `http-base-url`
- `http-api-key`
- `http-completions-path`
- `google-user-project`
- `model`
- `vim-keybind`
- `auto-summarize-threshold`
- `auto-summarize-threshold-version`

Notes:

- Comments and blank lines in `nite.conf` are preserved on config updates.
- Unknown keys are ignored by runtime logic.

## Development

Run the full test suite:

```bash
cargo test -p cocode
```

Run module boundary regression tests:

```bash
cargo test --test module_boundary_regressions
```

## Troubleshooting

- If CUDA builds fail, re-check `PATH`, `LD_LIBRARY_PATH`, `CUDA_HOME`, and GCC/NVCC host compiler env vars.
- If submodule code appears missing, run:

```bash
git submodule update --init --recursive
```
- If backend startup fails, verify `backend`, `http-base-url`, and `http-api-key` values in `nite.conf`.
