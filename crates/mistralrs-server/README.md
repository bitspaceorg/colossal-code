# mistralrs-server

A modular Axum service that exposes mistralrs-compatible models via HTTP, supporting multiple concurrent models, OpenAI-compatible endpoints, and advanced scheduling.

## Features

- **Multiple Models**: Load and switch between multiple models (LLMs, vision, etc.) dynamically.
- **OpenAI Compatibility**: Drop-in replacement for OpenAI's Chat Completion API (`/v1/chat/completions`).
- **Streaming**: Full SSE support for chat and generation endpoints.
- **Advanced Scheduling**: LRU eviction, concurrent request limits, and GPU memory management (paged attention).
- **Observability**: Built-in Prometheus metrics and request logging.
- **Admin API**: Dynamic model loading, unloading, and configuration reloading.

## Quickstart

### Prerequisites

- Rust 1.75+
- Hugging Face Token (for gated models)

### Running the Server

1.  **Clone and Build:**

    ```bash
    git clone https://github.com/EricLBuehler/mistral.rs.git
    cd mistral.rs/crates/mistralrs-server
    ```

2.  **Set HF Token (Optional):**

    ```bash
    export HF_TOKEN=your_token_here
    ```

3.  **Run with Default Config:**

    ```bash
    cargo run --bin mistralrs-server -- --config ./config/dev.toml
    ```

    _To run without a real GPU/Model (Mock Mode):_

    ```bash
    cargo run --bin mistralrs-server --features mock-manager -- --config ./config/dev.toml --mock-manager
    ```

## Configuration

The server is configured via a TOML file. Environment variables can override any setting using the `MISTRALRS__` prefix (double underscore separator).

### Config Schema

#### Server (`[server]`)

| Key                             | Type   | Default     | Env Override                                       |
| ------------------------------- | ------ | ----------- | -------------------------------------------------- |
| `port`                          | u16    | `8080`      | `MISTRALRS__SERVER__PORT`                          |
| `host`                          | String | `"0.0.0.0"` | `MISTRALRS__SERVER__HOST`                          |
| `max_total_concurrent_requests` | usize  | `64`        | `MISTRALRS__SERVER__MAX_TOTAL_CONCURRENT_REQUESTS` |

#### Scheduler (`[scheduler]`)

| Key                               | Type     | Default  | Env Override                                            |
| --------------------------------- | -------- | -------- | ------------------------------------------------------- |
| `max_loaded_models`               | usize    | `3`      | `MISTRALRS__SCHEDULER__MAX_LOADED_MODELS`               |
| `keep_alive_default`              | Duration | `"300s"` | `MISTRALRS__SCHEDULER__KEEP_ALIVE_DEFAULT`              |
| `max_parallel_requests_per_model` | usize    | `4`      | `MISTRALRS__SCHEDULER__MAX_PARALLEL_REQUESTS_PER_MODEL` |
| `token_source`                    | String   | `None`   | `MISTRALRS__SCHEDULER__TOKEN_SOURCE`                    |
| `paged_attn`                      | bool     | `None`   | `MISTRALRS__SCHEDULER__PAGED_ATTN`                      |
| `paged_attn_gpu_mem_usage`        | f32      | `None`   | `MISTRALRS__SCHEDULER__PAGED_ATTN_GPU_MEM_USAGE`        |

#### Models (`[models.<id>]`)

| Key            | Type     | Required | Env Override                            |
| -------------- | -------- | -------- | --------------------------------------- |
| `model_id`     | String   | Yes      | `MISTRALRS__MODELS__<ID>__MODEL_ID`     |
| `source`       | String   | Yes      | `MISTRALRS__MODELS__<ID>__SOURCE`       |
| `default`      | bool     | No       | `MISTRALRS__MODELS__<ID>__DEFAULT`      |
| `pinned`       | bool     | No       | `MISTRALRS__MODELS__<ID>__PINNED`       |
| `gpu_ids`      | Vec<i32> | No       | `MISTRALRS__MODELS__<ID>__GPU_IDS`      |
| `quantization` | String   | No       | `MISTRALRS__MODELS__<ID>__QUANTIZATION` |

### Example Config (`config.toml`)

```toml
[server]
port = 8080

[scheduler]
max_loaded_models = 2
token_source = "env:HF_TOKEN"

[models.mistral]
model_id = "mistral"
source = "hf://mistralai/Mistral-7B-Instruct-v0.1"
default = true
gpu_ids = [0]
```

## API Usage

### 1. Generate Text (Custom API)

**POST** `/api/generate`

```bash
curl -X POST http://localhost:8080/api/generate \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mistral",
    "prompt": "Explain quantum computing",
    "stream": false,
    "max_tokens": 100
  }'
```

### 2. Chat Completions (OpenAI Compatible)

**POST** `/v1/chat/completions`

```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mistral",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

### 3. Embeddings

**POST** `/api/embeddings`

```bash
curl -X POST http://localhost:8080/api/embeddings \
  -H "Content-Type: application/json" \
  -d '{
    "model": "mistral",
    "input": ["Hello world", "Machine learning"]
  }'
```

### 4. Admin & Metrics

- **Evict Model:**

    ```bash
    curl -X POST http://localhost:8080/admin/evict -d '{"model_id": "mistral"}'
    ```

- **Metrics (Prometheus):**
    ```bash
    curl http://localhost:8080/metrics
    ```

## Troubleshooting

- **"OutOfMemory" / CUDA Errors**:
    - Reduce `max_loaded_models`.
    - Set `paged_attn_gpu_mem_usage` to a lower value (e.g., `0.8`).
    - Ensure `gpu_ids` are valid.

- **Rate Limits**:
    - Check `server.max_total_concurrent_requests` and `scheduler.max_parallel_requests_per_model`.

- **Model Loading Fails**:
    - Verify `HF_TOKEN` is set.
    - Check internet connection.
    - Verify `source` URL/path is correct.

## Development & Testing

### Running Tests

We provide a mocked test suite that doesn't require a GPU:

```bash
# Run core tests
cargo test

# Run integration tests with mock manager
cargo test -p mistralrs-server-tests --features mock-manager
```

To run the "real manager" tests (which still use a mock engine but real scheduler/manager logic):

```bash
cargo test -p mistralrs-server-tests --features real-manager
```
