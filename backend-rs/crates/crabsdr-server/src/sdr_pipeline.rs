//! SDR Pipeline — encapsulates one SDR device's full processing chain.
//!
//! Each pipeline owns:
//!   - SDR subprocess (rtl_sdr/hackrf/rx_sdr)
//!   - DSP thread (FFT + demod)
//!   - ClientManager (per-pipeline connected clients)
//!   - Spectrum broadcast channel
//!   - Health status
//!
//! Pipelines are isolated: a crash in one does not affect others.

use crate::client::ClientManager;
use crate::dsp_thread::DspThread;
use crabsdr_core::{protocol, SdrInstanceConfig};
use crabsdr_sdr::{self, DeviceCapabilities, DriverCommand, SdrConfig};
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tracing::info;

/// Health status of a pipeline.
#[derive(Debug, Clone, Serialize)]
#[derive(Default)]
pub struct PipelineStatus {
    pub running: bool,
    pub error: Option<String>,
    pub client_count: usize,
    pub frames_processed: u64,
}


/// A complete SDR processing pipeline.
/// Software-Frequenzkorrektur: Mitte des Bandes, wie Anzeige und Abstimmung sie sehen
pub fn corrected_center(hw_center: u64, ppm: f64) -> u64 { (hw_center as f64 * (1.0 + ppm / 1e6)).round() as u64 }

pub struct SdrPipeline {
    pub id: String,
    pub label: String,
    /// Runtime-mutable config (updated via admin API).
    pub config: RwLock<SdrInstanceConfig>,
    /// FFT size (immutable after creation)
    pub fft_size: usize,
    /// Software-Frequenzkorrektur in ppm (fest nach dem Start)
    pub corr_ppm: f64,
    pub spectrum_tx: broadcast::Sender<Vec<u8>>,
    pub sdr_cmd_tx: mpsc::Sender<DriverCommand>,
    pub clients: Arc<Mutex<ClientManager>>,
    pub center_freq: Arc<AtomicU64>,
    pub status: Arc<RwLock<PipelineStatus>>,
    /// Device capabilities (gain elements, AGC, etc.)
    pub capabilities: DeviceCapabilities,
    /// Live-mutable flags (updated via admin API without restart)
    pub admin_only: AtomicBool,
    pub guest: AtomicBool,
    pub sample_rate: Arc<AtomicU32>,
}

impl SdrPipeline {
    /// Create and start a new pipeline from an SDR instance config.
    pub async fn start(config: SdrInstanceConfig, opus_bitrate: u32, opus_complexity: u32) -> Arc<Self> {
        let id = config.id.clone();
        let label = config.label.clone();
        info!("Starting pipeline '{}' ({}) — {} @ {} Hz", id, label, config.sdr_driver, config.center_freq);

        // Channels
        let (spectrum_tx, _) = broadcast::channel::<Vec<u8>>(4);
        // 32 Blöcke à 2 ms: kurze Hänger des DSP-Threads (Bildaufbau, Zoom-Spektrum) dürfen bis ~64 ms dauern, ohne dass der
        // Treiber blockiert und Abtastwerte verliert. Im Normalbetrieb ist der Kanal leer, die Laufzeit steigt dadurch nicht.
        let (iq_tx, iq_rx) = mpsc::channel(32);

        // Probe device capabilities before starting driver
        let capabilities = crabsdr_sdr::probe::probe_device(&config.sdr_driver, &config.sdr_device);
        info!("[{}] Device capabilities: {} ({} gain elements, AGC={})",
            id, capabilities.device_name, capabilities.gain_elements.len(), capabilities.has_agc);

        // Start SDR driver
        let sdr_config = SdrConfig::from(&config);
        let (sdr_cmd_tx, health_rx, _sdr_handle) = crabsdr_sdr::start_sdr(sdr_config, iq_tx).await;

        // Shared center frequency and sample rate (can be updated at runtime)
        let center_freq = Arc::new(AtomicU64::new(config.center_freq));
        let sample_rate = Arc::new(AtomicU32::new(config.sample_rate));

        // DSP thread
        let dsp = DspThread::new(
            config.fft_size,
            sample_rate.clone(),
            center_freq.clone(),
            config.freq_correction_ppm,
            config.fft_fps,
            opus_bitrate,
            opus_complexity,
        );
        let dsp_spectrum_tx = spectrum_tx.clone();
        let clients = Arc::new(Mutex::new(ClientManager::new()));
        let clients_for_dsp = clients.clone();
        let dsp_id = id.clone();
        let band_for_dsp = id.clone();

        std::thread::Builder::new()
            .name(format!("dsp-{}", dsp_id))
            .spawn(move || {
                dsp.run(iq_rx, dsp_spectrum_tx, clients_for_dsp, band_for_dsp);
            })
            .expect("Failed to spawn DSP thread");

        let status = Arc::new(RwLock::new(PipelineStatus::default()));

        let fft_size = config.fft_size;
        let corr_ppm = config.freq_correction_ppm;
        let pipeline = Arc::new(Self {
            admin_only: AtomicBool::new(config.admin_only),
            guest: AtomicBool::new(config.guest),
            id,
            label,
            fft_size,
            corr_ppm,
            config: RwLock::new(config),
            spectrum_tx,
            sdr_cmd_tx,
            clients,
            center_freq,
            sample_rate,
            status,
            capabilities,
        });

        // Spawn health watcher
        {
            let status = pipeline.status.clone();
            let pipeline_id = pipeline.id.clone();
            let mut health_rx = health_rx;
            tokio::spawn(async move {
                while health_rx.changed().await.is_ok() {
                    let healthy = *health_rx.borrow();
                    let mut s = status.write().await;
                    if s.running != healthy {
                        s.running = healthy;
                        if healthy {
                            s.error = None;
                            info!("[{}] SDR is online", pipeline_id);
                        } else {
                            s.error = Some("SDR offline".to_string());
                            info!("[{}] SDR went offline", pipeline_id);
                        }
                    }
                }
            });
        }

        pipeline
    }

    /// Get band info for API responses.
    pub async fn band_info(&self) -> serde_json::Value {
        let clients = self.clients.lock().await;
        let status = self.status.read().await;
        let config = self.config.read().await;
        let current_center = corrected_center(self.center_freq.load(std::sync::atomic::Ordering::Relaxed), self.corr_ppm);
        json!({
            "id": self.id,
            "label": self.label,
            "center_freq": current_center,
            "sample_rate": config.sample_rate,
            "gain": config.gain,
            "gain_elements": config.gain_elements,
            "ppm": config.ppm,
            "bias_tee": config.bias_tee,
            "device": config.sdr_device,
            "driver": config.sdr_driver,
            "running": status.running,
            "clients": clients.count_real(),
            "enabled": config.enabled,
            "admin_only": self.admin_only.load(std::sync::atomic::Ordering::Relaxed),
            "guest": self.guest.load(std::sync::atomic::Ordering::Relaxed),
            "capabilities": self.capabilities,
            "default_mode": config.default_mode,
        })
    }

    /// Broadcast a JSON message to all connected clients via the spectrum channel.
    pub fn broadcast_json(&self, msg: &serde_json::Value) {
        let frame = protocol::encode_json(&msg.to_string());
        let _ = self.spectrum_tx.send(frame);
    }
}
