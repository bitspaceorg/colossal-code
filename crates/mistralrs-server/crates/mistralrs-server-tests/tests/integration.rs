use std::sync::Arc;
#[cfg(feature = "mock-manager")]
use std::env;

use axum::body::{to_bytes, Body};
use axum::http::Request;
#[cfg(feature = "mock-manager")]
use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};
use tower::util::ServiceExt;
use tokio::sync::RwLock;

use mistralrs_server_api::{build_router, AppState, AuthState, HttpMetrics};
use mistralrs_server_config::{ConfigManager, ConfigSource, ServerConfig};
#[cfg(feature = "mock-manager")]
use mistralrs_server_config::RateLimitSection;
use mistralrs_server_core::{DynModelManager, NoopScheduler};
#[cfg(feature = "mock-manager")]
use mistralrs_server_core::ManagerConfig;
#[cfg(feature = "mock-manager")]
use mistralrs_server_core::{InMemoryModelManager, LoadModelRequest, SystemClock};

#[cfg(feature = "mock-manager")]
async fn build_app(config: ServerConfig) -> Router {
    let manager: DynModelManager = Arc::new(InMemoryModelManager::new(
        ManagerConfig {
            keep_alive_default: std::time::Duration::from_secs(5),
            max_loaded_models: 2,
            max_parallel_requests_per_model: 2,
        },
        Arc::new(NoopScheduler),
        SystemClock,
    ));
    manager
        .load_model(LoadModelRequest {
            model: "demo".into(),
            keep_alive: None,
            pinned: false,
        })
        .await
        .unwrap();
    let config_manager = ConfigManager::load(ConfigSource::Inline(config))
        .await
        .expect("config manager");
    let cfg_snapshot = config_manager.get().await;
    let metrics = HttpMetrics::new().unwrap();
    
    let factory: mistralrs_server_api::ManagerFactory = Arc::new(|cfg: &ServerConfig, scheduler, _| {
        let manager: DynModelManager = Arc::new(InMemoryModelManager::new(
            ManagerConfig::from(cfg),
            scheduler,
            SystemClock,
        ));
        Box::pin(async move { Ok(manager) })
    });

    let state = AppState {
        manager: Arc::new(RwLock::new(manager)),
        factory,
        config: config_manager,
        scheduler: Arc::new(NoopScheduler),
        model_metrics: metrics.model_metrics(),
        metrics,
        auth: AuthState::from_section(&cfg_snapshot.auth),
    };
    build_router(state).await.unwrap()
}

#[cfg(feature = "mock-manager")]
#[tokio::test]
async fn generate_returns_envelope() {
    let app = build_app(ServerConfig::default()).await;
    let body = json!({
        "model": "demo",
        "prompt": "hello",
        "stream": false
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/generate")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("data").is_some());
    assert!(value
        .get("meta")
        .and_then(|meta| meta.get("request_id"))
        .is_some());
}

#[cfg(feature = "mock-manager")]
#[tokio::test]
async fn streaming_generate_emits_sse() {
    let app = build_app(ServerConfig::default()).await;
    let body = json!({
        "model": "demo",
        "prompt": "stream",
        "stream": true
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/generate")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers.get("content-type").unwrap(),
        "text/event-stream"
    );
    assert!(headers.get("x-request-id").is_some());
    let payload = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(payload.to_vec()).unwrap();
    assert!(body.contains("\"meta\""));
    assert!(body.contains("\"data\""));
}

#[cfg(feature = "mock-manager")]
#[tokio::test]
async fn auth_rate_limit_is_enforced() {
    let mut config = ServerConfig::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec!["secret".into()];
    config.auth.rate_limit = Some(RateLimitSection {
        requests_per_minute: 1,
    });
    let app = build_app(config).await;
    let request = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/api/generate")
            .header("content-type", "application/json")
            .header("x-api-key", "secret")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let body = json!({"model": "demo", "prompt": "first"});
    let response = app.clone().oneshot(request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json!({"model": "demo", "prompt": "second"});
    let response = app.oneshot(request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value
        .get("meta")
        .and_then(|meta| meta.get("request_id"))
        .is_some());
}

#[cfg(feature = "mock-manager")]
#[tokio::test]
async fn admin_evict_offloads_model() {
    let app = build_app(ServerConfig::default()).await;
    let body = json!({
        "models": ["demo"]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/evict")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    
    let data = value["data"].as_array().expect("data is array");
    assert_eq!(data.len(), 1);
    assert_eq!(data[0], "demo");
}

#[cfg(feature = "mock-manager")]
#[tokio::test]
async fn reload_config_applies_overrides() {
    let app = build_app(ServerConfig::default()).await;
    env::set_var("MISTRALRS__SERVER__PORT", "9090");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/reload-config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    env::remove_var("MISTRALRS__SERVER__PORT");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let port = value
        .get("data")
        .and_then(|data| data.get("server"))
        .and_then(|server| server.get("port"))
        .unwrap()
        .as_u64()
        .unwrap();
    assert_eq!(port, 9090);
}

#[cfg(feature = "real-manager")]
mod real_manager {
    use super::*;
    use async_trait::async_trait;
    use axum::http::StatusCode;
    use mistralrs_core::{
        ChatCompletionChunkResponse,
        ChatCompletionResponse,
        Choice,
        ChunkChoice,
        CompletionChoice,
        CompletionChunkChoice,
        CompletionChunkResponse,
        CompletionResponse,
        Delta,
        NormalRequest,
        Request as EngineRequest,
        RequestMessage,
        Response as EngineResponse,
        ResponseMessage,
        Tool as CoreTool,
        ToolChoice,
        Usage as EngineUsage,
        EmbeddingResponse as EngineEmbeddingResponse,
    };
    use mistralrs_server_config::{ModelConfig, MistralBuilderConfig};
    use mistralrs_server_core::{
        EngineHandle, ManagerConfig, MistralModelManager, ModelManagerError, RuntimeAdapters,
    };
    use std::{sync::Mutex, time::Duration};
    use tokio::time::sleep;

    struct Harness {
        app: Router,
        engine: Arc<MockEngine>,
    }

    async fn harness() -> Harness {
        let mut cfg = ServerConfig::default();
        let mut model = ModelConfig::default();
        model.model_id = "demo".into();
        model.source = "hf://demo".into();
        model.default = true;
        model.pinned = true;
        cfg.models.insert("demo".into(), model);
        let config_manager = ConfigManager::load(ConfigSource::Inline(cfg))
            .await
            .expect("config");
        let cfg_snapshot = config_manager.get().await;
        let builder_cfg = MistralBuilderConfig::try_from(&cfg_snapshot).expect("builder config");
        let manager_cfg = ManagerConfig::from(&cfg_snapshot);
        let metrics = HttpMetrics::new().expect("metrics");
        let scheduler: Arc<dyn mistralrs_server_core::ModelScheduler> = Arc::new(NoopScheduler);
        let engine = Arc::new(MockEngine::new());
        let model_metrics = metrics.model_metrics();
        let manager = Arc::new(
            MistralModelManager::from_components(
                builder_cfg,
                manager_cfg,
                scheduler.clone(),
                engine.clone() as Arc<dyn EngineHandle>,
                None,
                metrics.registry(),
                model_metrics.clone(),
                RuntimeAdapters::current(),
            )
            .expect("manager"),
        ) as DynModelManager;
        
        let factory: mistralrs_server_api::ManagerFactory = Arc::new(|cfg: &ServerConfig, scheduler, metrics| {
             // In real harness, we might just return the existing manager or create a new one.
             // For simplicity, let's just error or create a dummy if called.
             // Ideally we would construct a new MistralModelManager with the SAME MockEngine.
             // But MockEngine is local here.
             // Since we don't test reload in real_manager harness yet, a dummy is fine.
             Box::pin(async move { 
                 Err(mistralrs_server_core::ModelManagerError::Other("reload not supported in harness".into()))
             })
        });

        let auth = AuthState::from_section(&cfg_snapshot.auth);
        let state = AppState {
            manager: Arc::new(RwLock::new(manager)),
            factory,
            config: config_manager,
            scheduler,
            model_metrics,
            metrics,
            auth,
        };
        let app = build_router(state).await.expect("router");
        Harness { app, engine }
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        streaming: bool,
        kind: RequestKind,
        tool_names: Vec<String>,
        forced_tool: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq)]
    enum RequestKind {
        Completion,
        Chat,
        Embeddings,
    }

    #[derive(Clone, Default)]
    struct MockEngine {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    impl MockEngine {
        fn new() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new()))
            }
        }

        fn take_requests(&self) -> Vec<RecordedRequest> {
            let mut guard = self.requests.lock().expect("requests lock");
            let taken = guard.clone();
            guard.clear();
            taken
        }

        fn record(
            &self,
            streaming: bool,
            tools: Option<Vec<CoreTool>>,
            tool_choice: Option<ToolChoice>,
            kind: RequestKind,
        ) {
            let tool_names = tools
                .unwrap_or_default()
                .into_iter()
                .map(|tool| tool.function.name)
                .collect();
            let forced_tool = tool_choice.and_then(|choice| match choice {
                ToolChoice::Tool(tool) => Some(tool.function.name),
                _ => None,
            });
            let mut guard = self.requests.lock().expect("requests lock");
            guard.push(RecordedRequest {
                streaming,
                kind,
                tool_names,
                forced_tool,
            });
        }

        fn completion_done(text: String) -> EngineResponse {
            EngineResponse::CompletionDone(CompletionResponse {
                id: "cmp".into(),
                choices: vec![CompletionChoice {
                    finish_reason: "stop".into(),
                    index: 0,
                    text,
                    logprobs: None,
                }],
                created: 0,
                model: "demo".into(),
                system_fingerprint: "test".into(),
                object: "text_completion".into(),
                usage: Self::usage(),
            })
        }

        fn completion_chunk() -> EngineResponse {
            EngineResponse::CompletionChunk(CompletionChunkResponse {
                id: "chunk".into(),
                choices: vec![CompletionChunkChoice {
                    text: "partial".into(),
                    index: 0,
                    logprobs: None,
                    finish_reason: None,
                }],
                created: 0,
                model: "demo".into(),
                system_fingerprint: "test".into(),
                object: "text_completion".into(),
            })
        }

        fn chat_chunk() -> EngineResponse {
            EngineResponse::Chunk(ChatCompletionChunkResponse {
                id: "chat".into(),
                choices: vec![ChunkChoice {
                    finish_reason: None,
                    index: 0,
                    delta: Delta {
                        content: Some("delta".into()),
                        role: "assistant".into(),
                        tool_calls: None,
                    },
                    logprobs: None,
                }],
                created: 0,
                model: "demo".into(),
                system_fingerprint: "test".into(),
                object: "chat.completion.chunk".into(),
                usage: None,
            })
        }

        fn chat_done() -> EngineResponse {
            EngineResponse::Done(ChatCompletionResponse {
                id: "chat".into(),
                choices: vec![Choice {
                    finish_reason: "stop".into(),
                    index: 0,
                    message: ResponseMessage {
                        content: Some("full".into()),
                        role: "assistant".into(),
                        tool_calls: None,
                    },
                    logprobs: None,
                }],
                created: 0,
                model: "demo".into(),
                system_fingerprint: "test".into(),
                object: "chat.completion".into(),
                usage: Self::usage(),
            })
        }

        fn usage() -> EngineUsage {
            EngineUsage {
                completion_tokens: 1,
                prompt_tokens: 1,
                total_tokens: 2,
                avg_tok_per_sec: 0.0,
                avg_prompt_tok_per_sec: 0.0,
                avg_compl_tok_per_sec: 0.0,
                total_time_sec: 0.0,
                total_prompt_time_sec: 0.0,
                total_completion_time_sec: 0.0,
            }
        }
    }

    #[async_trait]
    impl EngineHandle for MockEngine {
        async fn ensure_model_loaded(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn send_request_with_model(
            &self,
            request: EngineRequest,
            model: Option<&str>,
        ) -> Result<(), ModelManagerError> {
            match request {
                EngineRequest::Normal(normal) => {
                    self.handle_request(*normal, model).await;
                }
                EngineRequest::Embedding(req) => {
                    self.handle_embedding_request(req).await;
                }
                _ => {}
            }
            Ok(())
        }

        async fn remove_model(&self, _model: &str) -> Result<(), ModelManagerError> {
            Ok(())
        }

        async fn list_models(&self) -> Result<Vec<String>, ModelManagerError> {
            Ok(vec!["demo".into()])
        }
    }

    impl MockEngine {
        async fn handle_request(&self, req: NormalRequest, _model: Option<&str>) {
            let NormalRequest {
                messages,
                response,
                is_streaming,
                tools,
                tool_choice,
                .. 
            } = req;
            match messages {
                RequestMessage::Completion { text: _, .. } => {
                    self.record(is_streaming, tools, tool_choice, RequestKind::Completion);
                    if is_streaming {
                        let _ = response.send(Self::completion_chunk()).await;
                    }
                    let _ = response.send(Self::completion_done("all done".into())).await;
                }
                RequestMessage::Chat { .. } => {
                    self.record(is_streaming, tools, tool_choice, RequestKind::Chat);
                    if is_streaming {
                        let _ = response.send(Self::chat_chunk()).await;
                    }
                    let _ = response.send(Self::chat_done()).await;
                }
                _ => {
                    let _ = response
                        .send(EngineResponse::InternalError(Box::new(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "unsupported",
                        ))))
                        .await;
                }
            }
        }

        async fn handle_embedding_request(
            &self,
            req: mistralrs_core::EmbeddingRequest,
        ) {
            self.record(false, None, None, RequestKind::Embeddings);
            let _ = req
                .response
                .send(Self::embedding_response(req.inputs))
                .await;
        }

        fn embedding_response(inputs: Vec<String>) -> EngineResponse {
            let data = inputs
                .into_iter()
                .enumerate()
                .map(|(index, value)| mistralrs_core::EmbeddingData {
                    object: "embedding".to_string(),
                    embedding: vec![value.len() as f32 + index as f32],
                    index,
                })
                .collect();
            EngineResponse::Embedding(EngineEmbeddingResponse {
                id: "emb".into(),
                object: "list".into(),
                model: "demo".into(),
                data,
                usage: Self::usage(),
            })
        }
    }

    #[tokio::test]
    async fn streaming_completion_uses_envelope() {
        let harness = harness().await;
        let body = json!({
            "model": "demo",
            "prompt": "stream",
            "stream": true
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/generate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response.headers().get("x-request-id").cloned();
        assert!(request_id.is_some());
        let payload = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(payload.to_vec()).unwrap();
        assert!(text.contains("\"meta\""));
        assert!(text.contains("\"data\""));
        let recorded = harness.engine.take_requests();
        assert_eq!(recorded.len(), 1);
        assert!(matches!(recorded[0].kind, RequestKind::Completion));
        assert!(recorded[0].streaming);
    }

    #[tokio::test]
    async fn chat_tools_are_forwarded() {
        let harness = harness().await;
        let body = json!({
            "model": "demo",
            "stream": false,
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "use tool"}]
                }
            ],
            "tools": [{
                "name": "lookup",
                "description": "demo",
                "parameters": {"type": "object", "properties": {}}
            }],
            "tool_choice": {"tool": "lookup"}
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/chat")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let recorded = harness.engine.take_requests();
        assert_eq!(recorded.len(), 1);
        assert!(matches!(recorded[0].kind, RequestKind::Chat));
        assert_eq!(recorded[0].tool_names, vec!["lookup".to_string()]);
        assert_eq!(recorded[0].forced_tool.as_deref(), Some("lookup"));
    }

    #[tokio::test]
    async fn embeddings_endpoint_returns_vectors() {
        let harness = harness().await;
        let single = json!({
            "model": "demo",
            "input": "hello",
            "normalize": true
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/embeddings")
                    .header("content-type", "application/json")
                    .body(Body::from(single.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(value["data"]["embeddings"].as_array().unwrap().len(), 1);

        let multiple = json!({
            "model": "demo",
            "input": ["a", "bb"],
            "normalize": false
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/embeddings")
                    .header("content-type", "application/json")
                    .body(Body::from(multiple.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(value["data"]["embeddings"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn logs_include_stream_entries() {
        let harness = harness().await;
        let body = json!({
            "model": "demo",
            "prompt": "log test",
            "stream": true
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/generate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let logs = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/logs/demo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logs.status(), StatusCode::OK);
        let bytes = to_bytes(logs.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("data").is_some());
    }

    #[tokio::test]
    async fn pull_jobs_reach_succeeded_state() {
        let harness = harness().await;
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pull")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({
                        "model": "demo",
                        "source": "hf://repo"
                    }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let job_id = value
            .get("data")
            .and_then(|data| data.as_str())
            .expect("job id");
        let mut attempts = 0;
        loop {
            let uri = format!("/api/jobs/{job_id}");
            let status_resp = harness
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(status_resp.status(), StatusCode::OK);
            let body = to_bytes(status_resp.into_body(), usize::MAX).await.unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            let status = value
                .get("data")
                .and_then(|data| data.get("status"))
                .and_then(|status| status.as_str())
                .unwrap();
            if status == "succeeded" {
                break;
            }
            attempts += 1;
            if attempts > 20 {
                panic!("job status did not complete");
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn openai_chat_completions_schema() {
        let harness = harness().await;
        let body = json!({
            "model": "demo",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        
        assert!(value.get("id").is_some());
        assert_eq!(value["object"], "chat.completion");
        assert!(value["choices"].is_array());
        assert_eq!(value["choices"][0]["message"]["content"], "full");
        assert_eq!(value["choices"][0]["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn openai_chat_completions_streaming() {
        let harness = harness().await;
        let body = json!({
            "model": "demo",
            "messages": [{"role": "user", "content": "stream"}],
            "stream": true
        });
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("content-type").unwrap(), "text/event-stream");
        
        let payload = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(payload.to_vec()).unwrap();
        
        // Check for data: prefix
        assert!(body.contains("data: {"));
        // Check for [DONE]
        assert!(body.contains("data: [DONE]"));
        // Check that it DOES NOT contain the custom envelope fields in the event data
        assert!(!body.contains("\"meta\":"));
    }
}