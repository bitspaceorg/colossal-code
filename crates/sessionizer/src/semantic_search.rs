use anyhow::Result;
use qdrant_client::Qdrant;
use std::sync::{Arc, RwLock};
use tokio;

// Import the semantic search functionality from the semantic-search crate
use crate::semantic_search_lib::{IngestStatus, index_codebase};

pub struct SemanticSearchService {
    status: Arc<RwLock<IngestStatus>>,
}

impl SemanticSearchService {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(IngestStatus {
                state: "initializing".into(),
                total: 0,
                ingested: 0,
                progress_percent: 0.0,
            })),
        }
    }

    pub async fn start_indexing(&self, root_path: &str) -> Result<()> {
        let collection_name = "codebase";
        let client = Qdrant::from_url("http://localhost:6334").build()?;

        // Update status to indexing
        {
            let mut s = self.status.write().unwrap();
            s.state = "indexing".to_string();
        }

        // Start indexing in a separate task
        let status_clone = self.status.clone();
        let client_clone = client.clone();
        let root_path = root_path.to_string();

        tokio::spawn(async move {
            if let Err(e) = index_codebase(
                &root_path,
                &client_clone,
                collection_name,
                status_clone.clone(),
            )
            .await
            {
                let mut s = status_clone.write().unwrap();
                s.state = format!("error: {}", e);
                s.progress_percent = 0.0;
            }
        });

        Ok(())
    }

    pub fn get_status(&self) -> IngestStatus {
        self.status.read().unwrap().clone()
    }
}
