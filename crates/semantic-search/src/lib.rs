use actix_web::{HttpResponse, Responder, post, web};
use chunker::{self, Chunk, ChunkerFactory};
use futures::stream::{self, StreamExt};
use glob::glob;
use indicatif::{ProgressBar, ProgressStyle};
use log;
use qdrant_client::qdrant::{
    PointStruct, SearchParamsBuilder, SearchPointsBuilder, UpsertPointsBuilder,
};
use qdrant_client::{Payload, Qdrant};
use rayon::prelude::*;
use reqwest;
use serde::{Deserialize, Serialize};
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

async fn parse_response_to_vec(
    response: reqwest::Response,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
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

async fn call(message: &str) -> Result<reqwest::Response, Box<dyn std::error::Error>> {
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

#[derive(Serialize, Deserialize)]
struct ReqBody {
    query: String,
    collection_name: String,
    file_globs: Option<Vec<String>>,
    kind: Option<String>,
    limit: u64,
}

#[derive(Serialize, Deserialize)]
struct ErrorResponse {
    error: String,
}

fn expand_globs(patterns: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    for pattern in patterns {
        match glob(pattern) {
            Ok(paths) => {
                for entry in paths.filter_map(Result::ok) {
                    if entry.is_file() {
                        files.push(entry.to_string_lossy().to_string());
                    }
                }
            }
            Err(e) => {
                log::warn!("Invalid glob pattern {}: {}", pattern, e);
            }
        }
    }
    files
}

pub async fn index_codebase(
    root_path: &str,
    client: &Qdrant,
    collection_name: &str,
    status: Arc<RwLock<IngestStatus>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let files: Vec<_> = WalkDir::new(root_path)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && ChunkerFactory::is_supported(e.path()))
        .collect();

    let pb_files = ProgressBar::new(files.len() as u64);
    pb_files.set_style(
        ProgressStyle::with_template(
            "[Chunking] {bar:40.cyan/blue} {pos}/{len} {elapsed_precise} ETA {eta_precise}",
        )?
        .progress_chars("=>-"),
    );

    let all_chunks: Vec<Chunk> = files
        .par_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let path_str = path.to_str()?;

            match chunker::chunk_file(path_str) {
                Ok(chunks) => {
                    pb_files.inc(1);
                    Some(chunks)
                }
                Err(e) => {
                    log::error!("Failed to chunk file {}: {}", path.display(), e);
                    None
                }
            }
        })
        .flatten()
        .collect();

    pb_files.finish_with_message("Chunking done ✅");
    println!("No of Chunks: {}", all_chunks.len());

    {
        let mut s = status.write().unwrap();
        *s = IngestStatus::new(all_chunks.len());
    }

    let pb_index = ProgressBar::new(all_chunks.len() as u64);
    pb_index.set_style(
        ProgressStyle::with_template(
            "[Indexing] {bar:40.green/white} {pos}/{len} {elapsed_precise} ETA {eta_precise}",
        )?
        .progress_chars("=>-"),
    );

    let points: Vec<PointStruct> = stream::iter(all_chunks.into_iter().enumerate())
        .map(|(i, chunk)| {
            let pb_index = pb_index.clone();
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
                pb_index.inc(1);
                Some(PointStruct::new(i as u64, embedding, payload))
            }
        })
        .buffer_unordered(32)
        .filter_map(|p| async move { p })
        .collect()
        .await;

    pb_index.finish_with_message("Indexing done 🚀");

    if !points.is_empty() {
        client
            .upsert_points(UpsertPointsBuilder::new(collection_name, points))
            .await?;
        log::info!(
            "Indexed {} chunks into Qdrant.",
            pb_index.length().unwrap_or(0)
        );
    } else {
        log::info!("No chunks to index.");
    }

    {
        let mut s = status.write().unwrap();
        s.state = "ready".to_string();
        s.progress_percent = 100.0;
    }

    Ok(())
}

async fn search_codebase(
    query: &str,
    client: &Qdrant,
    collection_name: &str,
    limit: u64,
) -> Result<Vec<(f32, Payload)>, Box<dyn std::error::Error>> {
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

#[post("/v1/semantic-search")]
async fn semantic_search(
    req: web::Json<ReqBody>,
    client: web::Data<Qdrant>,
    status: web::Data<Arc<RwLock<IngestStatus>>>,
) -> impl Responder {
    let s = status.read().unwrap();
    if s.state != "ready" {
        return HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "error": "Indexing not finished yet",
            "state": s.state,
            "ingested": s.ingested,
            "total": s.total,
            "progress_percent": s.progress_percent,
        }));
    }

    let req = req.into_inner();
    let query = &req.query;
    let collection_name = &req.collection_name;
    let limit = req.limit;

    let mut restricted_files = Vec::new();
    if let Some(globs) = &req.file_globs {
        restricted_files = expand_globs(globs);
        log::info!("Restricted search to files: {:?}", restricted_files);
    }

    match search_codebase(query, &client, collection_name, limit).await {
        Ok(results) => {
            let filtered: Vec<_> = if restricted_files.is_empty() {
                results
            } else {
                results
                    .into_iter()
                    .filter(|(_, payload)| {
                        if let Ok(val) = serde_json::to_value(payload) {
                            if let Some(file_name) = val.get("file_name").and_then(|f| f.as_str()) {
                                return restricted_files.iter().any(|f| f == file_name);
                            }
                        }
                        false
                    })
                    .collect()
            };

            HttpResponse::Ok().json(filtered)
        }
        Err(err) => {
            let error_response = ErrorResponse {
                error: format!("Exception: {}", err),
            };
            HttpResponse::InternalServerError().json(error_response)
        }
    }
}
