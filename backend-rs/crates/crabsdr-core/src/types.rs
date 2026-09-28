use num_complex::Complex32;
use serde::{Deserialize, Serialize};

pub type IqSample = Complex32;
pub type IqBuffer = Vec<Complex32>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DemodMode {
    Wfm,
    Fm,
    Am,
    Sam,
    Usb,
    Lsb,
    Cw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgcMode {
    Off,
    Fast,
    Medium,
    Slow,
}

impl AgcMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(AgcMode::Off),
            "fast" => Some(AgcMode::Fast),
            "medium" => Some(AgcMode::Medium),
            "slow" => Some(AgcMode::Slow),
            _ => None,
        }
    }
}

impl DemodMode {
    pub fn default_bandwidth(&self) -> u32 {
        match self {
            DemodMode::Wfm => 150_000,
            DemodMode::Fm => 12_500,
            DemodMode::Am => 9_000,
            DemodMode::Sam => 9_000,
            DemodMode::Usb => 2_700,
            DemodMode::Lsb => 2_700,
            DemodMode::Cw => 500,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DemodMode::Wfm => "wfm",
            DemodMode::Fm => "fm",
            DemodMode::Am => "am",
            DemodMode::Sam => "sam",
            DemodMode::Usb => "usb",
            DemodMode::Lsb => "lsb",
            DemodMode::Cw => "cw",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "wfm" => Some(DemodMode::Wfm),
            "fm" => Some(DemodMode::Fm),
            "am" => Some(DemodMode::Am),
            "sam" => Some(DemodMode::Sam),
            "usb" => Some(DemodMode::Usb),
            "lsb" => Some(DemodMode::Lsb),
            "cw" => Some(DemodMode::Cw),
            _ => None,
        }
    }

    pub fn all() -> &'static [DemodMode] {
        &[
            DemodMode::Wfm,
            DemodMode::Fm,
            DemodMode::Am,
            DemodMode::Sam,
            DemodMode::Usb,
            DemodMode::Lsb,
            DemodMode::Cw,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct TuneState {
    pub freq: u64,
    pub mode: DemodMode,
    pub bandwidth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Tune {
        freq: u64,
        mode: String,
        bandwidth: Option<u32>,
    },
    SetCenterFreq {
        freq: u64,
    },
    SetGain {
        gain: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SdrCommand {
    SetFrequency(u64),
    SetGain(f64),
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdrStatus {
    pub running: bool,
    pub driver: String,
    pub restarts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}
