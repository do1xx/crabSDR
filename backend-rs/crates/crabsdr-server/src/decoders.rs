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
}

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
    #[serde(skip)]
    pub stderr: VecDeque<String>,
}

pub struct DecoderHub {
    inner: Mutex<Inner>,
    notify: Notify,
    data_dir: PathBuf,
    /// Zusätzliche Umgebung für alle Plugins (Station: CRAB_STATION_NAME/_CALL/_LOCATOR/_LAT/_LON)
    extra_env: std::sync::Mutex<Vec<(String, String)>>,
    mqtt: std::sync::Mutex<Option<mpsc::Sender<Event>>>,
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
        Arc::new(Self { inner: Mutex::new(Inner { seq: 0, events: VecDeque::new(), status: Vec::new() }), notify: Notify::new(), data_dir, extra_env: std::sync::Mutex::new(Vec::new()), mqtt: std::sync::Mutex::new(None) })
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
        while g.events.len() > KEEP_EVENTS { g.events.pop_front(); }
        if let Some(s) = g.status.get_mut(idx) { s.events += 1; s.last_event = Some(now_s()); }
        drop(g);
        self.notify.notify_waiters();
    }

    /// Alle `[[decoders]]` starten. Braucht die laufenden Bänder (für den virtuellen Hörer).
    pub async fn start(self: &Arc<Self>, cfgs: &[DecoderInstanceConfig], plugin_dir: &Path, pipelines: &HashMap<String, Arc<SdrPipeline>>) {
        for (idx, c) in cfgs.iter().enumerate() {
            let id = c.id.clone().unwrap_or_else(|| format!("{}-{}", c.plugin, c.freq / 1000));
            let pdir = plugin_dir.join(&c.plugin);
            let pdir = std::path::absolute(&pdir).unwrap_or(pdir);
            let manifest: Option<Manifest> = std::fs::read_to_string(pdir.join("decoder.json")).ok().and_then(|s| match serde_json::from_str(&s) {
                Ok(m) => Some(m),
                Err(e) => { warn!("Decoder '{}': decoder.json unlesbar: {}", id, e); None }
            });
            // Band: angegeben, sonst das erste, in dem der Kanal ganz liegt (mit Abstand zum Rand)
            let bw = manifest.as_ref().map(|m| m.input.bandwidth as u64).unwrap_or(12_500);
            let band = match &c.band {
                Some(b) => pipelines.get(b).cloned(),
                None => pipelines.values().find(|p| {
                    let cf = p.center_freq.load(std::sync::atomic::Ordering::Relaxed);
                    let half = p.sample_rate.load(std::sync::atomic::Ordering::Relaxed) as u64 / 2;
                    let margin = bw / 2 + 10_000;
                    c.freq >= cf.saturating_sub(half) + margin && c.freq + margin <= cf + half
                }).cloned(),
            };
            let label = c.label.clone().or_else(|| manifest.as_ref().map(|m| if m.label.is_empty() { m.name.clone() } else { m.label.clone() })).unwrap_or_else(|| c.plugin.clone());
            let mode = manifest.as_ref().map(|m| m.input.mode.clone()).unwrap_or_default();
            let state = if !c.enabled { "aus".to_string() } else if manifest.is_none() { "Plugin fehlt".into() } else if band.is_none() { "kein Band".into() } else { "startet".into() };
            {
                let mut g = self.inner.lock().await;
                g.status.push(Status { id: id.clone(), plugin: c.plugin.clone(), label: label.clone(), band: band.as_ref().map(|b| b.id.clone()).unwrap_or_default(),
                    freq: c.freq, mode, public: c.public, state: state.clone(), since: now_s(), restarts: 0, events: 0, last_event: None, audio_s: 0.0, stderr: VecDeque::new() });
            }
            let (Some(m), Some(pipe), true) = (manifest, band, c.enabled) else {
                warn!("Decoder '{}': nicht gestartet ({})", id, state);
                continue;
            };
            if let Some(miss) = m.requires.iter().find(|p| !have_program(p, &pdir)) {
                warn!("Decoder '{}': Programm '{}' fehlt", id, miss);
                self.set_state(idx, &format!("fehlt: {}", miss)).await;
                continue;
            }
            let data = self.data_dir.join("decoders").join(&id);
            let _ = std::fs::create_dir_all(&data);
            // absolut: die Plugins laufen in ihrem eigenen Ordner, ein relativer data_dir zeigte sonst dorthin
            let data = std::path::absolute(&data).unwrap_or(data);
            // virtueller Hörer im Band: Rohton in der Rate des Plugins
            let client_id = DECODER_CLIENT_BASE + idx as u64;
            let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
            {
                let dm = DemodMode::from_str(&m.input.mode).unwrap_or(DemodMode::Fm);
                let mut cm = pipe.clients.lock().await;
                cm.add(client_id, tx);
                cm.update_tune(client_id, c.freq, dm, m.input.bandwidth);
                cm.set_output_rate(client_id, m.input.rate, true);
                cm.set_agc_mode(client_id, AgcMode::Off);
            }
            info!("Decoder '{}' ({}) auf {} Hz im Band '{}', {} {} Hz, bw {}", id, m.name, c.freq, pipe.id, m.input.mode, m.input.rate, m.input.bandwidth);
            let env = Env { freq: c.freq, rate: m.input.rate, mode: m.input.mode.clone(), band: pipe.id.clone(), id: id.clone(), label: label.clone(), data: data.clone(), opts: c.options.clone() };
            let hub = self.clone();
            tokio::spawn(run_instance(hub, idx, m, pdir, env, rx));
        }
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
                { let mut g = hub.inner.lock().await; if let Some(s) = g.status.get_mut(idx) { s.restarts = restarts; } }
                let mut stdin = child.stdin.take();
                // stdout → Treffer
                if let Some(out) = child.stdout.take() {
                    let hub2 = hub.clone();
                    tokio::spawn(async move {
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
                // stderr → Log + Rest für die Statusanzeige
                if let Some(err) = child.stderr.take() {
                    let hub2 = hub.clone(); let id = env.id.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(err).lines();
                        while let Ok(Some(l)) = lines.next_line().await {
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
                            let Some(frame) = f else { return };
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
                f = rx.recv() => { if f.is_none() { return; } }
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
    let list: Vec<Value> = g.status.iter().filter(|s| p.may_decoder(&s.id, s.public)).map(|s| serde_json::to_value(s).unwrap_or(Value::Null)).collect();
    ([(header::CACHE_CONTROL, "no-store")], Json(json!({ "decoders": list, "t": now_s() })))
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
