pub mod subprocess;
pub mod rtl_tcp;
pub mod probe;

use crabsdr_core::IqBuffer;
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};

/// Convert unsigned 8-bit IQ pairs to Complex32.
/// Both rtl_sdr and rtl_tcp output u8 IQ pairs: (I, Q) interleaved.
pub fn u8_to_complex(raw: &[u8]) -> IqBuffer {
    let n_samples = raw.len() / 2;
    let mut iq = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let re = (raw[i * 2] as f32 - 127.5) / 127.5;
        let im = (raw[i * 2 + 1] as f32 - 127.5) / 127.5;
        iq.push(Complex32::new(re, im));
    }
    iq
}

/// A single gain element with its valid range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GainElement {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

/// Device capabilities probed from SoapySDR.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceCapabilities {
    pub device_name: String,
    pub gain_elements: Vec<GainElement>,
    pub has_agc: bool,
}

/// SDR driver configuration.
#[derive(Debug, Clone)]
pub struct SdrConfig {
    pub driver: String,
    pub device: String,
    pub tcp_host: String,
    pub tcp_port: u16,
    pub center_freq: u64,
    pub sample_rate: u32,
    pub gain: f64,
    pub ppm: i32,
    pub bias_tee: bool,
    /// Per-element gain values (e.g. {"IFGR": 40, "RFGR": 2})
    pub gain_elements: HashMap<String, f64>,
}

impl From<&crabsdr_core::Config> for SdrConfig {
    fn from(c: &crabsdr_core::Config) -> Self {
        Self {
            driver: c.sdr_driver.clone(),
            device: c.sdr_device.clone(),
            tcp_host: c.sdr_tcp_host.clone(),
            tcp_port: c.sdr_tcp_port,
            center_freq: c.center_freq,
            sample_rate: c.sample_rate,
            gain: c.gain,
            ppm: c.ppm,
            bias_tee: false,
            gain_elements: HashMap::new(),
        }
    }
}

impl From<&crabsdr_core::SdrInstanceConfig> for SdrConfig {
    fn from(c: &crabsdr_core::SdrInstanceConfig) -> Self {
        Self {
            driver: c.sdr_driver.clone(),
            device: c.sdr_device.clone(),
            tcp_host: c.sdr_tcp_host.clone(),
            tcp_port: c.sdr_tcp_port,
            center_freq: c.center_freq,
            sample_rate: c.sample_rate,
            gain: c.gain,
            ppm: c.ppm,
            bias_tee: c.bias_tee,
            gain_elements: c.gain_elements.clone().unwrap_or_default(),
        }
    }
}

/// Commands that can be sent to a running SDR driver.
#[derive(Debug)]
pub enum DriverCommand {
    SetFrequency(u64),
    SetGain(f64),
    /// Set per-element gains (e.g. {"IFGR": 40, "RFGR": 2})
    SetGainElements(HashMap<String, f64>),
    SetBiasTee(bool),
    SetPpm(i32),
    SetSampleRate(u32),
    Stop,
}

/// Start the appropriate SDR driver based on config, returning a channel of IQ buffers,
/// a command sender, and a health watch receiver (true=streaming, false=down).
pub async fn start_sdr(
    config: SdrConfig,
    iq_tx: mpsc::Sender<IqBuffer>,
) -> (mpsc::Sender<DriverCommand>, watch::Receiver<bool>, SdrHandle) {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (health_tx, health_rx) = watch::channel(false);

    let handle = if config.driver == "rtl_tcp" {
        let driver = rtl_tcp::RtlTcpDriver::new(config);
        let join = tokio::spawn(async move {
            driver.run(iq_tx, cmd_rx, health_tx).await;
        });
        SdrHandle { join }
    } else {
        let driver = subprocess::SubprocessDriver::new(config);
        let join = tokio::spawn(async move {
            driver.run(iq_tx, cmd_rx, health_tx).await;
        });
        SdrHandle { join }
    };

    (cmd_tx, health_rx, handle)
}

pub struct SdrHandle {
    pub join: tokio::task::JoinHandle<()>,
}
