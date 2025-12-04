use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, atomic::{AtomicUsize, Ordering}},
    time::{Duration, Instant},
};

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashSet;
use parking_lot::RwLock;
use prometheus::{IntCounter, IntGauge, Opts, Registry};
use tracing::info;

use mistralrs_server_core::{ModelMetadata, ModelScheduler, ModelManagerError};

#[derive(Clone)]
struct SchedulerMetrics {
    models_loaded_total: IntCounter,
    models_evicted_total: IntCounter,
    active_models: IntGauge,
    vram_usage_bytes: IntGauge,
}

impl SchedulerMetrics {
    fn new() -> Self {
        Self {
            models_loaded_total: IntCounter::with_opts(Opts::new(
                "scheduler_models_loaded_total",
                "Total models observed as loaded",
            ))
            .expect("counter"),
            models_evicted_total: IntCounter::with_opts(Opts::new(
                "scheduler_models_evicted_total",
                "Number of models advised for eviction",
            ))
            .expect("counter"),
            active_models: IntGauge::with_opts(Opts::new(
                "scheduler_active_models",
                "Gauge representing active loaded models",
            ))
            .expect("gauge"),
            vram_usage_bytes: IntGauge::with_opts(Opts::new(
                "scheduler_vram_usage_bytes",
                "Total estimated VRAM usage in bytes",
            ))
            .expect("gauge"),
        }
    }

    fn snapshot(&self) -> SchedulerMetricsSnapshot {
        SchedulerMetricsSnapshot {
            loaded: self.models_loaded_total.get(),
            evicted: self.models_evicted_total.get(),
            active: self.active_models.get(),
        }
    }

    fn register(&self, registry: &Registry) -> Result<()> {
        registry.register(Box::new(self.models_loaded_total.clone()))?;
        registry.register(Box::new(self.models_evicted_total.clone()))?;
        registry.register(Box::new(self.active_models.clone()))?;
        registry.register(Box::new(self.vram_usage_bytes.clone()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerMetricsSnapshot {
    pub loaded: u64,
    pub evicted: u64,
    pub active: i64,
}

#[derive(Clone)]
pub struct LruScheduler {
    inner: Arc<RwLock<InnerState>>,
    pinned: Arc<DashSet<String>>,
    keep_alive_fn: Arc<RwLock<Arc<KeepAliveFn>>>,
    metrics: SchedulerMetrics,
    max_vram_bytes: Arc<AtomicUsize>,
}

type KeepAliveFn = dyn Fn(&str) -> Option<Instant> + Send + Sync;

struct InnerState {
    max_models: usize,
    order: VecDeque<String>,
    last_access: HashMap<String, Instant>,
    model_sizes: HashMap<String, u64>,
    total_size: u64,
}

impl LruScheduler {
    pub fn new(max_models: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(InnerState {
                max_models,
                order: VecDeque::new(),
                last_access: HashMap::new(),
                model_sizes: HashMap::new(),
                total_size: 0,
            })),
            pinned: Arc::new(DashSet::new()),
            keep_alive_fn: Arc::new(RwLock::new(Arc::new(|_| None))),
            metrics: SchedulerMetrics::new(),
            max_vram_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn with_keep_alive_fn<F>(mut self, func: F) -> Self
    where
        F: Fn(&str) -> Option<Instant> + Send + Sync + 'static,
    {
        *self.keep_alive_fn.write() = Arc::new(func);
        self
    }

    pub fn metrics_snapshot(&self) -> SchedulerMetricsSnapshot {
        self.metrics.snapshot()
    }

    pub fn register_metrics(&self, registry: &Registry) -> Result<()> {
        self.metrics.register(registry)
    }

    fn touch(&self, model: &str) {
        let mut inner = self.inner.write();
        inner.order.retain(|name| name != model);
        inner.order.push_back(model.to_string());
        inner
            .last_access
            .insert(model.to_string(), Instant::now());
    }
}

#[async_trait]
impl ModelScheduler for LruScheduler {
    async fn on_model_loaded(&self, metadata: &ModelMetadata) {
        {
            let mut inner = self.inner.write();
            inner.order.retain(|name| name != &metadata.name);
            inner.order.push_back(metadata.name.clone());
            inner
                .last_access
                .insert(metadata.name.clone(), Instant::now());
            
            // Update size tracking
            if let Some(old_size) = inner.model_sizes.insert(metadata.name.clone(), metadata.size_bytes) {
                inner.total_size = inner.total_size.saturating_sub(old_size);
            }
            inner.total_size = inner.total_size.saturating_add(metadata.size_bytes);
            self.metrics.vram_usage_bytes.set(inner.total_size as i64);
        }
        if metadata.pinned {
            self.pinned.insert(metadata.name.clone());
        } else {
            self.pinned.remove(&metadata.name);
        }
        self.metrics.models_loaded_total.inc();
        self.metrics
            .active_models
            .set(self.inner.read().order.len() as i64);
        info!(model = %metadata.name, size_bytes = %metadata.size_bytes, "scheduler model loaded");
    }

    async fn on_model_unloaded(&self, model: &str) {
        let mut inner = self.inner.write();
        inner.order.retain(|name| name != model);
        inner.last_access.remove(model);
        
        if let Some(size) = inner.model_sizes.remove(model) {
            inner.total_size = inner.total_size.saturating_sub(size);
            self.metrics.vram_usage_bytes.set(inner.total_size as i64);
        }

        self.pinned.remove(model);
        self.metrics
            .active_models
            .set(inner.order.len() as i64);
    }

    async fn advise_evict(&self) -> Vec<String> {
        let now = Instant::now();
        let keep_alive_lookup = self.keep_alive_fn.read().clone();
        let mut inner = self.inner.write();
        if inner.order.len() <= inner.max_models {
            return vec![];
        }
        let mut candidates: Vec<_> = inner
            .order
            .iter()
            .map(|name| {
                let keep_alive_deadline =
                    keep_alive_lookup(name).unwrap_or(now - Duration::from_secs(0));
                let last_access = inner.last_access.get(name).copied().unwrap_or(now);
                (name.clone(), keep_alive_deadline, last_access)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let pinned_a = self.pinned.contains(&a.0);
            let pinned_b = self.pinned.contains(&b.0);
            pinned_a
                .cmp(&pinned_b)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        let mut evictions = Vec::new();
        for (name, keep_alive_deadline, _) in candidates {
            if inner.order.len() <= inner.max_models {
                break;
            }
            if self.pinned.contains(&name) {
                continue;
            }
            if keep_alive_deadline > now {
                continue;
            }
            inner.order.retain(|entry| entry != &name);
            inner.last_access.remove(&name);
            self.metrics.models_evicted_total.inc();
            evictions.push(name);
        }
        evictions
    }

    async fn register_activity(&self, model: &str) {
        self.touch(model);
    }

    fn set_keep_alive_lookup(
        &self,
        lookup: Arc<dyn Fn(&str) -> Option<Instant> + Send + Sync>,
    ) {
        *self.keep_alive_fn.write() = lookup;
    }

    fn set_max_vram_bytes(&self, bytes: usize) {
        self.max_vram_bytes.store(bytes, Ordering::SeqCst);
    }

    fn can_load_model(&self, model_name: &str, estimated_size_bytes: u64) -> Result<(), ModelManagerError> {
        let max_vram = self.max_vram_bytes.load(Ordering::SeqCst);
        if max_vram == 0 {
            // No VRAM limit configured, allow.
            return Ok(());
        }
        let current_vram_usage = self.inner.read().total_size;
        let potential_new_usage = current_vram_usage.saturating_add(estimated_size_bytes);

        if potential_new_usage > max_vram as u64 {
            return Err(ModelManagerError::Scheduler(format!(
                "insufficient VRAM to load model {model_name}. Current usage: {}MB, Model size: {}MB, Max: {}MB",
                current_vram_usage / 1024 / 1024,
                estimated_size_bytes / 1024 / 1024,
                max_vram / 1024 / 1024
            )));
        }
        Ok(())
    }

    fn register_metrics(&self, registry: &Registry) -> Result<()> {
        self.metrics.register(registry)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use mistralrs_server_core::ModelMetadata;

    fn metadata(name: &str, pinned: bool) -> ModelMetadata {
        ModelMetadata {
            name: name.into(),
            size_bytes: 0,
            context_length: 0,
            quantization: None,
            loaded: true,
            keep_alive: Duration::from_secs(1),
            pinned,
            parameters: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn evicts_oldest_non_pinned() {
        let scheduler = LruScheduler::new(1);
        scheduler.on_model_loaded(&metadata("a", true)).await;
        scheduler.on_model_loaded(&metadata("b", false)).await;
        let evicted = scheduler.advise_evict().await;
        assert_eq!(evicted, vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn keep_alive_prevents_eviction_until_ready() {
        let scheduler = LruScheduler::new(1).with_keep_alive_fn(|name| {
            if name == "a" {
                Some(Instant::now() + Duration::from_secs(60))
            } else {
                None
            }
        });
        scheduler.on_model_loaded(&metadata("a", false)).await;
        scheduler.on_model_loaded(&metadata("b", false)).await;
        let evicted = scheduler.advise_evict().await;
        assert_eq!(evicted, vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn metrics_updated_on_load_and_unload() {
        let registry = Registry::new();
        let scheduler = LruScheduler::new(2);
        scheduler.register_metrics(&registry).unwrap();
        scheduler.on_model_loaded(&metadata("a", false)).await;
        scheduler.on_model_loaded(&metadata("b", false)).await;
        scheduler.on_model_unloaded("b").await;
        let snapshot = scheduler.metrics_snapshot();
        assert_eq!(snapshot.loaded, 2);
        assert_eq!(snapshot.active, 1);
    }
}
