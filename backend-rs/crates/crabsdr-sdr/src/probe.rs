//! SoapySDR device capability probing.
//!
//! Runs `SoapySDRUtil --probe` to discover gain elements, ranges, and AGC support.
//! Falls back to hardcoded profiles for non-SoapySDR drivers.

use crate::{DeviceCapabilities, GainElement};
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;
use tracing::{info, warn};

/// A detected SDR device from SoapySDR --find.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedDevice {
    pub driver: String,
    pub label: String,
    pub serial: String,
    pub product: String,
    pub available: bool,
    pub properties: HashMap<String, String>,
}

/// Probe device capabilities. For SoapySDR (rx_sdr) devices, runs SoapySDRUtil.
/// For rtl_sdr, returns a hardcoded RTL-SDR profile.
pub fn probe_device(driver: &str, device: &str) -> DeviceCapabilities {
    match driver {
        "rtl_sdr" => rtlsdr_profile(),
        "rx_sdr" | "soapy" => soapy_probe(device).unwrap_or_else(|| {
            warn!("SoapySDR probe failed for '{}', using fallback profile", device);
            // Use device-specific fallback when probe fails (e.g. USB busy)
            if device.contains("soapyMiri") || device.contains("Miri") {
                soapymiri_fallback_profile()
            } else if device.contains("sdrplay") {
                sdrplay_fallback_profile()
            } else if device.contains("hackrf") {
                hackrf_profile()
            } else {
                generic_soapy_profile()
            }
        }),
        "hackrf" => hackrf_profile(),
        _ => DeviceCapabilities::default(),
    }
}

fn rtlsdr_profile() -> DeviceCapabilities {
    DeviceCapabilities {
        device_name: "RTL-SDR".into(),
        gain_elements: vec![GainElement {
            name: "TUNER".into(),
            min: 0.0,
            max: 49.6,
            step: Some(0.1),
        }],
        has_agc: true,
    }
}

fn hackrf_profile() -> DeviceCapabilities {
    DeviceCapabilities {
        device_name: "HackRF".into(),
        gain_elements: vec![
            GainElement { name: "AMP".into(), min: 0.0, max: 14.0, step: Some(14.0) },
            GainElement { name: "LNA".into(), min: 0.0, max: 40.0, step: Some(8.0) },
            GainElement { name: "VGA".into(), min: 0.0, max: 62.0, step: Some(2.0) },
        ],
        has_agc: false,
    }
}

fn generic_soapy_profile() -> DeviceCapabilities {
    DeviceCapabilities {
        device_name: "SoapySDR Device".into(),
        gain_elements: vec![GainElement {
            name: "OVERALL".into(),
            min: 0.0,
            max: 50.0,
            step: Some(1.0),
        }],
        has_agc: false,
    }
}

/// Run SoapySDRUtil --probe and parse the output.
fn soapy_probe(device: &str) -> Option<DeviceCapabilities> {
    let output = Command::new("SoapySDRUtil")
        .arg(format!("--probe={}", device))
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{}\n{}", stdout, stderr);

    if text.contains("Error") && text.contains("no available") {
        return None;
    }

    info!("SoapySDR probe output for '{}':\n{}", device, text);

    let mut caps = DeviceCapabilities::default();

    // Parse device name from "-- Device identification" section
    for line in text.lines() {
        if line.contains("driver=") && line.contains("label=") {
            // e.g. "  driver=sdrplay, label=SDRplay Dev0 RSP1"
            if let Some(label_start) = line.find("label=") {
                caps.device_name = line[label_start + 6..].trim().to_string();
            }
        }
    }

    // Parse gain elements from anywhere in probe output.
    // Handles both SDRplay format (under "-- Gain Info:") and
    // SoapyMiri format (under "-- RX Channel 0").
    // Examples:
    //   IFGR gain range: [20, 59] dB
    //   LNA gain range: [0, 1, 1] dB     (min, max, step)
    //   Baseband gain range: [0, 59, 1] dB
    for line in text.lines() {
        let trimmed = line.trim();
        // Skip aggregate ranges
        if trimmed.contains("Full gain range") || trimmed.contains("Automatic gain range") {
            continue;
        }
        if let Some(caps_parsed) = parse_gain_line(trimmed) {
            caps.gain_elements.push(caps_parsed);
        }
    }

    // Parse AGC — multiple formats across SoapySDR modules
    if text.contains("hasGainMode") && text.contains("true") {
        caps.has_agc = true;
    }
    if text.contains("AGC supported") || text.contains("Supports AGC: YES") {
        caps.has_agc = true;
    }

    // If no gain elements parsed, use device name hint
    if caps.gain_elements.is_empty() {
        // Try to detect from device string
        if device.contains("sdrplay") {
            return Some(sdrplay_fallback_profile());
        }
        if device.contains("soapyMiri") || device.contains("Miri") {
            return Some(soapymiri_fallback_profile());
        }
        if device.contains("hackrf") {
            return Some(hackrf_profile());
        }
        // Generic fallback
        caps.gain_elements.push(GainElement {
            name: "OVERALL".into(),
            min: 0.0,
            max: 50.0,
            step: Some(1.0),
        });
    }

    Some(caps)
}

/// Parse a gain range line like "IFGR gain range: [20, 59] dB"
/// Also handles 3-element format: "LNA gain range: [0, 1, 1] dB" (min, max, step)
fn parse_gain_line(line: &str) -> Option<GainElement> {
    if !line.contains("gain range:") {
        return None;
    }

    let name_end = line.find(" gain range:")?;
    let name = line[..name_end].trim().to_string();
    if name.is_empty() {
        return None;
    }

    // Extract range [min, max] or [min, max, step]
    let bracket_start = line.find('[')?;
    let bracket_end = line.find(']')?;
    let range_str = &line[bracket_start + 1..bracket_end];
    let parts: Vec<&str> = range_str.split(',').collect();
    if parts.len() < 2 {
        return None;
    }

    let min: f64 = parts[0].trim().parse().ok()?;
    let max: f64 = parts[1].trim().parse().ok()?;

    // Step from 3rd element in brackets, or from "step=N" after brackets
    let step = if parts.len() >= 3 {
        parts[2].trim().parse::<f64>().ok()
    } else if let Some(step_pos) = line.find("step") {
        let after = &line[step_pos + 4..];
        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit() && *c != '.')
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        digits.parse::<f64>().ok()
    } else {
        None
    };

    Some(GainElement { name, min, max, step })
}

/// Scan for all connected SDR devices via SoapySDRUtil --find.
pub fn scan_devices() -> Vec<DetectedDevice> {
    let output = match Command::new("SoapySDRUtil").arg("--find").output() {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to run SoapySDRUtil --find: {}", e);
            return vec![];
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{}\n{}", stdout, stderr);

    let mut devices = Vec::new();
    let mut current: Option<HashMap<String, String>> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Found device") {
            // Save previous device
            if let Some(props) = current.take() {
                devices.push(build_detected_device(props));
            }
            current = Some(HashMap::new());
        } else if let Some(ref mut props) = current {
            if let Some((key, value)) = trimmed.split_once('=') {
                props.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    // Save last device
    if let Some(props) = current.take() {
        devices.push(build_detected_device(props));
    }

    info!("SoapySDR scan found {} devices", devices.len());
    devices
}

fn build_detected_device(props: HashMap<String, String>) -> DetectedDevice {
    let driver = props.get("driver").cloned().unwrap_or_default();
    let label = props.get("label").cloned().unwrap_or_default();
    let serial = props.get("serial").cloned().unwrap_or_default();
    let product = props.get("product").cloned()
        .or_else(|| props.get("device").cloned())
        .unwrap_or_default();
    let available = !props.get("tuner").is_some_and(|v| v == "unavailable");

    DetectedDevice {
        driver,
        label,
        serial,
        product,
        available,
        properties: props,
    }
}

/// SoapyMiri (MSi2500-based RSP1 clones) fallback profile.
/// Gain elements: LNA (0/1 = off/on) and Baseband (0–59 dB).
/// rx_sdr expects: -g "LNA=1,Baseband=45"
fn soapymiri_fallback_profile() -> DeviceCapabilities {
    DeviceCapabilities {
        device_name: "SoapyMiri RSP1".into(),
        gain_elements: vec![
            GainElement { name: "LNA".into(), min: 0.0, max: 1.0, step: Some(1.0) },
            GainElement { name: "Baseband".into(), min: 0.0, max: 59.0, step: Some(1.0) },
        ],
        has_agc: false,
    }
}

fn sdrplay_fallback_profile() -> DeviceCapabilities {
    DeviceCapabilities {
        device_name: "SDRplay RSP".into(),
        gain_elements: vec![
            GainElement { name: "IFGR".into(), min: 20.0, max: 59.0, step: Some(1.0) },
            GainElement { name: "RFGR".into(), min: 0.0, max: 3.0, step: Some(1.0) },
        ],
        has_agc: true,
    }
}
