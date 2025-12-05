use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, Stream as FuturesStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use mistralrs::{
    ChatCompletionChunkResponse, ChatCompletionResponse, Choice, ChunkChoice, Delta, GgufModelBuilder,
    Model, RequestBuilder, Response, ResponseMessage, Usage,
};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[async_trait]
pub trait LLMBackend: Send + Sync {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>>;
    
    async fn load_model(&self) -> Result<()>;
    
    async fn reload_model(&self, model_filename: String) -> Result<()>;

    async fn get_model(&self) -> Result<Arc<Model>>;
}

pub struct LocalBackend {
    model: Arc<Mutex<Option<Arc<Model>>>>,
    model_path: String,
    model_files: Arc<Mutex<Vec<String>>>,
}

impl LocalBackend {
    pub fn new(model_path: String, model_files: Vec<String>) -> Self {
        Self {
            model: Arc::new(Mutex::new(None)),
            model_path,
            model_files: Arc::new(Mutex::new(model_files)),
        }
    }
}

#[async_trait]
impl LLMBackend for LocalBackend {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        let model_guard = self.model.lock().await;
        let model = model_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model not loaded"))?
            .clone();
        drop(model_guard);

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            if let Ok(mut stream) = model.stream_chat_request(request).await {
                while let Some(response) = stream.next().await {
                    if tx.send(response).is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Box::new(UnboundedReceiverStream::new(rx)))
    }

    async fn get_model(&self) -> Result<Arc<Model>> {
         let model_guard = self.model.lock().await;
         if let Some(m) = model_guard.as_ref() {
             return Ok(m.clone());
         }
         drop(model_guard);
         self.load_model().await?;
         let model_guard = self.model.lock().await;
         Ok(model_guard.as_ref().unwrap().clone())
    }

    async fn load_model(&self) -> Result<()> {
        let mut model_guard = self.model.lock().await;
        if model_guard.is_none() {
             // Redirect stdout/stderr to /dev/null during model loading to suppress progress bars
             // (Platform specific loading logic omitted for brevity in this first pass, but included in full implementation)
            let model_files = self.model_files.lock().await.clone();
            let model = GgufModelBuilder::new(
                self.model_path.clone(),
                model_files,
            )
            .build()
            .await?;

            *model_guard = Some(Arc::new(model));
        }
        Ok(())
    }

    async fn reload_model(&self, new_model_filename: String) -> Result<()> {
        let mut model_guard = self.model.lock().await;
        *model_guard = None;
        drop(model_guard);

        let mut model_files_guard = self.model_files.lock().await;
        *model_files_guard = vec![new_model_filename];
        drop(model_files_guard);
        Ok(())
    }
}

pub struct HttpBackend {
    client: Client,
    base_url: String,
    api_key: String,
    model: Mutex<String>,
    completions_path: String,
    requires_model_load: bool,
}

impl HttpBackend {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        completions_path: String,
        requires_model_load: bool,
    ) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        let completions_path = if completions_path.starts_with('/') {
            completions_path
        } else {
            format!("/{}", completions_path)
        };
        Self {
            client: Client::new(),
            base_url,
            api_key,
            model: Mutex::new(model),
            completions_path,
            requires_model_load,
        }
    }

    fn auth_header(&self) -> Option<String> {
        if self.api_key.trim().is_empty() {
            None
        } else {
            Some(format!("Bearer {}", self.api_key))
        }
    }
}

#[async_trait]
impl LLMBackend for HttpBackend {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn FuturesStream<Item = Response> + Unpin + Send>> {
        let messages = request.messages();
        let model = {
            let guard = self.model.lock().await;
            guard.clone()
        };

        let mut openai_messages = Vec::new();
        for msg in messages {
            match serde_json::to_value(msg) {
                Ok(value) => openai_messages.push(value),
                Err(_) => openai_messages.push(serde_json::json!({
                    "role": "user",
                    "content": ""
                })),
            }
        }

        let payload = serde_json::json!({
            "model": model,
            "messages": openai_messages,
            "stream": false,
        });

        let mut request = self
            .client
            .post(format!(
                "{}/{}",
                self.base_url,
                self.completions_path.trim_start_matches('/')
            ))
            .json(&payload);

        if let Some(header) = self.auth_header() {
            request = request.header("Authorization", header);
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        let chat_response: OpenAiChatResponse = response.json().await?;
        let id = chat_response
            .id
            .clone()
            .unwrap_or_else(|| format!("nite-http-{}", Uuid::new_v4()));
        let model_name = chat_response
            .model
            .clone()
            .unwrap_or_else(|| model.clone());
        let created = chat_response.created.unwrap_or_else(|| current_timestamp());
        let system_fingerprint = chat_response
            .system_fingerprint
            .clone()
            .unwrap_or_default();
        let content = chat_response
            .choices
            .get(0)
            .map(|choice| choice.message.content.clone())
            .unwrap_or_default();
        let role = chat_response
            .choices
            .get(0)
            .and_then(|choice| choice.message.role.clone())
            .unwrap_or_else(|| "assistant".to_string());
        let finish_reason = chat_response
            .choices
            .get(0)
            .and_then(|choice| choice.finish_reason.clone())
            .unwrap_or_else(|| "stop".to_string());

        let chunk_choice = ChunkChoice {
            finish_reason: Some(finish_reason.clone()),
            index: 0,
            delta: Delta {
                content: Some(content.clone()),
                role: role.clone(),
                tool_calls: None,
            },
            logprobs: None,
        };

        let chunk = ChatCompletionChunkResponse {
            id: id.clone(),
            choices: vec![chunk_choice],
            created: created as u128,
            model: model_name.clone(),
            system_fingerprint: system_fingerprint.clone(),
            object: chat_response
                .object
                .clone()
                .unwrap_or_else(|| "chat.completion.chunk".to_string()),
            usage: None,
        };

        let usage = chat_response
            .usage
            .clone()
            .map(|usage| Usage {
                completion_tokens: usage.completion_tokens.unwrap_or_else(|| estimate_tokens(&content)),
                prompt_tokens: usage.prompt_tokens.unwrap_or(0),
                total_tokens: usage.total_tokens.unwrap_or_else(|| estimate_tokens(&content)),
                avg_tok_per_sec: 0.0,
                avg_prompt_tok_per_sec: 0.0,
                avg_compl_tok_per_sec: 0.0,
                total_time_sec: 0.0,
                total_prompt_time_sec: 0.0,
                total_completion_time_sec: 0.0,
            })
            .unwrap_or_else(|| Usage {
                completion_tokens: estimate_tokens(&content),
                prompt_tokens: 0,
                total_tokens: estimate_tokens(&content),
                avg_tok_per_sec: 0.0,
                avg_prompt_tok_per_sec: 0.0,
                avg_compl_tok_per_sec: 0.0,
                total_time_sec: 0.0,
                total_prompt_time_sec: 0.0,
                total_completion_time_sec: 0.0,
            });

        let response_choice = Choice {
            finish_reason,
            index: 0,
            message: ResponseMessage {
                content: Some(content.clone()),
                role,
                tool_calls: None,
            },
            logprobs: None,
        };

        let done = ChatCompletionResponse {
            id,
            choices: vec![response_choice],
            created,
            model: model_name,
            system_fingerprint,
            object: chat_response
                .object
                .unwrap_or_else(|| "chat.completion".to_string()),
            usage,
        };

        let response_stream = stream::iter(vec![Response::Chunk(chunk), Response::Done(done)]);
        Ok(Box::new(response_stream))
    }

    async fn load_model(&self) -> Result<()> {
        if !self.requires_model_load {
            return Ok(());
        }

        let model = {
            let guard = self.model.lock().await;
            guard.clone()
        };

        let mut request = self
            .client
            .post(format!("{}/api/load", self.base_url))
            .json(&serde_json::json!({
                "model": model,
                "keep_alive": serde_json::Value::Null,
                "pinned": false,
            }));

        if let Some(header) = self.auth_header() {
            request = request.header("Authorization", header);
        }

        let response = request.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "HTTP backend failed to load model: {}",
                error_text
            ));
        }
        Ok(())
    }

    async fn reload_model(&self, model_identifier: String) -> Result<()> {
        let mut guard = self.model.lock().await;
        *guard = model_identifier;
        Ok(())
    }
    
    async fn get_model(&self) -> Result<Arc<Model>> {
        Err(anyhow::anyhow!("Direct model access not supported in HttpBackend"))
    }
}

#[derive(Debug, Deserialize, Clone)]
struct OpenAiUsage {
    completion_tokens: Option<usize>,
    prompt_tokens: Option<usize>,
    total_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatMessage {
    content: String,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatChoice {
    message: OpenAiChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    id: Option<String>,
    model: Option<String>,
    created: Option<u64>,
    system_fingerprint: Option<String>,
    object: Option<String>,
    choices: Vec<OpenAiChatChoice>,
    usage: Option<OpenAiUsage>,
}

fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
