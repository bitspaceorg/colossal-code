use mistralrs_server_config::{ServerConfig, SchedulerSection, ServerSection, ModelConfig};
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::collections::HashMap;

#[test]
fn randomized_scheduler_validation() {
    let mut rng = StdRng::seed_from_u64(42);

    for _ in 0..100 {
        let max_loaded = rng.gen_range(1..100);
        let max_parallel = rng.gen_range(1..100);
        
        // Calculate expected total
        let total_parallel = max_loaded * max_parallel;
        
        // Generate a server limit around the total_parallel
        let server_limit = if rng.gen_bool(0.5) {
            rng.gen_range(1..total_parallel) // Should fail
        } else {
            rng.gen_range(total_parallel..total_parallel * 2) // Should pass
        };

        let config = ServerConfig {
            server: ServerSection {
                max_total_concurrent_requests: server_limit,
                ..ServerSection::default()
            },
            scheduler: SchedulerSection {
                max_loaded_models: max_loaded,
                max_parallel_requests_per_model: max_parallel,
                ..SchedulerSection::default()
            },
            models: {
                let mut m = HashMap::new();
                let mut model = ModelConfig::default();
                model.model_id = "demo".into();
                model.source = "hf://demo".into();
                model.default = true;
                m.insert("demo".into(), model);
                m
            },
            ..ServerConfig::default()
        };

        let result = config.validate();
        if total_parallel > server_limit {
            assert!(result.is_err(), "Expected error for total_parallel {} > server_limit {}", total_parallel, server_limit);
        } else {
            assert!(result.is_ok(), "Expected ok for total_parallel {} <= server_limit {}, got {:?}", total_parallel, server_limit, result.err());
        }
    }
}

#[test]
fn randomized_gpu_ids_validation() {
    let mut rng = StdRng::seed_from_u64(123);

    for _ in 0..50 {
        let mut ids: Vec<i32> = (0..rng.gen_range(1..10)).map(|_| rng.gen_range(-1..10)).collect();
        
        let has_negative = ids.iter().any(|&x| x < 0);
        let mut sorted = ids.clone();
        sorted.sort();
        let has_duplicate = sorted.windows(2).any(|w| w[0] == w[1]);
        
        let mut config = ServerConfig::default();
        let mut model = ModelConfig::default();
        model.model_id = "gpu-test".into();
        model.source = "hf://demo".into();
        model.default = true;
        model.gpu_ids = Some(ids);
        config.models.insert("gpu-test".into(), model);

        let result = config.validate();
        
        if has_negative || has_duplicate {
             assert!(result.is_err(), "Expected error for invalid gpu_ids");
        } else {
             assert!(result.is_ok(), "Expected ok for valid gpu_ids");
        }
    }
}
