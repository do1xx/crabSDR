//! Hörer-Verwaltung: Zustand je WebSocket-Verbindung (Abstimmung, Codec, Rauschsperre, Wasserfall-Abo, Name).
//!
//! Der DSP-Thread holt sich je Rahmen eine Momentaufnahme (`snapshot`) und gruppiert die Hörer nach
//! `ChannelKey`: alle Hörer mit gleicher Frequenz/Modus/Bandbreite/AGC/Ausgaberate teilen sich einen Kanal.

use crabsdr_core::{AgcMode, DemodMode};
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Plugin-Kennungen liegen oberhalb, echte Hörer darunter.
pub const PLUGIN_CLIENT_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Alles, was einen Kanal eindeutig macht. Hörer mit gleichem Schlüssel teilen Ausschnitt, Demod und Opus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelKey {
    pub freq: u64,
    pub mode: DemodMode,
    pub bandwidth: u32,
    pub agc: AgcMode,
    /// Ausgaberate für PCM-Abnehmer (Browser 48000, Decoder z. B. 22050); Opus läuft immer mit 24 kHz
    pub out_rate: u32,
    /// Rohton ohne AGC/Filter (Decoder-Plugins)
    pub raw: bool,
    /// SSB: untere Kante des Durchlassbereichs in Hz (z. B. 300); sonst 0
    pub pass_lo: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquelchMode {
    Off,
    Auto,
    Manual,
}

#[derive(Debug, Clone, Copy)]
pub struct Squelch {
    pub mode: SquelchMode,
    /// Schwelle in dBFS (Manual)
    pub db: f32,
    /// Auto: öffnet, wenn der Pegel so viele dB über dem Rauschboden liegt (Regler in der Oberfläche)
    pub margin_db: f32,
    pub hang_ms: u32,
}

impl Default for Squelch {
    fn default() -> Self {
        Self { mode: SquelchMode::Off, db: -60.0, margin_db: AUTO_SQUELCH_MARGIN_DB, hang_ms: 500 }
    }
}

/// Voreinstellung für den Abstand über dem Rauschboden
pub const AUTO_SQUELCH_MARGIN_DB: f32 = 6.0;

/// Wasserfall-Abo: Zoomstufe (0 = ganzes Band, je Stufe halbe Breite) und Start-Bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaterfallSub {
    pub zoom: u8,
    pub start_bin: u16,
}

pub struct ClientState {
    pub tx: mpsc::Sender<Vec<u8>>,
    pub tune_freq: Option<u64>,
    pub mode: DemodMode,
    pub bandwidth: u32,
    pub pass_lo: u32,
    pub use_opus: bool,
    pub output_sample_rate: u32,
    pub raw_audio: bool,
    pub agc_mode: AgcMode,
    pub squelch: Squelch,
    pub name: String,
    /// Altes Vollspektrum (0x81, 4096 Bins) gewünscht (Svelte-Oberfläche)
    pub wants_full_spectrum: bool,
    pub waterfall: Option<WaterfallSub>,
    /// zählt jede Wasserfall-Anmeldung, damit der DSP-Thread danach eine Absolut-Zeile schickt
    pub wf_seq: u32,
    /// Verlauf beim Anmelden: so viele Zeilen (Wasserfallhöhe), je `wf_slow` Server-Zeilen gemittelt
    pub wf_hist_rows: u16,
    pub wf_slow: u8,
    /// Verlauf als JPEG rendern, Wasserfall-Modus (1–3)
    pub wf_jpeg: bool,
    pub wf_mode: u8,
}

/// Momentaufnahme eines Hörers für den DSP-Thread.
#[derive(Clone)]
pub struct ClientView {
    pub id: u64,
    pub tx: mpsc::Sender<Vec<u8>>,
    pub name: String,
    pub tuned: Option<ChannelKey>,
    pub use_opus: bool,
    pub squelch: Squelch,
    pub wants_full_spectrum: bool,
    pub waterfall: Option<WaterfallSub>,
    pub wf_seq: u32,
    pub wf_hist_rows: u16,
    pub wf_slow: u8,
    pub wf_jpeg: bool,
    pub wf_mode: u8,
}

pub struct ClientManager {
    clients: HashMap<u64, ClientState>,
}

impl ClientManager {
    pub fn new() -> Self {
        Self { clients: HashMap::new() }
    }

    pub fn add(&mut self, id: u64, tx: mpsc::Sender<Vec<u8>>) {
        self.clients.insert(
            id,
            ClientState {
                tx,
                tune_freq: None,
                mode: DemodMode::Fm,
                bandwidth: 12500,
                pass_lo: 0,
                use_opus: false,
                output_sample_rate: 48000,
                raw_audio: false,
                agc_mode: AgcMode::Medium,
                squelch: Squelch::default(),
                name: String::new(),
                wants_full_spectrum: false,
                waterfall: None,
                wf_seq: 0,
                wf_hist_rows: 0,
                wf_slow: 1,
                wf_jpeg: false,
                wf_mode: 1,
            },
        );
    }

    pub fn remove(&mut self, id: u64) {
        self.clients.remove(&id);
    }

    pub fn update_tune(&mut self, id: u64, freq: u64, mode: DemodMode, bandwidth: u32) {
        self.update_tune_lo(id, freq, mode, bandwidth, 0);
    }
    pub fn update_tune_lo(&mut self, id: u64, freq: u64, mode: DemodMode, bandwidth: u32, pass_lo: u32) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.tune_freq = Some(freq);
            c.mode = mode;
            c.bandwidth = bandwidth;
            c.pass_lo = pass_lo;
        }
    }

    /// Abstimmung aufheben (Hörer hört nur noch Wasserfall / wechselt das Band)
    pub fn untune(&mut self, id: u64) {
        if let Some(c) = self.clients.get_mut(&id) { c.tune_freq = None; }
    }

    pub fn get_tune(&self, id: u64) -> Option<(u64, DemodMode, u32)> {
        self.clients.get(&id).and_then(|c| c.tune_freq.map(|f| (f, c.mode, c.bandwidth)))
    }

    pub fn count(&self) -> usize {
        self.clients.len()
    }

    pub fn count_real(&self) -> usize {
        self.clients.keys().filter(|&&id| id < PLUGIN_CLIENT_BASE).count()
    }

    pub fn set_opus(&mut self, id: u64, use_opus: bool) {
        if let Some(c) = self.clients.get_mut(&id) { c.use_opus = use_opus; }
    }

    pub fn set_output_rate(&mut self, id: u64, sample_rate: u32, raw_audio: bool) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.output_sample_rate = sample_rate;
            c.raw_audio = raw_audio;
        }
    }

    pub fn set_agc_mode(&mut self, id: u64, agc_mode: AgcMode) {
        if let Some(c) = self.clients.get_mut(&id) { c.agc_mode = agc_mode; }
    }

    pub fn set_squelch(&mut self, id: u64, squelch: Squelch) {
        if let Some(c) = self.clients.get_mut(&id) { c.squelch = squelch; }
    }

    pub fn set_name(&mut self, id: u64, name: &str) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.name = name.chars().filter(|ch| !ch.is_control()).take(32).collect();
        }
    }

    pub fn set_full_spectrum(&mut self, id: u64, on: bool) {
        if let Some(c) = self.clients.get_mut(&id) { c.wants_full_spectrum = on; }
    }

    pub fn set_waterfall_hist(&mut self, id: u64, rows: u16, slow: u8, jpeg: bool, mode: u8) {
        if let Some(c) = self.clients.get_mut(&id) { c.wf_hist_rows = rows; c.wf_slow = slow.max(1); c.wf_jpeg = jpeg; c.wf_mode = mode; }
    }

    pub fn set_waterfall(&mut self, id: u64, sub: Option<WaterfallSub>) {
        if let Some(c) = self.clients.get_mut(&id) { c.waterfall = sub; c.wf_seq = c.wf_seq.wrapping_add(1); }
    }

    /// Momentaufnahme aller Hörer (ein Lock je Rahmen im DSP-Thread).
    pub fn snapshot(&self) -> Vec<ClientView> {
        self.clients
            .iter()
            .map(|(&id, c)| ClientView {
                id,
                tx: c.tx.clone(),
                name: c.name.clone(),
                tuned: c.tune_freq.map(|freq| ChannelKey {
                    freq,
                    mode: c.mode,
                    bandwidth: c.bandwidth,
                    agc: c.agc_mode,
                    out_rate: c.output_sample_rate,
                    raw: c.raw_audio,
                    pass_lo: c.pass_lo,
                }),
                use_opus: c.use_opus,
                squelch: c.squelch,
                wants_full_spectrum: c.wants_full_spectrum,
                waterfall: c.waterfall,
                wf_seq: c.wf_seq,
                wf_hist_rows: c.wf_hist_rows,
                wf_slow: c.wf_slow,
                wf_jpeg: c.wf_jpeg,
                wf_mode: c.wf_mode,
            })
            .collect()
    }

    /// Hörerliste (echte Hörer) als JSON-Array: id, Name, Frequenz, Modus.
    pub fn listeners_json(&self, band: &str) -> Vec<serde_json::Value> {
        let mut v: Vec<_> = self
            .clients
            .iter()
            .filter(|(&id, _)| id < PLUGIN_CLIENT_BASE)
            .map(|(&id, c)| json!({"id": id, "name": c.name, "band": band, "freq": c.tune_freq, "mode": c.mode.as_str()}))
            .collect();
        v.sort_by_key(|e| e["id"].as_u64().unwrap_or(0));
        v
    }
}
