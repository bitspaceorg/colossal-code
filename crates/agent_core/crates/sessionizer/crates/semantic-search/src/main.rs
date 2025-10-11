use std::sync::{Arc, RwLock};
use qdrant_client::qdrant::{
    CreateCollectionBuilder, Distance, ScalarQuantizationBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use rayon::prelude::*;
use actix_web::{web, App, HttpServer};
use actix_web::middleware::Logger;

use semantic_search_lib::{index_codebase, semantic_search, IngestStatus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let root_path = "/home/wise/arsenal/lowbit";
    let collection_name = "codebase";
    let client = Qdrant::from_url("http://localhost:6334").build()?;
    let client_clone = client.clone();

    let collections_list = client.list_collections().await?;
    if collections_list
        .collections
        .par_iter()
        .any(|c| c.name == collection_name)
    {
        client.delete_collection(collection_name).await?;
    }

    client
        .create_collection(
            CreateCollectionBuilder::new(collection_name)
                .vectors_config(VectorParamsBuilder::new(768, Distance::Cosine))
                .quantization_config(ScalarQuantizationBuilder::default()),
        )
        .await?;

    let status = Arc::new(RwLock::new(IngestStatus {
        state: "indexing".into(),
        total: 0,
        ingested: 0,
        progress_percent: 0.0,
    }));

    let status_clone = status.clone();
    let client_clone_for_spawn = client_clone.clone();
    tokio::spawn(async move {
        if let Err(e) = index_codebase(root_path, &client_clone_for_spawn, collection_name, status_clone.clone()).await {
            let mut s = status_clone.write().unwrap();
            s.state = format!("error: {}", e);
            s.progress_percent = 0.0;
        }
    });

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(client_clone.clone()))
            .app_data(web::Data::new(status.clone()))
            .service(semantic_search)
            .wrap(Logger::default())
    })
    .bind(("127.0.0.1", 1551))?
    .run()
    .await?;

    Ok(())
}
