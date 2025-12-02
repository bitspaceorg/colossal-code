use anyhow::Result;
use prometheus::{IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry};

#[derive(Clone)]
pub struct ModelMetrics {
    requests_total: IntCounterVec,
    tokens_total: IntCounterVec,
    stream_tokens_total: IntCounterVec,
    models_loaded_total: IntCounter,
    models_unloaded_total: IntCounter,
    active_requests: IntGaugeVec,
    total_active: IntGauge,
}

impl ModelMetrics {
    pub fn register(registry: &Registry) -> Result<Self> {
        let requests_total = IntCounterVec::new(
            Opts::new("mistral_requests_total", "Total requests handled"),
            &["model", "kind"],
        )?;
        let tokens_total = IntCounterVec::new(
            Opts::new("mistral_tokens_total", "Tokens observed across requests"),
            &["model", "phase"],
        )?;
        let stream_tokens_total = IntCounterVec::new(
            Opts::new(
                "mistral_stream_tokens_total",
                "Tokens emitted through streaming responses",
            ),
            &["model"],
        )?;
        let models_loaded_total = IntCounter::with_opts(Opts::new(
            "mistral_models_loaded_total",
            "Number of models loaded",
        ))?;
        let models_unloaded_total = IntCounter::with_opts(Opts::new(
            "mistral_models_unloaded_total",
            "Number of models unloaded",
        ))?;
        let active_requests = IntGaugeVec::new(
            Opts::new(
                "mistral_active_requests",
                "Current requests executing per model",
            ),
            &["model"],
        )?;
        let total_active = IntGauge::with_opts(Opts::new(
            "mistral_active_requests_total",
            "Total active requests",
        ))?;

        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(tokens_total.clone()))?;
        registry.register(Box::new(stream_tokens_total.clone()))?;
        registry.register(Box::new(models_loaded_total.clone()))?;
        registry.register(Box::new(models_unloaded_total.clone()))?;
        registry.register(Box::new(active_requests.clone()))?;
        registry.register(Box::new(total_active.clone()))?;

        Ok(Self {
            requests_total,
            tokens_total,
            stream_tokens_total,
            models_loaded_total,
            models_unloaded_total,
            active_requests,
            total_active,
        })
    }

    pub fn inc_request(&self, model: &str, kind: &str) {
        if let Ok(counter) = self.requests_total.get_metric_with_label_values(&[model, kind]) {
            counter.inc();
        }
    }

    pub fn add_tokens(&self, model: &str, prompt: u32, completion: u32) {
        if prompt > 0 {
            if let Ok(counter) = self
                .tokens_total
                .get_metric_with_label_values(&[model, "prompt"])
            {
                counter.inc_by(prompt as u64);
            }
        }
        if completion > 0 {
            if let Ok(counter) = self
                .tokens_total
                .get_metric_with_label_values(&[model, "completion"])
            {
                counter.inc_by(completion as u64);
            }
        }
    }

    pub fn add_stream_tokens(&self, model: &str, tokens: u32) {
        if let Ok(counter) = self
            .stream_tokens_total
            .get_metric_with_label_values(&[model])
        {
            counter.inc_by(tokens as u64);
        }
    }

    pub fn track_active(&self, model: &str, active: i64) {
        if let Ok(gauge) = self
            .active_requests
            .get_metric_with_label_values(&[model])
        {
            gauge.set(active);
        }
    }

    pub fn set_total_active(&self, active: i64) {
        self.total_active.set(active);
    }

    pub fn inc_loaded(&self) {
        self.models_loaded_total.inc();
    }

    pub fn inc_unloaded(&self) {
        self.models_unloaded_total.inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_metrics() {
        let registry = Registry::new();
        let metrics = ModelMetrics::register(&registry).expect("metrics");
        metrics.inc_request("model", "chat");
        metrics.add_tokens("model", 2, 3);
        metrics.add_stream_tokens("model", 1);
        metrics.track_active("model", 1);
        metrics.set_total_active(1);
        metrics.inc_loaded();
        metrics.inc_unloaded();
        let request_counter = metrics
            .requests_total
            .get_metric_with_label_values(&["model", "chat"])
            .expect("request counter");
        assert_eq!(request_counter.get(), 1);

        let prompt_tokens = metrics
            .tokens_total
            .get_metric_with_label_values(&["model", "prompt"])
            .expect("prompt tokens");
        let completion_tokens = metrics
            .tokens_total
            .get_metric_with_label_values(&["model", "completion"])
            .expect("completion tokens");
        assert_eq!(prompt_tokens.get(), 2);
        assert_eq!(completion_tokens.get(), 3);

        let stream_tokens = metrics
            .stream_tokens_total
            .get_metric_with_label_values(&["model"])
            .expect("stream tokens");
        assert_eq!(stream_tokens.get(), 1);

        let active_gauge = metrics
            .active_requests
            .get_metric_with_label_values(&["model"])
            .expect("active gauge");
        assert_eq!(active_gauge.get(), 1);
        assert_eq!(metrics.total_active.get(), 1);
        assert_eq!(metrics.models_loaded_total.get(), 1);
        assert_eq!(metrics.models_unloaded_total.get(), 1);
        assert!(!registry.gather().is_empty());
    }
}
