use futures::stream::{self, StreamExt};
use qdrant_client::qdrant::{
    PointStruct, SearchParamsBuilder, SearchPointsBuilder, UpsertPointsBuilder,
};
use qdrant_client::{Payload, Qdrant};
use reqwest;
use serde::Serialize;
use serde_json;
use std::fs;
use std::str;
use std::sync::{Arc, RwLock};
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone, Serialize)]
pub struct IngestStatus {
    pub state: String,
    pub total: usize,
    pub ingested: usize,
    pub progress_percent: f32,
}

impl IngestStatus {
    pub fn new(total: usize) -> Self {
        Self {
            state: "indexing".to_string(),
            total,
            ingested: 0,
            progress_percent: 0.0,
        }
    }
}

pub async fn parse_response_to_vec(
    response: reqwest::Response,
) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let text = response.text().await?;
    if let Ok(vecvec) = serde_json::from_str::<Vec<Vec<f32>>>(&text) {
        if let Some(inner) = vecvec.into_iter().next() {
            return Ok(inner);
        } else {
            return Err("Empty embedding vector".into());
        }
    }
    let json: serde_json::Value = serde_json::from_str(&text)?;
    let floats = json["embeddings"]
        .as_array()
        .ok_or("Expected 'embeddings' field with a JSON array")?
        .get(0)
        .ok_or("No embeddings found")?
        .as_array()
        .ok_or("Expected embedding to be an array")?
        .iter()
        .map(|v| v.as_f64().ok_or("Expected float values").map(|f| f as f32))
        .collect::<Result<Vec<f32>, &str>>()?;
    Ok(floats)
}

pub async fn call(
    message: &str,
) -> Result<reqwest::Response, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let json_payload = serde_json::json!({ "inputs": message });
    let res = client
        .post("http://127.0.0.1:9090/embed")
        .json(&json_payload)
        .send()
        .await?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await?;
        return Err(format!("Request failed: {} - {}", status, body).into());
    }
    Ok(res)
}

fn is_ignored(entry: &DirEntry) -> bool {
    let path_str = entry.path().to_string_lossy();
    path_str.contains("/target/")
        || path_str.contains(".git")
        || path_str.contains("/node_modules/")
        || path_str.contains("/venv/")
        || path_str.contains("/__pycache__/")
        || path_str.contains("annotations")
        || path_str.contains("logs")
}

fn is_likely_text(data: &[u8]) -> bool {
    str::from_utf8(data).is_ok()
}

// Re-export Chunk from chunker crate for backward compatibility
pub use chunker::Chunk;

/// Calculate the difference between two strings and return the byte range of the change
/// This is a simplified implementation that finds the first and last differing bytes
pub fn calculate_change_range(old_content: &str, new_content: &str) -> Option<(u64, u64)> {
    let old_bytes = old_content.as_bytes();
    let new_bytes = new_content.as_bytes();

    // Find the first differing byte
    let mut start_diff = 0;
    while start_diff < old_bytes.len() && start_diff < new_bytes.len() {
        if old_bytes[start_diff] != new_bytes[start_diff] {
            break;
        }
        start_diff += 1;
    }

    // If files are identical, no change
    if start_diff == old_bytes.len() && start_diff == new_bytes.len() {
        return None;
    }

    // Find the last differing byte
    let mut end_diff_old = old_bytes.len();
    let mut end_diff_new = new_bytes.len();

    while end_diff_old > start_diff && end_diff_new > start_diff {
        end_diff_old -= 1;
        end_diff_new -= 1;
        if old_bytes.get(end_diff_old) != new_bytes.get(end_diff_new) {
            end_diff_old += 1; // Include the differing byte
            end_diff_new += 1;
            break;
        }
    }

    // Handle case where one string is prefix of another
    if end_diff_old == start_diff || end_diff_new == start_diff {
        end_diff_old = old_bytes.len();
        end_diff_new = new_bytes.len();
    }

    // Return the range of the change
    let start_byte = start_diff as u64;
    let end_byte = std::cmp::max(end_diff_old, end_diff_new) as u64;

    Some((start_byte, end_byte))
}

/// Find chunks that are affected by a file change based on byte range
/// This function identifies which existing chunks in Qdrant would be affected
/// by a change in the file between start_byte and end_byte
pub async fn find_affected_chunks(
    client: &Qdrant,
    collection_name: &str,
    file_name: &str,
    start_byte: u64,
    end_byte: u64,
) -> Result<Vec<Payload>, Box<dyn std::error::Error + Send + Sync>> {
    // Create a filter to match points for this file
    let filter = qdrant_client::qdrant::Filter::must([
        qdrant_client::qdrant::Condition::matches("file_name", file_name.to_string()),
        // Find chunks that overlap with the changed range
        // A chunk is affected if:
        // 1. It starts before the end of our change AND
        // 2. It ends after the start of our change
        qdrant_client::qdrant::Condition::range(
            "start_byte",
            qdrant_client::qdrant::Range {
                lt: Some((end_byte + 1) as f64), // start_byte < end_byte + 1
                ..Default::default()
            },
        ),
        qdrant_client::qdrant::Condition::range(
            "end_byte",
            qdrant_client::qdrant::Range {
                gt: Some((start_byte - 1) as f64), // end_byte > start_byte - 1
                ..Default::default()
            },
        ),
    ]);

    // Search for points matching the filter
    let search_result = client
        .search_points(
            SearchPointsBuilder::new(collection_name, vec![0.0; 768], 100) // dummy vector
                .filter(filter)
                .with_payload(true)
                .limit(100),
        )
        .await?;

    let results = search_result
        .result
        .into_iter()
        .map(|point| Payload::from(point.payload))
        .collect();

    Ok(results)
}

pub async fn index_codebase(
    root_path: &str,
    client: &Qdrant,
    collection_name: &str,
    status: Arc<RwLock<IngestStatus>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let files: Vec<_> = WalkDir::new(root_path)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && chunker::ChunkerFactory::is_supported(e.path()))
        .collect();

    // Process files sequentially to ensure sandbox restrictions apply
    // NOTE: Using sequential iteration instead of par_iter() because Rayon's thread pool
    // would bypass Landlock sandbox restrictions. The current thread is sandboxed, but
    // Rayon workers are not.
    let all_chunks: Vec<Chunk> = files
        .iter() // Sequential instead of par_iter()
        .filter_map(|entry| {
            let path = entry.path();
            match fs::read(path) {
                Ok(data) => {
                    if is_likely_text(&data) {
                        match chunker::chunk_file(path.to_str().unwrap()) {
                            Ok(chunks) => Some(chunks),
                            Err(_e) => {
                                // eprintln!("Failed to chunk file {}: {}", path.display(), e);
                                None
                            }
                        }
                    } else {
                        None
                    }
                }
                Err(_e) => {
                    // eprintln!("Failed to read file {}: {}", path.display(), e);
                    None
                }
            }
        })
        .flatten()
        .collect();

    {
        let mut s = status.write().unwrap();
        *s = IngestStatus::new(all_chunks.len());
    }

    let points: Vec<PointStruct> = stream::iter(all_chunks.into_iter().enumerate())
        .map(|(i, chunk)| {
            let status = status.clone();
            async move {
                let response = call(&chunk.source_code).await.ok()?;
                let embedding = parse_response_to_vec(response).await.ok()?;
                let payload: Payload = serde_json::json!({
                    "file_name": chunk.file_name,
                    "kind": chunk.kind,
                    "start_byte": chunk.start_byte,
                    "end_byte": chunk.end_byte,
                    "source_code": chunk.source_code,
                })
                .try_into()
                .ok()?;

                {
                    let mut s = status.write().unwrap();
                    s.ingested += 1;
                    s.progress_percent = if s.total > 0 {
                        (s.ingested as f32 / s.total as f32) * 100.0
                    } else {
                        0.0
                    };
                }
                Some(PointStruct::new(i as u64, embedding, payload))
            }
        })
        .buffer_unordered(64) // Increased from 32 to 64 for better parallelism on the network-bound embedding generation
        .filter_map(|p| async move { p })
        .collect()
        .await;

    if !points.is_empty() {
        client
            .upsert_points(UpsertPointsBuilder::new(collection_name, points))
            .await?;
    } else {
        println!("No chunks to index.");
    }

    {
        let mut s = status.write().unwrap();
        s.state = "ready".to_string();
        s.progress_percent = 100.0;
    }

    Ok(())
}

pub async fn search_codebase(
    query: &str,
    client: &Qdrant,
    collection_name: &str,
    limit: u64,
) -> Result<Vec<(f32, Payload)>, Box<dyn std::error::Error + Send + Sync>> {
    let response = call(query).await?;
    let query_embedding = parse_response_to_vec(response).await?;

    let search_result = client
        .search_points(
            SearchPointsBuilder::new(collection_name, query_embedding, limit)
                .with_payload(true)
                .params(SearchParamsBuilder::default()),
        )
        .await?;

    let results = search_result
        .result
        .into_iter()
        .map(|point| (point.score, Payload::from(point.payload)))
        .collect();

    Ok(results)
}

/// Update only the affected chunks for a modified file region
/// This function deletes affected chunks and reindexes only the changed region
pub async fn update_affected_chunks(
    client: &Qdrant,
    collection_name: &str,
    file_name: &str,
    start_byte: u64,
    end_byte: u64,
    _new_content: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!(
        "Updating affected chunks for file: {} in range {}-{}",
        file_name, start_byte, end_byte
    );

    // 1. Find affected chunks in Qdrant
    let affected_chunks =
        find_affected_chunks(client, collection_name, file_name, start_byte, end_byte).await?;

    println!("Found {} affected chunks", affected_chunks.len());

    // 2. Extract point IDs of affected chunks for deletion
    let point_ids: Vec<qdrant_client::qdrant::PointId> = affected_chunks
        .iter()
        .filter_map(|payload| {
            // Try to extract point_id from the payload using serde_json
            match serde_json::to_value(payload) {
                Ok(json_value) => {
                    if let Some(point_id_value) = json_value.get("point_id") {
                        if let Some(point_id_str) = point_id_value.as_str() {
                            // Convert string point ID to PointId
                            Some(qdrant_client::qdrant::PointId::from(
                                point_id_str.to_string(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        })
        .collect();

    // 3. Delete the affected chunks
    if !point_ids.is_empty() {
        println!("Deleting {} affected chunks", point_ids.len());

        // Delete the specific affected points by their IDs
        let delete_points = qdrant_client::qdrant::DeletePointsBuilder::new(collection_name)
            .points(point_ids)
            .wait(true)
            .build();
        let _ = client.delete_points(delete_points).await;
    }

    // 4. Re-parse the entire file and re-chunk it
    // This ensures we get proper Tree-sitter chunks with correct boundaries
    let new_chunks = match chunker::chunk_file(file_name) {
        Ok(chunks) => chunks,
        Err(e) => {
            // eprintln!("Failed to re-chunk file {}: {}", file_name, e);
            return Err(format!("Chunking error: {}", e).into());
        }
    };

    // 5. Generate embeddings and add new chunks to Qdrant
    for chunk in new_chunks.into_iter() {
        if let Ok(response) = call(&chunk.source_code).await {
            if let Ok(embedding) = parse_response_to_vec(response).await {
                // Use UUID for point ID to avoid collisions
                let point_id = uuid::Uuid::new_v4().to_string();

                let payload: qdrant_client::Payload = serde_json::json!({
                    "file_name": chunk.file_name,
                    "kind": chunk.kind,
                    "start_byte": chunk.start_byte,
                    "end_byte": chunk.end_byte,
                    "source_code": chunk.source_code,
                    "point_id": point_id,
                })
                .try_into()
                .unwrap_or_default();

                let point = qdrant_client::qdrant::PointStruct::new(point_id, embedding, payload);

                // Add the new point to Qdrant
                let _ = client
                    .upsert_points(qdrant_client::qdrant::UpsertPointsBuilder::new(
                        collection_name,
                        vec![point],
                    ))
                    .await;
            }
        }
    }

    Ok(())
}
