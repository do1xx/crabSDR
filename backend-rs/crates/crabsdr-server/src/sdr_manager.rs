//! SDR Manager — orchestrates multiple SDR pipelines.

use crate::sdr_pipeline::SdrPipeline;
use crabsdr_core::SdrInstanceConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// Manages all SDR pipelines.
pub struct SdrManager {
    pipelines: HashMap<String, Arc<SdrPipeline>>,
}

impl SdrManager {
    pub fn new() -> Self {
        Self {
            pipelines: HashMap::new(),
        }
    }

    /// Start all enabled SDR pipelines from config.
    pub async fn start_all(
        &mut self,
        sdrs: Vec<SdrInstanceConfig>,
        opus_bitrate: u32,
        opus_complexity: u32,
    ) {
        for sdr_config in sdrs {
            if !sdr_config.enabled {
                info!("SDR '{}' is disabled, skipping", sdr_config.id);
                continue;
            }
            let pipeline = SdrPipeline::start(sdr_config, opus_bitrate, opus_complexity).await;
            self.pipelines.insert(pipeline.id.clone(), pipeline);
        }
        info!("Started {} SDR pipelines", self.pipelines.len());
    }

    /// Add a pipeline at runtime.
    pub fn add(&mut self, pipeline: Arc<SdrPipeline>) {
        info!("Added pipeline '{}' at runtime", pipeline.id);
        self.pipelines.insert(pipeline.id.clone(), pipeline);
    }

    /// Remove a pipeline by ID. Returns the removed pipeline (if any).
    pub fn remove(&mut self, id: &str) -> Option<Arc<SdrPipeline>> {
        let removed = self.pipelines.remove(id);
        if removed.is_some() {
            info!("Removed pipeline '{}'", id);
        }
        removed
    }

    /// Get a pipeline by ID.
    pub fn get(&self, id: &str) -> Option<&Arc<SdrPipeline>> {
        self.pipelines.get(id)
    }

    /// List all pipeline IDs.
    pub fn list(&self) -> Vec<&str> {
        self.pipelines.keys().map(|s| s.as_str()).collect()
    }

    /// Get all pipelines.
    pub fn all(&self) -> &HashMap<String, Arc<SdrPipeline>> {
        &self.pipelines
    }

    /// Get band info for all pipelines.
    pub async fn band_info_all(&self) -> Vec<serde_json::Value> {
        let mut bands = Vec::new();
        for pipeline in self.pipelines.values() {
            bands.push(pipeline.band_info().await);
        }
        // Sort by center_freq for consistent ordering
        bands.sort_by(|a, b| {
            let fa = a.get("center_freq").and_then(|v| v.as_u64()).unwrap_or(0);
            let fb = b.get("center_freq").and_then(|v| v.as_u64()).unwrap_or(0);
            fa.cmp(&fb)
        });
        bands
    }
}
