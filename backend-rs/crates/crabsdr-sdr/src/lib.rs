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

/// Rohformat der IQ-Daten, die ein Unterprozess liefert. RTL-Sticks: 8 Bit (cu8). SoapySDR-Geräte über rx_sdr:
/// Voreinstellung 16 Bit (cs16) – Airspy, SDRplay/MSi2500, LimeSDR, PlutoSDR haben 12–14 Bit, in 8 Bit ginge der
/// größte Teil der Dynamik verloren; cf32 für Module, die nur Gleitkomma liefern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IqFormat { Cu8, Cs16, Cf32 }

impl IqFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cu8" | "u8" => Some(Self::Cu8),
            "cs16" | "s16" => Some(Self::Cs16),
            "cf32" | "f32" => Some(Self::Cf32),
            _ => None,
        }
    }
    /// Bytes je komplexem Sample
    pub fn bytes_per_sample(self) -> usize { match self { Self::Cu8 => 2, Self::Cs16 => 4, Self::Cf32 => 8 } }
    /// Name für rx_sdr -I/-F
    pub fn rx_sdr_name(self) -> &'static str { match self { Self::Cu8 => "CU8", Self::Cs16 => "CS16", Self::Cf32 => "CF32" } }
}

/// 16-Bit-IQ (little endian, I Q I Q …) → Complex32 im Bereich ±1
pub fn s16_to_complex(raw: &[u8]) -> IqBuffer {
    raw.chunks_exact(4).map(|b| Complex32::new(
        i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        i16::from_le_bytes([b[2], b[3]]) as f32 / 32768.0)).collect()
}

/// 32-Bit-Gleitkomma-IQ (little endian) → Complex32
pub fn f32_to_complex(raw: &[u8]) -> IqBuffer {
    raw.chunks_exact(8).map(|b| Complex32::new(
        f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        f32::from_le_bytes([b[4], b[5], b[6], b[7]]))).collect()
}

/// Rohdaten im angegebenen Format umrechnen
pub fn to_complex(fmt: IqFormat, raw: &[u8]) -> IqBuffer {
    match fmt { IqFormat::Cu8 => u8_to_complex(raw), IqFormat::Cs16 => s16_to_complex(raw), IqFormat::Cf32 => f32_to_complex(raw) }
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
    /// Rohformat für rx_sdr (`format` im Band); None = Voreinstellung des Treibers
    pub format: Option<String>,
}

impl SdrConfig {
    /// Format, in dem der Unterprozess liefert: rx_sdr/soapy nach `format` (Voreinstellung cs16), sonst 8 Bit
    pub fn iq_format(&self) -> IqFormat {
        match self.driver.as_str() {
            "rx_sdr" | "soapy" => self.format.as_deref().and_then(IqFormat::parse).unwrap_or(IqFormat::Cs16),
            _ => IqFormat::Cu8,
        }
    }
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
            format: None,
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
            format: c.format.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formate() {
        assert_eq!(IqFormat::parse("CS16"), Some(IqFormat::Cs16));
        assert_eq!(IqFormat::parse("cf32"), Some(IqFormat::Cf32));
        assert_eq!(IqFormat::parse("s12"), None);
        let s16 = [0x00u8, 0x40, 0x00, 0xC0];            // +16384, −16384
        let c = s16_to_complex(&s16);
        assert_eq!(c.len(), 1);
        assert!((c[0].re - 0.5).abs() < 1e-6 && (c[0].im + 0.5).abs() < 1e-6);
        let mut f = Vec::new(); f.extend_from_slice(&0.25f32.to_le_bytes()); f.extend_from_slice(&(-1.0f32).to_le_bytes());
        let c = f32_to_complex(&f);
        assert!((c[0].re - 0.25).abs() < 1e-7 && (c[0].im + 1.0).abs() < 1e-7);
        let mut sc = SdrConfig { driver: "rx_sdr".into(), device: "driver=soapyMiri".into(), tcp_host: String::new(), tcp_port: 0,
            center_freq: 145_000_000, sample_rate: 2_048_000, gain: 30.0, ppm: 0, bias_tee: false, gain_elements: HashMap::new(), format: None };
        assert_eq!(sc.iq_format(), IqFormat::Cs16);
        sc.format = Some("cf32".into()); assert_eq!(sc.iq_format(), IqFormat::Cf32);
        sc.driver = "rtl_sdr".into(); assert_eq!(sc.iq_format(), IqFormat::Cu8);
    }
}
