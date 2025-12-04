use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use clap::Parser;
use tokio::{net::TcpListener, runtime::Builder, signal, sync::RwLock};
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use mistralrs_server_api::{build_router, AppState, AuthState, HttpMetrics, ManagerFactory, SchedulerFactory};
use mistralrs_server_config::{load_from_path, ConfigManager, ConfigSource, ServerConfig};
use mistralrs_server_core::{
    DynModelManager, LoadModelRequest, MistralModelManager, RuntimeAdapters,
};
use mistralrs_server_scheduler::LruScheduler;

#[cfg(feature = "mock-manager")]
use mistralrs_server_core::{InMemoryModelManager, ManagerConfig, SystemClock};

/// Launch the HTTP server.
///
/// Example: `cargo run -p mistralrs-server --features mock-manager -- --config config/dev.toml --mock-manager`
#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "config/mistralrs.toml")]
    config: PathBuf,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,
    #[arg(long)]
    threads: Option<usize>,
    #[arg(long, default_value_t = false)]
    mock_manager: bool,
    #[arg(long, default_value_t = false)]
    use_gpu: bool,
}

fn main() -> Result<()> {
    color_eyre::install().ok();
    let args = Args::parse();
    apply_overrides(&args);
    let initial_cfg = load_from_path(&args.config)?;
    let worker_threads = args
        .threads
        .unwrap_or(initial_cfg.server.runtime_threads);
    let runtime = Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()?;
    runtime.block_on(async { run(args).await })
}

async fn run(args: Args) -> Result<()> {
    let config_manager = ConfigManager::load(ConfigSource::Path(args.config.clone())).await?;
    let config = config_manager.get().await;
    init_tracing(&config.logging.level);

    let metrics = HttpMetrics::new()?;
    
    let scheduler_factory: SchedulerFactory = Arc::new(|config| {
        Arc::new(LruScheduler::new(config.scheduler.max_loaded_models))
    });
    
    let scheduler = (scheduler_factory)(&config);
    scheduler.register_metrics(metrics.registry())?;

    let factory = create_factory(args.mock_manager);
    let manager = (factory)(&config, scheduler.clone(), &metrics).await?;
    preload_models(&manager, &config).await;

    let auth_state = AuthState::from_section(&config.auth);
    let state = AppState {
        manager: Arc::new(RwLock::new(manager)),
        factory,
        scheduler_factory,
        config: config_manager.clone(),
        scheduler: Arc::new(RwLock::new(scheduler)),
        model_metrics: metrics.model_metrics(),
        metrics: metrics.clone(),
        auth: auth_state,
    };
    let router = build_router(state).await?;
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "server listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn apply_overrides(args: &Args) {
    if let Some(host) = &args.host {
        std::env::set_var("MISTRALRS__SERVER__HOST", host);
    }
    if let Some(port) = args.port {
        std::env::set_var("MISTRALRS__SERVER__PORT", port.to_string());
    }
    if let Some(level) = &args.log_level {
        std::env::set_var("MISTRALRS__LOGGING__LEVEL", level);
    }
    if let Some(threads) = args.threads {
        std::env::set_var("MISTRALRS__SERVER__RUNTIME_THREADS", threads.to_string());
    }
    if args.use_gpu {
        // Enable CUDA paged attention if GPU is requested
        std::env::set_var("MISTRALRS__SCHEDULER__PAGED_ATTN_CUDA", "true");
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json())
        .try_init();
}

fn create_factory(force_mock: bool) -> ManagerFactory {
    Arc::new(move |config, scheduler, metrics| {
        let config = config.clone();
        let scheduler = scheduler.clone();
        let metrics = metrics.clone();
        Box::pin(async move {
            #[cfg(feature = "mock-manager")]
            if force_mock {
                let manager = Arc::new(InMemoryModelManager::new(
                    ManagerConfig::from(&config),
                    scheduler,
                    SystemClock,
                ));
                return Ok(manager as DynModelManager);
            }
            #[cfg(not(feature = "mock-manager"))]
            if force_mock {
                return Err(anyhow!(
                    "mock manager feature disabled; rebuild with --features mock-manager"
                ));
            }
            let manager = MistralModelManager::new(
                &config,
                scheduler,
                metrics.registry(),
                metrics.model_metrics(),
                RuntimeAdapters::current(),
            )
            .await?;
            Ok(Arc::new(manager) as DynModelManager)
        })
    })
}

async fn preload_models(manager: &DynModelManager, config: &ServerConfig) {
    for model in config.models.values() {
        if model.pinned || model.default {
            let _ = manager
                .load_model(LoadModelRequest {
                    model: model.model_id.clone(),
                    keep_alive: model.keep_alive,
                    pinned: model.pinned,
                })
                .await;
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut stream = signal(SignalKind::terminate()).expect("failed to install signal handler");
        stream.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}