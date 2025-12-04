use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use eventsource_stream::Eventsource;
use futures::stream::TryStreamExt;

use mistralrs::{
    ChatCompletionChunkResponse, Device, DeviceMapSetting, GgufModelBuilder, LoaderBuilder,
    Model, RequestBuilder, Response, TokenSource,
};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait]
pub trait LLMBackend: Send + Sync {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn futures::Stream<Item = Response> + Unpin + Send>>;
    
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
    ) -> Result<Box<dyn futures::Stream<Item = Response> + Unpin + Send>> {
        let model_guard = self.model.lock().await;
        let model = model_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model not loaded"))?
            .clone();
        drop(model_guard);

        let stream = model.stream_chat_request(request).await?;
        Ok(Box::new(stream))
    }

    async fn get_model(&self) -> Result<Arc<Model>> {
         let mut model_guard = self.model.lock().await;
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
}

impl HttpBackend {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches("/").to_string(),
            api_key,
        }
    }
}

#[async_trait]
impl LLMBackend for HttpBackend {
    async fn stream_chat_request(
        &self,
        request: RequestBuilder,
    ) -> Result<Box<dyn futures::Stream<Item = Response> + Unpin + Send>> {
        let messages = request.messages();
        
        // Convert messages to OpenAI format
        let mut openai_messages = Vec::new();
        for msg in messages {
             let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
             let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
             openai_messages.push(serde_json::json!({
                 "role": role,
                 "content": content
             }));
        }

        let payload = serde_json::json!({
            "model": "gpt-3.5-turbo", // Placeholder
            "messages": openai_messages,
            "stream": true,
        });

        let response = self.client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("API Error: {}", error_text));
        }
        
        let stream = response.bytes_stream().eventsource();
        let mapped_stream = stream
            .map_err(|e| anyhow::anyhow!("Stream error: {}", e))
            .filter_map(|result| async move {
                match result {
                    Ok(event) => {
                        if event.data == "[DONE]" {
                            None
                        } else {
                            match serde_json::from_str::<ChatCompletionChunkResponse>(&event.data) {
                                Ok(chunk) => Some(Ok(Response::Chunk(chunk))),
                                Err(e) => Some(Err(anyhow::anyhow!("JSON parse error: {}", e))),
                            }
                        }
                    }
                    Err(e) => Some(Err(e)),
                }
            });

        Ok(Box::new(mapped_stream))
    }

    async fn load_model(&self) -> Result<()> {
        Ok(())
    }

    async fn reload_model(&self, _model_filename: String) -> Result<()> {
        Ok(())
    }
    
    async fn get_model(&self) -> Result<Arc<Model>> {
        Err(anyhow::anyhow!("Direct model access not supported in HttpBackend"))
    }
}
