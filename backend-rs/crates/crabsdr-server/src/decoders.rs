//! Decoder: externe Programme bekommen den demodulierten Ton eines crabSDR-Kanals auf stdin und melden Treffer als
//! JSON-Zeilen auf stdout.
//!
//! - **Plugin** (`plugin_dir/<name>/decoder.json`): was der Decoder braucht – Betriebsart, Abtastrate, Bandbreite,
//!   Pegel, Programmaufruf, benötigte Programme. Beispiele: `plugins/aprs`, `plugins/ft8`, `plugins/sstv`.
//! - **Einsatz** (`[[decoders]]` in config.toml): welches Plugin auf welcher Frequenz lauscht, öffentlich oder nicht.
//! - **Laufzeit**: je Einsatz ein virtueller Hörer im Band (teilt sich den Kanal mit Hörern gleicher Einstellung),
//!   ein eigener Prozess mit eigener Schreib-Aufgabe (ein hängender Decoder hält nichts anderes auf; der DSP-Thread
//!   schickt ohnehin nur mit try_send), Neustart mit wachsender Pause, stderr ins Log.
//! - **Treffer**: Ringpuffer im Speicher, `GET /api/decoders` (Status), `GET /api/decoders/events?since=&wait=1`
//!   (Long-Poll wie der Chat), Dateien der Decoder (z. B. SSTV-Bilder) unter `/api/decoders/files/<id>/<pfad>`.
//!
//! Protokoll des Decoders: stdin = s16le mono mit `rate`; stdout = eine Zeile je Treffer, am besten JSON mit
//! `kind` (z. B. "packet", "position", "decode", "image") und beliebigen Feldern; andere Zeilen werden zu
//! `{kind:"text", text}`. Umgebung: CRAB_FREQ, CRAB_RATE, CRAB_MODE, CRAB_BAND, CRAB_ID, CRAB_LABEL, CRAB_DATA,
//! CRAB_OPT_<NAME>; im Befehl dieselben Werte als {freq} {rate} {mode} {band} {id} {label} {data} {opt.name}.

use axum::{
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use crabsdr_core::{AgcMode, DecoderInstanceConfig, DemodMode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{info, warn};

use crate::client::PLUGIN_CLIENT_BASE;
use crate::sdr_pipeline::SdrPipeline;
use crate::AppState;

/// Virtuelle Hörer der Decoder liegen hinter denen der alten Plugins
const DECODER_CLIENT_BASE: u64 = PLUGIN_CLIENT_BASE + 0x1_0000;
const KEEP_EVENTS: usize = 2000;
const STDERR_TAIL: usize = 20;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub description: String,
    #[serde(default)]
    pub input: InputSpec,
    pub command: Vec<String>,
    /// Programme, die im PATH (oder im Plugin-Verzeichnis) vorhanden sein müssen
    #[serde(default)]
    pub requires: Vec<String>,
    /// Liefert der Decoder Ton zurück (z. B. FreeDV: dekodierte Sprache)? Dann ist stdout rohes s16le mono mit `rate`,
    /// Treffer kommen als JSON-Zeilen auf stderr (Zeilen, die mit `{` beginnen). crabSDR verteilt den Ton als Opus an
    /// Hörer, die `listen_decoder` gewählt haben, und unter /stream/decoder/<id>.ogg.
    #[serde(default)]
    pub output: Option<OutputSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputSpec {
    #[serde(default = "d_kind")]
    #[allow(dead_code)]
    pub kind: String,
    #[serde(default = "d_out_rate")]
    pub rate: u32,
}
fn d_out_rate() -> u32 { 8000 }

#[derive(Debug, Clone, Deserialize)]
pub struct InputSpec {
    /// "audio" (s16le mono); IQ folgt
    #[serde(default = "d_kind")]
    #[allow(dead_code)]
    pub kind: String,
    #[serde(default = "d_mode")]
    pub mode: String,
    #[serde(default = "d_rate")]
    pub rate: u32,
    #[serde(default = "d_bw")]
    pub bandwidth: u32,
    /// "raw" (Diskriminator/SSB ohne AGC) oder "norm" (langsamer Pegelregler wie in multichan.py, für SSB-Decoder)
    #[serde(default = "d_level")]
    pub level: String,
    /// feste Verstärkung für den Rohton (z. B. APRS: direwolf will einen Pegel um 50, der Rohdiskriminator liefert ~5)
    #[serde(default = "d_gain")]
    pub gain: f32,
}
fn d_kind() -> String { "audio".into() }
fn d_mode() -> String { "fm".into() }
fn d_rate() -> u32 { 48000 }
fn d_bw() -> u32 { 12500 }
fn d_level() -> String { "raw".into() }
fn d_gain() -> f32 { 1.0 }
impl Default for InputSpec {
    fn default() -> Self { Self { kind: d_kind(), mode: d_mode(), rate: d_rate(), bandwidth: d_bw(), level: d_level(), gain: d_gain() } }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Event {
    pub seq: u64,
    pub t: f64,
    pub id: String,
    pub plugin: String,
    pub label: String,
    pub band: String,
    pub freq: u64,
    pub kind: String,
    pub data: Value,
    #[serde(skip)]
    pub public: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub id: String,
    pub plugin: String,
    pub label: String,
    pub band: String,
    pub freq: u64,
    pub mode: String,
    pub public: bool,
    /// "läuft" | "startet" | "wartet" | "fehlt: <programm>" | "kein Band" | "Plugin fehlt"
    pub state: String,
    pub since: u64,
    pub restarts: u32,
    pub events: u64,
    pub last_event: Option<u64>,
    pub audio_s: f64,
    /// liefert Ton zurück (hörbar über listen_decoder / /stream/decoder/<id>.ogg)
    pub audio: bool,
    #[serde(skip)]
    pub stderr: VecDeque<String>,
    /// Rundsender des Bandes: Sync/Text-Treffer von Ton-Decodern gehen live an alle Hörer (Knopf „DV“ leuchtet bei Lock)
    #[serde(skip)]
    pub bcast: Option<tokio::sync::broadcast::Sender<Vec<u8>>>,
    /// zur Laufzeit angelegt (Betriebsart DV: FreeDV auf der Frequenz des Hörers), endet, wenn niemand mehr hört
    pub dynamic: bool,
    #[serde(skip)]
    pub idle: u8,
    #[serde(skip)]
    pub client_id: u64,
    #[serde(skip)]
    pub opts: HashMap<String, String>,
    #[serde(skip)]
    pub enabled: bool,
}

pub struct DecoderHub {
    inner: Mutex<Inner>,
    notify: Notify,
    data_dir: PathBuf,
    /// Zusätzliche Umgebung für alle Plugins (Station: CRAB_STATION_NAME/_CALL/_LOCATOR/_LAT/_LON)
    extra_env: std::sync::Mutex<Vec<(String, String)>>,
    mqtt: std::sync::Mutex<Option<mpsc::Sender<Event>>>,
    /// Ton aus Decodern: je Decoder-ID ein Kodierer, je Hörer ein Sender (WebSocket-Hörer und Streams)
    audio: std::sync::Mutex<AudioFan>,
    /// Plugin-Ordner und Bänder, damit Decoder auch zur Laufzeit entstehen können (DV)
    plugin_dir: std::sync::Mutex<PathBuf>,
    pipelines: std::sync::Mutex<HashMap<String, Arc<SdrPipeline>>>,
}

/// Verteilt dekodierten Ton (PCM vom Plugin) als Opus-Rahmen (Tag 0x82, 24 kHz wie die Band-Kanäle) an Abonnenten
#[derive(Default)]
struct AudioFan {
    /// Decoder-ID → (Umrechner auf 24 kHz, Opus-Kodierer)
    enc: HashMap<String, (crabsdr_dsp::resample::Resampler, crate::dsp_thread::OpusEnc)>,
    /// Hörer-ID → Sender; welche Decoder-ID er hört
    subs: HashMap<u64, (String, mpsc::Sender<Vec<u8>>)>,
}
struct Inner {
    seq: u64,
    events: VecDeque<Event>,
    status: Vec<Status>,
}

fn now_s() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn now_f() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

impl DecoderHub {
    /// Stationsangaben für die Plugins (vor `start`)
    pub fn set_station(&self, st: &crabsdr_core::config::StationConfig) {
        let mut v = vec![("CRAB_STATION_NAME".to_string(), st.name.clone()), ("CRAB_STATION_LOCATOR".to_string(), st.locator.clone())];
        if let Some((la, lo)) = st.home() { v.push(("CRAB_STATION_LAT".into(), format!("{:.5}", la))); v.push(("CRAB_STATION_LON".into(), format!("{:.5}", lo))); }
        *self.extra_env.lock().unwrap() = v;
    }

    pub fn data_dir(&self) -> &std::path::Path { &self.data_dir }

    /// Kennungen der Decoder eines Plugins, die diese Person sehen darf, in Reihenfolge der Konfiguration
    pub async fn visible_ids(&self, plugin: &str, p: &crate::access::Principal) -> Vec<String> {
        self.inner.lock().await.status.iter().filter(|s| s.plugin == plugin && p.may_decoder(&s.id, s.public)).map(|s| s.id.clone()).collect()
    }

    /// Für die Admin-Seite: alle Decoder mit den letzten Zeilen ihrer Fehlerausgabe
    pub async fn admin_list(&self) -> Vec<Value> {
        self.inner.lock().await.status.iter().map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
            v["stderr"] = json!(s.stderr.iter().rev().take(20).rev().collect::<Vec<_>>());
            v
        }).collect()
    }

    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(Inner { seq: 0, events: VecDeque::new(), status: Vec::new() }), notify: Notify::new(), data_dir, extra_env: std::sync::Mutex::new(Vec::new()), mqtt: std::sync::Mutex::new(None), audio: std::sync::Mutex::new(AudioFan::default()), plugin_dir: std::sync::Mutex::new(PathBuf::new()), pipelines: std::sync::Mutex::new(HashMap::new()) })
    }

    /// Hörer abonniert den Ton eines Decoders (ersetzt sein Band-Abstimmen); false, wenn der Decoder keinen Ton liefert
    pub async fn subscribe_audio(&self, id: &str, client: u64, tx: mpsc::Sender<Vec<u8>>) -> bool {
        let ok = self.inner.lock().await.status.iter().any(|s| s.id == id && s.audio);
        if ok { self.audio.lock().unwrap().subs.insert(client, (id.to_string(), tx)); }
        ok
    }
    pub fn unsubscribe_audio(&self, client: u64) { self.audio.lock().unwrap().subs.remove(&client); }
    /// Sichtbarkeit eines Decoders: Some(public) oder None, wenn es ihn nicht gibt
    pub async fn is_public(&self, id: &str) -> Option<bool> { self.inner.lock().await.status.iter().find(|s| s.id == id).map(|s| s.public) }
    pub fn audio_listeners(&self, id: &str) -> usize { self.audio.lock().unwrap().subs.values().filter(|(d, _)| d == id).count() }

    /// PCM (s16le mono, `rate`) vom Plugin: auf 24 kHz, Opus, an alle Abonnenten dieses Decoders
    fn audio_frame(&self, id: &str, rate: u32, bytes: &[u8]) {
        let mut g = self.audio.lock().unwrap();
        if !g.subs.values().any(|(d, _)| d == id) { g.enc.remove(id); return; }   // niemand hört: nichts kodieren
        let (res, enc) = match g.enc.entry(id.to_string()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let Some(enc) = crate::dsp_thread::OpusEnc::new(32_000, 5) else { return };
                v.insert((crabsdr_dsp::resample::Resampler::new(rate, crate::dsp_thread::OPUS_RATE), enc))
            }
        };
        let f: Vec<f32> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let up = res.process(&f);
        let pcm: Vec<i16> = up.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        let packets = enc.encode(&pcm);
        if packets.is_empty() { return; }
        let frames: Vec<Vec<u8>> = packets.iter().map(|p| crabsdr_core::protocol::encode_opus_audio(p)).collect();
        let mut dead = Vec::new();
        for (cid, (d, tx)) in g.subs.iter() {
            if d != id { continue; }
            for fr in &frames { if tx.try_send(fr.clone()).is_err() && tx.is_closed() { dead.push(*cid); } }
        }
        for c in dead { g.subs.remove(&c); }
    }

    async fn set_state(&self, idx: usize, state: &str) {
        let mut g = self.inner.lock().await;
        if let Some(s) = g.status.get_mut(idx) {
            if s.state != state { s.state = state.to_string(); s.since = now_s(); }
        }
    }

    async fn push(&self, idx: usize, kind: String, data: Value) {
        let mut g = self.inner.lock().await;
        let Some(st) = g.status.get(idx).cloned() else { return };
        g.seq += 1;
        let ev = Event { seq: g.seq, t: now_f(), id: st.id.clone(), plugin: st.plugin.clone(), label: st.label.clone(), band: st.band.clone(), freq: st.freq, kind, data, public: st.public };
        if let Some(tx) = self.mqtt.lock().ok().and_then(|m| m.clone()) { let _ = tx.try_send(ev.clone()); }   // voll = verwerfen, nie blockieren
        g.events.push_back(ev);
        // Live an die Hörer des Bandes (nur Ton-Decoder: Sync und Text), damit die Oberfläche einen Lock anzeigen kann
        let last = g.events.back().map(|e| (e.kind.clone(), e.data.clone()));
        if let (true, Some((k, data))) = (st.audio, last) {
            if let (true, Some(tx)) = (k == "sync" || k == "text", &st.bcast) {
                let msg = json!({ "type": "decoder", "id": st.id, "kind": k, "data": data });
                let _ = tx.send(crabsdr_core::protocol::encode_json(&msg.to_string()));
            }
        }
        while g.events.len() > KEEP_EVENTS { g.events.pop_front(); }
        if let Some(s) = g.status.get_mut(idx) { s.events += 1; s.last_event = Some(now_s()); }
        drop(g);
        self.notify.notify_waiters();
    }

    /// Alle `[[decoders]]` starten. Braucht die laufenden Bänder (für den virtuellen Hörer).
    pub async fn start(self: &Arc<Self>, cfgs: &[DecoderInstanceConfig], plugin_dir: &Path, pipelines: &HashMap<String, Arc<SdrPipeline>>) {
        *self.plugin_dir.lock().unwrap() = std::path::absolute(plugin_dir).unwrap_or(plugin_dir.to_path_buf());
        *self.pipelines.lock().unwrap() = pipelines.clone();
        for c in cfgs {
            let idx = self.add_status(c, false).await;
            self.launch(idx).await;
        }
        self.spawn_janitor();
    }

    fn manifest_of(&self, plugin: &str) -> (PathBuf, Option<Manifest>) {
        let pdir = self.plugin_dir.lock().unwrap().join(plugin);
        let m = std::fs::read_to_string(pdir.join("decoder.json")).ok().and_then(|s| match serde_json::from_str::<Manifest>(&s) {
            Ok(m) => Some(m),
            Err(e) => { warn!("Plugin '{}': decoder.json unlesbar: {}", plugin, e); None }
        });
        (pdir, m)
    }

    /// Band, in dem der Kanal ganz liegt (mit Abstand zum Rand)
    fn band_for(&self, freq: u64, bw: u64, wanted: Option<&str>) -> Option<Arc<SdrPipeline>> {
        let pipes = self.pipelines.lock().unwrap();
        if let Some(b) = wanted { return pipes.get(b).cloned(); }
        pipes.values().find(|p| {
            let cf = crate::sdr_pipeline::corrected_center(p.center_freq.load(std::sync::atomic::Ordering::Relaxed), p.corr_ppm);
            let half = p.sample_rate.load(std::sync::atomic::Ordering::Relaxed) as u64 / 2;
            let margin = bw / 2 + 10_000;
            freq >= cf.saturating_sub(half) + margin && freq + margin <= cf + half
        }).cloned()
    }

    /// Statuszeile anlegen (noch nicht gestartet); liefert den Index
    async fn add_status(&self, c: &DecoderInstanceConfig, dynamic: bool) -> usize {
        let id = c.id.clone().unwrap_or_else(|| format!("{}-{}", c.plugin, c.freq / 1000));
        let (_, manifest) = self.manifest_of(&c.plugin);
        let bw = manifest.as_ref().map(|m| m.input.bandwidth as u64).unwrap_or(12_500);
        let band = self.band_for(c.freq, bw, c.band.as_deref());
        let label = c.label.clone().or_else(|| manifest.as_ref().map(|m| if m.label.is_empty() { m.name.clone() } else { m.label.clone() })).unwrap_or_else(|| c.plugin.clone());
        let mode = manifest.as_ref().map(|m| m.input.mode.clone()).unwrap_or_default();
        let state = if !c.enabled { "aus".to_string() } else if manifest.is_none() { "Plugin fehlt".into() } else if band.is_none() { "kein Band".into() } else { "startet".into() };
        let mut g = self.inner.lock().await;
        let idx = g.status.len();
        g.status.push(Status { id, plugin: c.plugin.clone(), label, band: band.as_ref().map(|b| b.id.clone()).unwrap_or_default(),
            freq: c.freq, mode, public: c.public, state, since: now_s(), restarts: 0, events: 0, last_event: None, audio_s: 0.0, audio: false, stderr: VecDeque::new(),
            bcast: band.as_ref().map(|b| b.spectrum_tx.clone()), dynamic, idle: 0, client_id: DECODER_CLIENT_BASE + idx as u64, opts: c.options.clone(), enabled: c.enabled });
        idx
    }

    /// Decoder mit Statusindex starten: Plugin prüfen, virtuellen Hörer im Band anmelden, Prozess betreuen
    async fn launch(self: &Arc<Self>, idx: usize) {
        let st = { let g = self.inner.lock().await; g.status.get(idx).cloned() };
        let Some(st) = st else { return };
        let (pdir, manifest) = self.manifest_of(&st.plugin);
        let band = if st.band.is_empty() { None } else { self.pipelines.lock().unwrap().get(&st.band).cloned() };
        let (Some(m), Some(pipe), true) = (manifest, band, st.enabled) else {
            warn!("Decoder '{}': nicht gestartet ({})", st.id, st.state);
            return;
        };
        if let Some(miss) = m.requires.iter().find(|p| !have_program(p, &pdir)) {
            warn!("Decoder '{}': Programm '{}' fehlt", st.id, miss);
            self.set_state(idx, &format!("fehlt: {}", miss)).await;
            return;
        }
        let data = self.data_dir.join("decoders").join(&st.id);
        let _ = std::fs::create_dir_all(&data);
        // absolut: die Plugins laufen in ihrem eigenen Ordner, ein relativer data_dir zeigte sonst dorthin
        let data = std::path::absolute(&data).unwrap_or(data);
        // virtueller Hörer im Band: Rohton in der Rate des Plugins
        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
        {
            let dm = DemodMode::from_str(&m.input.mode).unwrap_or(DemodMode::Fm);
            let mut cm = pipe.clients.lock().await;
            cm.add(st.client_id, tx);
            cm.update_tune(st.client_id, st.freq, dm, m.input.bandwidth);
            cm.set_output_rate(st.client_id, m.input.rate, true);
            cm.set_agc_mode(st.client_id, AgcMode::Off);
        }
        self.set_state(idx, "startet").await;
        info!("Decoder '{}' ({}) auf {} Hz im Band '{}', {} {} Hz, bw {}", st.id, m.name, st.freq, pipe.id, m.input.mode, m.input.rate, m.input.bandwidth);
        let env = Env { freq: st.freq, rate: m.input.rate, mode: m.input.mode.clone(), band: pipe.id.clone(), id: st.id.clone(), label: st.label.clone(), data: data.clone(), opts: st.opts.clone() };
        let hub = self.clone();
        tokio::spawn(run_instance(hub, idx, m, pdir, env, rx));
    }

    /// Betriebsart DV: FreeDV-Decoder auf der Frequenz des Hörers (auf 100 Hz gerundet) im Band `band` – vorhandenen
    /// wiederverwenden, beendeten neu starten, sonst anlegen (höchstens `max` laufende dynamische Decoder je Station)
    pub async fn ensure_dynamic(self: &Arc<Self>, plugin: &str, freq: u64, band: &str, max: u32) -> Result<String, String> {
        let freq = (freq + 50) / 100 * 100;
        let id = format!("dv-{}", freq);
        let existing = { let g = self.inner.lock().await; g.status.iter().enumerate().find(|(_, s)| s.id == id).map(|(i, s)| (i, s.state.clone())) };
        match existing {
            Some((_, state)) if state != "beendet" && !state.starts_with("fehlt") && state != "kein Band" => return Ok(id),
            Some((idx, _)) => {
                { let mut g = self.inner.lock().await; if let Some(s) = g.status.get_mut(idx) { s.idle = 0; s.band = band.to_string(); s.enabled = true; s.events = 0; } }
                self.launch(idx).await;
                let ok = self.inner.lock().await.status.get(idx).map(|s| s.state == "startet" || s.state == "läuft").unwrap_or(false);
                return if ok { Ok(id) } else { Err("Decoder konnte nicht starten (Plugin oder Programm fehlt)".into()) };
            }
            None => {}
        }
        let running = self.inner.lock().await.status.iter().filter(|s| s.dynamic && s.state != "beendet").count();
        if running >= max as usize { return Err(format!("Schon {} DV-Decoder in Betrieb (dv_max), bitte später", max)); }
        let (_, m) = self.manifest_of(plugin);
        let bw = m.as_ref().map(|m| m.input.bandwidth as u64).unwrap_or(3000);
        if self.band_for(freq, bw, Some(band)).is_none() { return Err("Band unbekannt".into()); }
        let c = DecoderInstanceConfig { plugin: plugin.to_string(), freq, id: Some(id.clone()), band: Some(band.to_string()),
            label: Some(format!("DV {:.3} MHz", freq as f64 / 1e6)), enabled: true, public: true, options: HashMap::new() };
        let idx = self.add_status(&c, true).await;
        self.launch(idx).await;
        let ok = self.inner.lock().await.status.get(idx).map(|s| s.state == "startet" || s.state == "läuft").unwrap_or(false);
        if ok { Ok(id) } else { Err("Decoder konnte nicht starten (Plugin oder Programm fehlt)".into()) }
    }

    /// Dynamischen Decoder beenden: virtuellen Hörer abmelden → run_instance endet, Prozess wird beendet
    async fn stop_instance(&self, idx: usize) {
        let st = { let g = self.inner.lock().await; g.status.get(idx).cloned() };
        let Some(st) = st else { return };
        let pipe = self.pipelines.lock().unwrap().get(&st.band).cloned();
        if let Some(p) = pipe { p.clients.lock().await.remove(st.client_id); }
        self.audio.lock().unwrap().enc.remove(&st.id);
        self.set_state(idx, "beendet").await;
        info!("Decoder '{}' beendet (niemand hört mehr)", st.id);
    }

    /// Alle 30 s: dynamische Decoder ohne Hörer nach drei Runden (≈ 90 s) beenden
    fn spawn_janitor(self: &Arc<Self>) {
        let hub = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let candidates: Vec<(usize, String)> = hub.inner.lock().await.status.iter().enumerate()
                    .filter(|(_, s)| s.dynamic && s.state != "beendet").map(|(i, s)| (i, s.id.clone())).collect();
                for (idx, id) in candidates {
                    let listeners = hub.audio_listeners(&id);
                    let stop = { let mut g = hub.inner.lock().await; match g.status.get_mut(idx) { Some(s) => { if listeners == 0 { s.idle += 1; } else { s.idle = 0; } s.idle >= 3 } None => false } };
                    if stop { hub.stop_instance(idx).await; }
                }
            }
        });
    }
}

struct Env { freq: u64, rate: u32, mode: String, band: String, id: String, label: String, data: PathBuf, opts: HashMap<String, String> }

impl Env {
    fn subst(&self, s: &str) -> String {
        let mut o = s.replace("{freq}", &self.freq.to_string()).replace("{rate}", &self.rate.to_string()).replace("{mode}", &self.mode)
            .replace("{band}", &self.band).replace("{id}", &self.id).replace("{label}", &self.label).replace("{data}", &self.data.to_string_lossy());
        for (k, v) in &self.opts { o = o.replace(&format!("{{opt.{}}}", k), v); }
        o
    }
}

pub(crate) fn have_program(p: &str, pdir: &Path) -> bool {
    if p.contains('/') { return pdir.join(p).exists() || Path::new(p).exists(); }
    if pdir.join(p).exists() { return true; }
    std::env::var_os("PATH").map(|path| std::env::split_paths(&path).any(|d| d.join(p).is_file())).unwrap_or(false)
}

/// Langsamer Pegelregler (Effektivwert → Ziel, Zeitkonstante 10 s), (für Decoder, die gleichmäßigen Pegel brauchen)
struct LevelNorm { env: Option<f32>, rate: f32 }
impl LevelNorm {
    fn apply(&mut self, s: &mut [i16]) {
        if s.is_empty() { return; }
        let rms = (s.iter().map(|&x| (x as f32) * (x as f32)).sum::<f32>() / s.len() as f32).sqrt() + 1e-3;
        let a = (s.len() as f32 / (10.0 * self.rate)).min(1.0);
        let e = match self.env { None => rms, Some(e) => e + a * (rms - e) };
        self.env = Some(e);
        let g = 1500.0 / e.max(1.0);
        for x in s.iter_mut() { *x = ((*x as f32) * g).clamp(-32000.0, 32000.0) as i16; }
    }
}

async fn run_instance(hub: Arc<DecoderHub>, idx: usize, m: Manifest, pdir: PathBuf, env: Env, mut rx: mpsc::Receiver<Vec<u8>>) {
    let mut restarts: u32 = 0;
    let mut norm = if m.input.level == "norm" { Some(LevelNorm { env: None, rate: m.input.rate as f32 }) } else { None };
    loop {
        let args: Vec<String> = m.command.iter().map(|a| env.subst(a)).collect();
        let prog = if args[0].starts_with("./") || (!args[0].contains('/') && pdir.join(&args[0]).exists()) { pdir.join(args[0].trim_start_matches("./")).to_string_lossy().to_string() } else { args[0].clone() };
        let mut cmd = Command::new(&prog);
        cmd.args(&args[1..]).current_dir(&pdir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true)
            .env("CRAB_FREQ", env.freq.to_string()).env("CRAB_RATE", env.rate.to_string()).env("CRAB_MODE", &env.mode)
            .env("CRAB_BAND", &env.band).env("CRAB_ID", &env.id).env("CRAB_LABEL", &env.label).env("CRAB_DATA", &env.data);
        for (k, v) in hub.extra_env.lock().unwrap().iter() { cmd.env(k, v); }
        for (k, v) in &env.opts { cmd.env(format!("CRAB_OPT_{}", k.to_uppercase()), v); }
        let started = std::time::Instant::now();
        match cmd.spawn() {
            Err(e) => {
                warn!("Decoder '{}': Start fehlgeschlagen: {}", env.id, e);
                hub.set_state(idx, &format!("Startfehler: {}", e)).await;
            }
            Ok(mut child) => {
                hub.set_state(idx, "läuft").await;
                { let mut g = hub.inner.lock().await; if let Some(s) = g.status.get_mut(idx) { s.restarts = restarts; s.audio = m.output.is_some(); } }
                let mut stdin = child.stdin.take();
                let audio_out = m.output.as_ref().map(|o| o.rate);
                // stdout → Treffer (Textdecoder) oder Ton (output = audio)
                if let Some(out) = child.stdout.take() {
                    let hub2 = hub.clone(); let id = env.id.clone();
                    tokio::spawn(async move {
                        if let Some(rate) = audio_out {
                            use tokio::io::AsyncReadExt;
                            let mut out = out; let mut buf = vec![0u8; 4096]; let mut rest: Vec<u8> = Vec::new();
                            while let Ok(n) = out.read(&mut buf).await {
                                if n == 0 { break; }
                                rest.extend_from_slice(&buf[..n]);
                                let even = rest.len() & !1;
                                if even > 0 { let chunk: Vec<u8> = rest.drain(..even).collect(); hub2.audio_frame(&id, rate, &chunk); }
                            }
                            return;
                        }
                        let mut lines = BufReader::new(out).lines();
                        while let Ok(Some(l)) = lines.next_line().await {
                            let l = l.trim(); if l.is_empty() { continue; }
                            let (kind, data) = match serde_json::from_str::<Value>(l) {
                                Ok(Value::Object(o)) => (o.get("kind").and_then(|k| k.as_str()).unwrap_or("text").to_string(), Value::Object(o)),
                                _ => ("text".to_string(), json!({ "text": l })),
                            };
                            hub2.push(idx, kind, data).await;
                        }
                    });
                }
                // stderr → Log + Rest für die Statusanzeige; bei Ton-Decodern sind JSON-Zeilen hier die Treffer
                if let Some(err) = child.stderr.take() {
                    let hub2 = hub.clone(); let id = env.id.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(err).lines();
                        while let Ok(Some(l)) = lines.next_line().await {
                            if audio_out.is_some() && l.trim_start().starts_with('{') {
                                if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(l.trim()) {
                                    let kind = o.get("kind").and_then(|k| k.as_str()).unwrap_or("text").to_string();
                                    hub2.push(idx, kind, Value::Object(o)).await;
                                    continue;
                                }
                            }
                            tracing::debug!("Decoder '{}' stderr: {}", id, l);
                            let mut g = hub2.inner.lock().await;
                            if let Some(s) = g.status.get_mut(idx) { s.stderr.push_back(l.chars().take(200).collect()); while s.stderr.len() > STDERR_TAIL { s.stderr.pop_front(); } }
                        }
                    });
                }
                // Ton zuführen, bis der Prozess endet
                let mut fed: u64 = 0;
                loop {
                    tokio::select! {
                        st = child.wait() => { warn!("Decoder '{}' beendet: {:?}", env.id, st.ok()); break; }
                        f = rx.recv() => {
                            let Some(frame) = f else { hub.set_state(idx, "beendet").await; return };
                            if frame.len() < 3 || frame[0] != crabsdr_core::protocol::TAG_AUDIO { continue; }
                            let bytes = &frame[1..];
                            let gain = m.input.gain;
                            let out: Vec<u8> = if norm.is_some() || gain != 1.0 {
                                let mut s: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                                if gain != 1.0 { for x in s.iter_mut() { *x = ((*x as f32) * gain).clamp(-32000.0, 32000.0) as i16; } }
                                if let Some(n) = norm.as_mut() { n.apply(&mut s); }
                                s.iter().flat_map(|v| v.to_le_bytes()).collect()
                            } else { bytes.to_vec() };
                            fed += (out.len() / 2) as u64;
                            if let Some(si) = stdin.as_mut() {
                                if si.write_all(&out).await.is_err() { stdin = None; }
                            }
                            if fed % (env.rate as u64 * 10) < (out.len() / 2) as u64 {
                                let mut g = hub.inner.lock().await;
                                if let Some(s) = g.status.get_mut(idx) { s.audio_s = fed as f64 / env.rate as f64; }
                            }
                        }
                    }
                }
            }
        }
        // Neustart mit wachsender Pause (2 … 60 s), nach 5 min Laufzeit wieder klein anfangen
        if started.elapsed() > Duration::from_secs(300) { restarts = 0; }
        restarts += 1;
        hub.set_state(idx, "wartet").await;
        let pause = Duration::from_secs((2u64 << restarts.min(5)).min(60));
        let until = tokio::time::Instant::now() + pause;
        // in der Pause Ton verwerfen (nichts aufstauen)
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                f = rx.recv() => { if f.is_none() { hub.set_state(idx, "beendet").await; return; } }
            }
        }
        { let mut g = hub.inner.lock().await; if let Some(s) = g.status.get_mut(idx) { s.restarts = restarts; } }
    }
}

// ---------- MQTT ----------

impl DecoderHub {
    /// Treffer an einen MQTT-Broker weiterreichen (eigene Aufgabe, verbindet sich selbst neu; Warteschlange 1000 Treffer).
    pub fn start_mqtt(&self, cfg: &crabsdr_core::MqttConfig) {
        if !cfg.enabled { return; }
        use rumqttc::{AsyncClient, LastWill, MqttOptions, QoS};
        let pass = cfg.password_file.as_ref().and_then(|p| std::fs::read_to_string(p).map_err(|e| warn!("MQTT: Passwortdatei {:?}: {}", p, e)).ok()).map(|s| s.trim().to_string());
        let topic = cfg.topic.trim_end_matches('/').to_string();
        let station = topic.rsplit('/').next().unwrap_or("crabsdr").to_string();
        let mut opts = MqttOptions::new(format!("crabsdr-{}", station), cfg.host.clone(), cfg.port);
        opts.set_keep_alive(Duration::from_secs(30));
        if let Some(u) = &cfg.username { opts.set_credentials(u.clone(), pass.unwrap_or_default()); }
        let status = format!("{}/status", topic);
        opts.set_last_will(LastWill::new(status.clone(), "offline", QoS::AtLeastOnce, true));
        let (client, mut el) = AsyncClient::new(opts, 1000);
        let (tx, mut rx) = mpsc::channel::<Event>(1000);
        *self.mqtt.lock().unwrap() = Some(tx);
        let (c2, st2, host) = (client.clone(), status.clone(), format!("{}:{}", cfg.host, cfg.port));
        tokio::spawn(async move {
            loop {
                match el.poll().await {
                    Ok(rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                        info!("MQTT verbunden mit {}", host);
                        let _ = c2.publish(st2.clone(), QoS::AtLeastOnce, true, "online").await;
                    }
                    Ok(_) => {}
                    Err(e) => { warn!("MQTT {}: {} – neuer Versuch in 10 s", host, e); tokio::time::sleep(Duration::from_secs(10)).await; }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let t = format!("{}/{}/{}", topic, ev.id, ev.kind.replace(['/', '+', '#'], "_"));
                let payload = json!({ "station": station, "seq": ev.seq, "t": ev.t, "id": ev.id, "plugin": ev.plugin, "label": ev.label,
                    "band": ev.band, "freq": ev.freq, "kind": ev.kind, "public": ev.public, "data": ev.data });
                if client.try_publish(t, QoS::AtLeastOnce, false, payload.to_string()).is_err() {
                    tracing::debug!("MQTT: Warteschlange voll, Treffer verworfen");
                }
            }
        });
    }
}

// ---------- HTTP ----------

pub async fn api_list(State(state): State<Arc<AppState>>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    let p = crate::access::from_headers_or_guest(&state, &headers).await;
    let g = state.decoders.inner.lock().await;
    let list: Vec<Value> = g.status.iter().filter(|s| p.may_decoder(&s.id, s.public) && !(s.dynamic && s.state == "beendet")).map(|s| serde_json::to_value(s).unwrap_or(Value::Null)).collect();
    drop(g);
    // Betriebsart DV (FreeDV auf beliebiger Frequenz) verfügbar? Plugin vorhanden und dv_max > 0
    let dv = state.config.read().await.dv_max > 0 && state.decoders.plugin_dir.lock().unwrap().join("freedv").join("decoder.json").exists();
    ([(header::CACHE_CONTROL, "no-store")], Json(json!({ "decoders": list, "dv": dv, "t": now_s() })))
}

#[derive(Deserialize)]
pub struct EvQuery { since: Option<u64>, id: Option<String>, wait: Option<u8>, limit: Option<usize> }

async fn events_since(hub: &DecoderHub, q: &EvQuery, p: &crate::access::Principal) -> (u64, Vec<Event>) {
    let g = hub.inner.lock().await;
    let since = q.since.unwrap_or(0);
    let lim = q.limit.unwrap_or(200).min(KEEP_EVENTS);
    let mut v: Vec<Event> = g.events.iter().filter(|e| p.may_decoder(&e.id, e.public) && e.seq > since && q.id.as_ref().is_none_or(|i| &e.id == i)).cloned().collect();
    if v.len() > lim { v.drain(..v.len() - lim); }
    (g.seq, v)
}

pub async fn api_events(State(state): State<Arc<AppState>>, headers: axum::http::HeaderMap, Query(q): Query<EvQuery>) -> impl IntoResponse {
    let p = crate::access::from_headers_or_guest(&state, &headers).await;
    let hub = state.decoders.clone();
    let (mut seq, mut ev) = events_since(&hub, &q, &p).await;
    if ev.is_empty() && q.wait == Some(1) {
        let _ = tokio::time::timeout(Duration::from_secs(25), hub.notify.notified()).await;
        let r = events_since(&hub, &q, &p).await; seq = r.0; ev = r.1;
    }
    ([(header::CACHE_CONTROL, "no-store")], Json(json!({ "seq": seq, "events": ev })))
}

#[derive(Deserialize)]
pub struct FileQuery { token: Option<String> }

/// Bilder und Übersichten eines Decoders (z. B. SSTV-Bilder) aus `data_dir/decoders/<id>/`. Nur .png/.jpg/.json –
/// im Datenordner liegen auch Arbeitsdateien (direwolf.conf mit Passcode, Protokolle), die niemand abrufen darf.
/// Bilder nicht öffentlicher Decoder: Token im Kopf oder als ?token= (für <img>, nur Hören-Tokens).
pub async fn api_file(State(state): State<Arc<AppState>>, headers: axum::http::HeaderMap, Query(fq): Query<FileQuery>, AxPath((id, path)): AxPath<(String, String)>) -> Response {
    let tok = crate::access::bearer(&headers).or(fq.token);
    let p = crate::access::resolve(&state, tok.as_deref()).await.unwrap_or_else(|_| crate::access::Principal::guest());
    let ok_id = { let g = state.decoders.inner.lock().await; g.status.iter().any(|s| s.id == id && p.may_decoder(&s.id, s.public)) };
    let ext_ok = matches!(std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg" | "json"));
    let name_ok = path.split('/').all(|seg| !seg.is_empty() && seg != ".." && !seg.starts_with('.'));
    if !ok_id || !ext_ok || !name_ok || id.contains("..") || id.contains('/') { return StatusCode::NOT_FOUND.into_response(); }
    let file = state.decoders.data_dir.join("decoders").join(&id).join(&path);
    match tokio::fs::read(&file).await {
        Ok(b) => {
            let ct = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", _ => "application/json",
            };
            ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "public, max-age=60")], Body::from(b)).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
