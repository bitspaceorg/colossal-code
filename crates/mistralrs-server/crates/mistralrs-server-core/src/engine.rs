use std::sync::Arc;

use async_trait::async_trait;
use mistralrs_core::{MistralRs, Request};
use tokio::sync::mpsc::error::SendError;

use crate::ModelManagerError;

#[derive(Clone)]
pub struct SharedMistralRsState {
    inner: Arc<MistralRs>,
}

impl SharedMistralRsState {
    pub fn new(inner: Arc<MistralRs>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &Arc<MistralRs> {
        &self.inner
    }
}

impl From<upstream_mistralrs_server_core::types::SharedMistralRsState> for SharedMistralRsState {
    fn from(value: upstream_mistralrs_server_core::types::SharedMistralRsState) -> Self {
        Self { inner: value }
    }
}

impl From<SharedMistralRsState> for upstream_mistralrs_server_core::types::SharedMistralRsState {
    fn from(value: SharedMistralRsState) -> Self {
        value.inner
    }
}

#[async_trait]
pub trait EngineHandle: Send + Sync {
    async fn ensure_model_loaded(&self, model: &str) -> Result<(), ModelManagerError>;
    async fn send_request_with_model(
        &self,
        request: Request,
        model: Option<&str>,
    ) -> Result<(), ModelManagerError>;
    async fn remove_model(&self, model: &str) -> Result<(), ModelManagerError>;
    async fn list_models(&self) -> Result<Vec<String>, ModelManagerError>;
}

#[async_trait]
impl EngineHandle for SharedMistralRsState {
    async fn ensure_model_loaded(&self, model: &str) -> Result<(), ModelManagerError> {
        let models = self.list_models().await?;
        if models.iter().any(|entry| entry == model) {
            Ok(())
        } else {
            Err(ModelManagerError::NotFound(model.to_string()))
        }
    }

    async fn send_request_with_model(
        &self,
        request: Request,
        model: Option<&str>,
    ) -> Result<(), ModelManagerError> {
        let sender = self
            .inner
            .get_sender(model)
            .map_err(|err| ModelManagerError::Other(err.to_string()))?;
        sender
            .send(request)
            .await
            .map_err(|err: SendError<Request>| ModelManagerError::Other(err.to_string()))
    }

    async fn remove_model(&self, model: &str) -> Result<(), ModelManagerError> {
        self.inner
            .remove_model(model)
            .map_err(|err| ModelManagerError::Other(err))
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelManagerError> {
        self.inner
            .list_models()
            .map_err(|err| ModelManagerError::Other(err))
    }
}
