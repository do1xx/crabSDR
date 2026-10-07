//! Ton als Stream über HTTP, wie Webradio: `/stream/<kHz>/<betriebsart>.ogg` (Opus) oder `.wav` (PCM 16 bit, 48 kHz) spielt
//! VLC, mpv, jeder Browser, Home Assistant oder ein Decoder. `/stream/pair.wav?l=<kHz>/<mode>&r=<kHz>/<mode>` legt zwei
//! Kanäle auf links und rechts. Das Band sucht der Server anhand der Frequenz. Rechte wie im Browser (Band-Freigabe,
//! `?token=`), Lastgrenzen aus der Konfiguration (max_streams, max_per_ip, max_listeners, max_channels), jeder Stream zählt
//! als Hörer. Bei geschlossener Rauschsperre läuft Stille weiter, sonst bricht der Spieler ab.
//! `/stream/presets.m3u` liefert die Schnellwahl der Station als Senderliste.
//!
//! Der DSP liefert PCM (48 kHz); Opus wird hier je Stream kodiert, damit `br` (Bitrate kbit/s) je Hörer gilt.
//! Parameter: `bw` Bandbreite Hz · `pb=lo,hi` SSB-Durchlass Hz · `sq=auto|auto:<dB>|<dBFS>|off` · `agc=fast|medium|slow|off`
//! · `br` Opus-Bitrate 8–128 · `name` Anzeigename · `band` Band-ID · `token`.

use crate::access;
use crate::client::{Squelch, SquelchMode};
use crate::sdr_pipeline::{corrected_center, SdrPipeline};
use crate::AppState;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use crabsdr_core::{protocol, AgcMode, DemodMode};
use serde::Deserialize;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

/// Abtastrate des Streams; der DSP rechnet auf diese Rate um
const RATE: u32 = 48_000;
/// 20 ms je Rahmen (Opus) bzw. je Takt (PCM)
const FRAME: usize = (RATE / 50) as usize;
/// Rahmen je Ogg-Seite bzw. Sendeblock (100 ms)
const PER_PAGE: usize = 5;
/// Mehr als 0,5 s wartender Ton wird verworfen, sonst wächst die Verzögerung bei Netzstau
const MAX_BUFFER: usize = FRAME * 25;

#[derive(Deserialize, Default)]
pub struct StreamQuery {
    pub token: Option<String>,
    pub bw: Option<u32>,
    pub pb: Option<String>,
    pub sq: Option<String>,
    pub agc: Option<String>,
    pub br: Option<u32>,
    /// nur I/Q: Abtastrate 32000 oder 48000 (.wav), Bittiefe 8 oder 16 (.wav)
    pub rate: Option<u32>,
    pub bits: Option<u16>,
    pub name: Option<String>,
    pub band: Option<String>,
    pub l: Option<String>,
    pub r: Option<String>,
}

/// Fehler als (Status, Text); erst am Ende zur Antwort gemacht (Clippy: große Err-Variante vermeiden)
type Fail = (StatusCode, String);
fn err(code: StatusCode, msg: &str) -> Fail { (code, msg.to_string()) }

/// Frequenz: kHz („145700“, „145700.5“) oder MHz („145.700“); Werte unter 1000 gelten als MHz
fn parse_freq(s: &str) -> Option<u64> {
    let v: f64 = s.trim().replace(',', ".").parse().ok()?;
    if v <= 0.0 || v.is_nan() { return None; }
    let hz = if v < 1000.0 { v * 1e6 } else { v * 1e3 };
    Some(hz.round() as u64)
}

/// Band, in dessen Ausschnitt die Frequenz liegt (oder das mit `band=` genannte)
async fn find_pipeline(state: &AppState, freq: u64, band: Option<&str>) -> Option<Arc<SdrPipeline>> {
    let manager = state.manager.read().await;
    let mut hit: Option<Arc<SdrPipeline>> = None;
    for p in manager.all().values() {
        if let Some(b) = band { if p.id == b { return Some(p.clone()); } continue; }
        let center = corrected_center(p.center_freq.load(Ordering::Relaxed), p.corr_ppm) as i64;
        let half = p.sample_rate.load(Ordering::Relaxed) as i64 / 2;
        if (freq as i64 - center).abs() <= half && hit.as_ref().is_none_or(|h| h.id > p.id) { hit = Some(p.clone()); }
    }
    hit
}

fn parse_squelch(s: Option<&str>, mode: DemodMode) -> Squelch {
    let default_auto = matches!(mode, DemodMode::Fm | DemodMode::Data | DemodMode::Wfm);
    let mut sq = Squelch { mode: if default_auto { SquelchMode::Auto } else { SquelchMode::Off }, ..Squelch::default() };
    match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None => {}
        Some("off") => sq.mode = SquelchMode::Off,
        Some("auto") => sq.mode = SquelchMode::Auto,
        Some(v) if v.starts_with("auto:") => {
            sq.mode = SquelchMode::Auto;
            if let Ok(m) = v[5..].parse::<f32>() { sq.margin_db = m.clamp(0.0, 40.0); }
        }
        Some(v) => if let Ok(db) = v.parse::<f32>() { sq.mode = SquelchMode::Manual; sq.db = db.clamp(-140.0, 0.0); },
    }
    sq
}

/// Ein abgestimmter Kanal: Frequenz, Betriebsart und die Hörer-Einstellungen aus der URL
#[derive(Clone)]
struct Spec { freq: u64, mode: DemodMode, bandwidth: u32, pass_lo: u32, squelch: Squelch, agc: AgcMode, rate: u32 }

fn spec(freq_s: &str, mode_s: &str, q: &StreamQuery) -> Result<Spec, Fail> {
    let Some(mode) = DemodMode::from_str(mode_s) else { return Err(err(StatusCode::BAD_REQUEST, "Betriebsart unbekannt (fm, data, am, sam, usb, lsb, cw, wfm)")) };
    let Some(freq) = parse_freq(freq_s) else { return Err(err(StatusCode::BAD_REQUEST, "Frequenz in kHz, z. B. 145700")) };
    let (mut bandwidth, mut pass_lo) = (q.bw.unwrap_or_else(|| mode.default_bandwidth()).clamp(100, 250_000), 300u32);
    if let Some((lo, hi)) = q.pb.as_deref().and_then(|pb| pb.split_once(',')) {
        if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<u32>(), hi.trim().parse::<u32>()) {
            if hi > lo { pass_lo = lo.min(5000); bandwidth = (hi - lo).clamp(100, 20_000); }
        }
    }
    let rate = if mode == DemodMode::Iq && q.rate == Some(32_000) { 32_000 } else { RATE };
    Ok(Spec { freq, mode, bandwidth, pass_lo, squelch: parse_squelch(q.sq.as_deref(), mode), agc: q.agc.as_deref().and_then(AgcMode::from_str).unwrap_or(AgcMode::Medium), rate })
}

/// Ein laufender Kanal: beim Band angemeldet, PCM kommt über `rx`; beim Fallenlassen wird abgemeldet
struct Tap { pipeline: Arc<SdrPipeline>, id: u64, rx: mpsc::Receiver<Vec<u8>> }

impl Drop for Tap {
    fn drop(&mut self) {
        let (p, id) = (self.pipeline.clone(), self.id);
        tokio::spawn(async move { p.clients.lock().await.remove(id); info!("[{}] Stream {} beendet", p.id, id); });
    }
}

/// Rechte und Lastgrenzen prüfen, Kanal beim Band anmelden
async fn attach(state: &AppState, headers: &HeaderMap, peer: SocketAddr, q: &StreamQuery, sp: &Spec, name: &str) -> Result<Tap, Fail> {
    let Some(pipeline) = find_pipeline(state, sp.freq, q.band.as_deref()).await else {
        return Err(err(StatusCode::NOT_FOUND, "Kein Band dieser Station deckt die Frequenz ab"));
    };
    let tok = access::bearer(headers).or_else(|| q.token.clone());
    // Fester Stream-Schlüssel aus der Konfiguration: zählt wie der Sysop, aber nur hier (nie für die Admin-Seite)
    let key = state.config.read().await.stream_key.clone();
    let p = if !key.is_empty() && key.len() >= 16 && tok.as_deref() == Some(key.as_str()) {
        access::Principal { id: "stream-key".into(), username: "Sysop".into(), role: "admin".into(), scope: "listen".into(), bands: vec![], decoders: vec![], must_change: false }
    } else {
        let Ok(p) = access::resolve(state, tok.as_deref()).await else { return Err(err(StatusCode::UNAUTHORIZED, "Sitzung abgelaufen")) };
        p
    };
    let (public, admin_only) = (pipeline.guest.load(Ordering::Relaxed), pipeline.admin_only.load(Ordering::Relaxed));
    if !p.may_band(&pipeline.id, public, admin_only) {
        return Err(err(StatusCode::FORBIDDEN, "Band nur für angemeldete Hörer (?token=…)"));
    }
    let ip = access::client_ip(headers, Some(peer));
    let (max_l, max_ip, max_ch, max_st, iq_mode, max_iq) = { let c = state.config.read().await; (c.max_listeners, c.max_per_ip, c.max_channels, c.max_streams, c.iq_stream.clone(), c.max_iq) };
    if sp.mode == DemodMode::Iq {
        let ok = match iq_mode.as_str() { "all" => true, "users" => !p.is_guest(), "admin" => p.is_admin(), _ => false };
        if iq_mode == "off" { return Err(err(StatusCode::NOT_FOUND, "I/Q-Stream ist auf dieser Station aus (iq_stream)")); }
        if !ok { return Err(err(StatusCode::FORBIDDEN, "I/Q-Stream nur mit Anmeldung (?token=…)")); }
    }
    let id = crate::NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Vec<u8>>(32);
    {
        let mut clients = pipeline.clients.lock().await;
        if max_st > 0 && clients.count_streams() >= max_st as usize { return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Zu viele Streams auf dieser Station, bitte später")); }
        if max_l > 0 && clients.count_real() >= max_l as usize { return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Station voll, bitte später")); }
        if max_ip > 0 && clients.count_ip(&ip) >= max_ip as usize { return Err(err(StatusCode::TOO_MANY_REQUESTS, "Zu viele Verbindungen von deiner Adresse")); }
        if sp.mode == DemodMode::Iq && max_iq > 0 && clients.count_iq() >= max_iq as usize { return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Schon ein I/Q-Stream aktiv (max_iq)")); }
        clients.add(id, tx);
        clients.set_origin(id, &ip, true);
        clients.set_opus(id, false);
        clients.set_output_rate(id, sp.rate, false);
        clients.set_name(id, name);
        clients.set_session(id, &format!("stream{id}"));
        clients.set_agc_mode(id, sp.agc);
        clients.set_squelch(id, sp.squelch);
        if !clients.try_tune(id, sp.freq, sp.mode, sp.bandwidth, sp.pass_lo, max_ch) {
            clients.remove(id);
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Station voll: zu viele verschiedene Kanäle in Betrieb"));
        }
    }
    info!("[{}] Stream {} ({}): {} Hz {} bw {}", pipeline.id, id, name, sp.freq, sp.mode.as_str(), sp.bandwidth);
    Ok(Tap { pipeline, id, rx })
}

/// PCM-Rahmen (Tag 0x02, s16le) in den Puffer; zu viel Rückstand wird verworfen
fn push_pcm(buf: &mut VecDeque<i16>, frame: &[u8], max: usize) {
    if frame.first() != Some(&protocol::TAG_AUDIO) || frame.len() < 3 { return; }
    for c in frame[1..].chunks_exact(2) { buf.push_back(i16::from_le_bytes([c[0], c[1]])); }
    while buf.len() > max { buf.pop_front(); }
}

/// 20 ms aus dem Puffer, sonst Stille
fn take_frame(buf: &mut VecDeque<i16>, out: &mut [i16]) {
    for s in out.iter_mut() { *s = buf.pop_front().unwrap_or(0); }
}

enum Format { Ogg, Wav }

fn format_of(file: &str) -> Option<(&str, Format)> {
    if let Some(stem) = file.strip_suffix(".ogg") { return Some((stem, Format::Ogg)); }
    if let Some(stem) = file.strip_suffix(".wav") { return Some((stem, Format::Wav)); }
    None
}

/// `GET /stream/<freq>/<mode>.ogg|.wav` – ein Kanal, mono
pub async fn ogg(
    Path((freq_s, file)): Path<(String, String)>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some((stem, format)) = format_of(&file) else { return err(StatusCode::NOT_FOUND, "nur .ogg oder .wav").into_response() };
    let mut sp = match spec(&freq_s, stem, &q) { Ok(s) => s, Err(r) => return r.into_response() };
    let iq = sp.mode == DemodMode::Iq;
    if iq && matches!(format, Format::Ogg) { sp.rate = RATE; }                          // Opus kann kein 32 kHz
    let channels: u16 = if iq { 2 } else { 1 };
    let bits: u16 = if iq && matches!(format, Format::Wav) && q.bits == Some(8) { 8 } else { 16 };
    let rate = sp.rate;
    let name = q.name.clone().unwrap_or_else(|| "Stream".into());
    let mut tap = match attach(&state, &headers, peer, &q, &sp, &name).await { Ok(t) => t, Err(r) => return r.into_response() };
    let (bitrate, station, picture) = { let c = state.config.read().await; (q.br.map(|b| b.clamp(8, if iq { 256 } else { 128 }) * 1000).unwrap_or(if iq { 160_000 } else { c.opus_bitrate }), c.station.name.clone(), station_picture(c.site_dir.as_deref())) };
    let title = format!("{} {}{} · {}", fmt_khz(sp.freq), sp.mode.as_str().to_uppercase(), if iq { format!(" {} kHz", rate / 1000) } else { String::new() }, station);
    let tags = Tags { title: title.clone(), artist: format!("{} · crabSDR", station), picture };
    let (out_tx, out_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let ctype = match format { Format::Ogg => "audio/ogg", Format::Wav => "audio/wav" };
    tokio::spawn(async move {
        let mut enc = match format { Format::Ogg => Some(match Encoder::new(bitrate, channels, tap.id as u32, tags) { Some(e) => e, None => return }), Format::Wav => None };
        let head = match &mut enc { Some(e) => e.headers(), None => wav_header(channels, rate, bits) };
        if out_tx.send(Ok(Bytes::from(head))).await.is_err() { return; }
        let per_tick = (rate / 50) as usize * channels as usize;                          // Abtastwerte je 20 ms (verschränkt)
        let max_buf = (rate / 2) as usize * channels as usize;
        let mut buf = VecDeque::with_capacity(max_buf);
        let mut frame = vec![0i16; per_tick];
        let mut block = Vec::with_capacity(per_tick * 2 * PER_PAGE);
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let (mut started, start, mut n) = (false, tokio::time::Instant::now(), 0usize);
        loop {
            tokio::select! {
                f = tap.rx.recv() => match f { Some(f) => push_pcm(&mut buf, &f, max_buf), None => break },
                _ = tick.tick() => {
                    // Anlauf mit 100 ms Vorrat, damit die DSP-Stöße (ein Rahmen je 50–60 ms) nicht lückeln
                    if !started { if buf.len() >= per_tick * 5 || start.elapsed() > Duration::from_millis(300) { started = true; } else { continue; } }
                    take_frame(&mut buf, &mut frame);
                    match &mut enc {
                        Some(e) => { e.push(&frame); }
                        None if bits == 8 => for s in &frame { block.push(((*s >> 8) + 128) as u8); },
                        None => for s in &frame { block.extend_from_slice(&s.to_le_bytes()); },
                    }
                    n += 1;
                    if n % PER_PAGE == 0 {
                        let out = match &mut enc { Some(e) => e.flush(), None => std::mem::take(&mut block) };
                        if out_tx.send(Ok(Bytes::from(out))).await.is_err() { break; }
                    }
                }
            }
        }
    });
    respond(ctype, &title, out_rx)
}

/// `GET /stream/pair.wav?l=<kHz>/<mode>&r=<kHz>/<mode>` – zwei Kanäle auf links und rechts, für Decoder am Audiokabel
pub async fn pair(
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let parse = |s: Option<&str>| -> Result<Spec, Fail> {
        let Some((f, m)) = s.and_then(|v| v.split_once('/')) else { return Err(err(StatusCode::BAD_REQUEST, "l und r als <kHz>/<betriebsart>, z. B. l=145700/fm&r=145725/fm")) };
        spec(f, m, &q)
    };
    let (sl, sr) = match (parse(q.l.as_deref()), parse(q.r.as_deref())) { (Ok(a), Ok(b)) => (a, b), (Err(r), _) | (_, Err(r)) => return r.into_response() };
    let name = q.name.clone().unwrap_or_else(|| "Stream".into());
    let mut tl = match attach(&state, &headers, peer, &q, &sl, &format!("{name} L")).await { Ok(t) => t, Err(r) => return r.into_response() };
    let mut tr = match attach(&state, &headers, peer, &q, &sr, &format!("{name} R")).await { Ok(t) => t, Err(r) => return r.into_response() };
    let title = format!("crabSDR {} {} | {} {}", fmt_khz(sl.freq), sl.mode.as_str().to_uppercase(), fmt_khz(sr.freq), sr.mode.as_str().to_uppercase());
    let (out_tx, out_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        if out_tx.send(Ok(Bytes::from(wav_header(2, RATE, 16)))).await.is_err() { return; }
        let (mut bl, mut br) = (VecDeque::with_capacity(MAX_BUFFER), VecDeque::with_capacity(MAX_BUFFER));
        let (mut fl, mut fr) = (vec![0i16; FRAME], vec![0i16; FRAME]);
        let mut block = Vec::with_capacity(FRAME * 4 * PER_PAGE);
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let (mut started, start, mut n) = (false, tokio::time::Instant::now(), 0usize);
        loop {
            tokio::select! {
                f = tl.rx.recv() => match f { Some(f) => push_pcm(&mut bl, &f, MAX_BUFFER), None => break },
                f = tr.rx.recv() => match f { Some(f) => push_pcm(&mut br, &f, MAX_BUFFER), None => break },
                _ = tick.tick() => {
                    if !started { if (bl.len() >= FRAME * 5 && br.len() >= FRAME * 5) || start.elapsed() > Duration::from_millis(300) { started = true; } else { continue; } }
                    take_frame(&mut bl, &mut fl); take_frame(&mut br, &mut fr);
                    for (a, b) in fl.iter().zip(&fr) { block.extend_from_slice(&a.to_le_bytes()); block.extend_from_slice(&b.to_le_bytes()); }
                    n += 1;
                    if n % PER_PAGE == 0 && out_tx.send(Ok(Bytes::from(std::mem::take(&mut block)))).await.is_err() { break; }
                }
            }
        }
    });
    respond("audio/wav", &title, out_rx)
}

fn respond(ctype: &'static str, title: &str, out_rx: mpsc::Receiver<Result<Bytes, std::io::Error>>) -> Response {
    let body = Body::from_stream(futures_util::stream::unfold(out_rx, |mut rx| async { rx.recv().await.map(|b| (b, rx)) }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ctype)
        .header(header::CACHE_CONTROL, "no-store")
        .header("icy-name", title.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect::<String>())
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Antwort").into_response())
}

fn fmt_khz(hz: u64) -> String {
    if hz.is_multiple_of(1000) { format!("{} kHz", hz / 1000) } else { format!("{:.1} kHz", hz as f64 / 1000.0) }
}

/// WAV-Kopf für einen endlosen Strom (Längen 0xFFFFFFFF; VLC, ffmpeg und sox spielen so lange, wie Daten kommen)
fn wav_header(channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let bytes = bits / 8;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF"); h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt "); h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());                                  // PCM (8 bit vorzeichenlos, 16 bit vorzeichenbehaftet)
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * channels as u32 * bytes as u32).to_le_bytes()); // Bytes je Sekunde
    h.extend_from_slice(&(channels * bytes).to_le_bytes());                    // Blockgröße
    h.extend_from_slice(&bits.to_le_bytes());                                  // Bit je Abtastwert
    h.extend_from_slice(b"data"); h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    h
}

/// `GET /stream/presets.m3u`: Schnellwahl der Station (presets.json aus Stationsordner oder Oberfläche) als Senderliste
pub async fn m3u(headers: HeaderMap, State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.config.read().await;
    let mut candidates = Vec::new();
    if let Some(d) = &cfg.site_dir { candidates.push(d.join("presets.json")); }
    candidates.push(cfg.frontend_dir.join("presets.json"));
    let label = cfg.station.name.clone();
    drop(cfg);
    let text = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| "[]".into());
    let presets: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
    let proto = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("localhost");
    let mut out = String::from("#EXTM3U\n");
    for p in presets {
        let (Some(freq), Some(mode)) = (p.get("freq").and_then(|v| v.as_f64()), p.get("mode").and_then(|v| v.as_str())) else { continue };
        if DemodMode::from_str(mode).is_none() { continue; }
        let name = p.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let f = if freq.fract() == 0.0 { format!("{}", freq as u64) } else { format!("{freq}") };
        out.push_str(&format!("#EXTINF:-1,{} – {} {} {}\n{proto}://{host}/stream/{f}/{mode}.ogg\n", label, name, f, mode.to_uppercase()));
    }
    ([(header::CONTENT_TYPE, "audio/x-mpegurl; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], out).into_response()
}

/// Opus-Kodierer je Stream plus Ogg-Verpackung (RFC 3533 / RFC 7845): Kopfseiten OpusHead + OpusTags, dann Tonseiten
struct Encoder { enc: opus::Encoder, mux: OggOpus, buf: Vec<u8>, channels: u16, tags: Tags }

impl Encoder {
    fn new(bitrate: u32, channels: u16, serial: u32, tags: Tags) -> Option<Self> {
        let ch = if channels == 2 { opus::Channels::Stereo } else { opus::Channels::Mono };
        let mut enc = opus::Encoder::new(RATE, ch, opus::Application::Audio).ok()?;
        let _ = enc.set_bitrate(opus::Bitrate::Bits(bitrate as i32));
        Some(Self { enc, mux: OggOpus::new(serial, channels), buf: vec![0u8; 4000], channels, tags })
    }
    fn headers(&mut self) -> Vec<u8> { self.mux.headers(&self.tags) }
    /// ein 20-ms-Rahmen (mono: FRAME Werte, stereo: 2·FRAME verschränkt)
    fn push(&mut self, pcm: &[i16]) {
        debug_assert_eq!(pcm.len(), FRAME * self.channels as usize);
        let _ = self.channels;
        if let Ok(n) = self.enc.encode(pcm, &mut self.buf) { self.mux.add(self.buf[..n].to_vec()); }
    }
    fn flush(&mut self) -> Vec<u8> { self.mux.flush() }
}

/// Metadaten für die OpusTags-Seite
#[derive(Clone, Default)]
struct Tags { title: String, artist: String, picture: Option<(String, Vec<u8>)> }

/// Stationsbild für VLC & Co.: `logo.png` oder `logo.jpg` im Stationsordner (site_dir), höchstens 512 kB
fn station_picture(site_dir: Option<&std::path::Path>) -> Option<(String, Vec<u8>)> {
    let dir = site_dir?;
    for (name, mime) in [("logo.png", "image/png"), ("logo.jpg", "image/jpeg"), ("logo.jpeg", "image/jpeg")] {
        if let Ok(data) = std::fs::read(dir.join(name)) { if !data.is_empty() && data.len() <= 512 * 1024 { return Some((mime.to_string(), data)); } }
    }
    None
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char); out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

struct OggOpus { serial: u32, seq: u32, granule: u64, channels: u16, pending: Vec<Vec<u8>> }

impl OggOpus {
    fn new(serial: u32, channels: u16) -> Self { Self { serial, seq: 0, granule: 0, channels, pending: Vec::new() } }

    fn headers(&mut self, t: &Tags) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"OpusHead");
        head.push(1);                                              // Version
        head.push(self.channels as u8);
        head.extend_from_slice(&312u16.to_le_bytes());             // Pre-Skip (Kodierer-Vorlauf)
        head.extend_from_slice(&RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());               // Verstärkung
        head.push(0);                                              // Kanalabbildung (1–2 Kanäle)
        // OpusTags: Vorbis-Kommentare TITLE/ARTIST (zeigt VLC in der Wiedergabeliste) und optional das Stationsbild als
        // METADATA_BLOCK_PICTURE (FLAC-Bildblock, base64), das VLC als Cover einblendet
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        let vendor = b"crabSDR";
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        let mut comments: Vec<String> = vec![format!("TITLE={}", t.title), format!("ARTIST={}", t.artist), "ENCODER=crabSDR".into()];
        if let Some((mime, data)) = &t.picture {
            let mut pic = Vec::with_capacity(32 + mime.len() + data.len());
            pic.extend_from_slice(&3u32.to_be_bytes());                       // Typ 3 = Titelbild
            pic.extend_from_slice(&(mime.len() as u32).to_be_bytes()); pic.extend_from_slice(mime.as_bytes());
            pic.extend_from_slice(&0u32.to_be_bytes());                       // Beschreibung leer
            for _ in 0..4 { pic.extend_from_slice(&0u32.to_be_bytes()); }     // Breite, Höhe, Farbtiefe, Farben: unbekannt
            pic.extend_from_slice(&(data.len() as u32).to_be_bytes()); pic.extend_from_slice(data);
            comments.push(format!("METADATA_BLOCK_PICTURE={}", base64(&pic)));
        }
        tags.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in &comments { tags.extend_from_slice(&(c.len() as u32).to_le_bytes()); tags.extend_from_slice(c.as_bytes()); }
        let mut out = self.page(0x02, 0, &[head]);
        out.extend(self.page(0x00, 0, &[tags]));
        out
    }

    fn add(&mut self, packet: Vec<u8>) { self.pending.push(packet); }

    fn flush(&mut self) -> Vec<u8> {
        if self.pending.is_empty() { return Vec::new(); }
        let packets = std::mem::take(&mut self.pending);
        self.granule += FRAME as u64 * packets.len() as u64;      // 48-kHz-Abtastwerte je Rahmen
        self.page(0x00, self.granule, &packets)
    }

    fn page(&mut self, header_type: u8, granule: u64, packets: &[Vec<u8>]) -> Vec<u8> {
        let mut lacing = Vec::new();
        for p in packets {
            let mut n = p.len();
            while n >= 255 { lacing.push(255u8); n -= 255; }
            lacing.push(n as u8);                                  // letzter Wert < 255 schließt das Paket ab
        }
        let mut out = Vec::with_capacity(27 + lacing.len() + packets.iter().map(|p| p.len()).sum::<usize>());
        out.extend_from_slice(b"OggS");
        out.push(0);
        out.push(header_type);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&self.serial.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);                      // CRC, kommt gleich
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        for p in packets { out.extend_from_slice(p); }
        let crc = ogg_crc(&out).to_le_bytes();
        out[22..26].copy_from_slice(&crc);
        self.seq = self.seq.wrapping_add(1);
        out
    }
}

/// CRC-32 nach Ogg: Polynom 0x04c11db7, Startwert 0, ohne Spiegelung und End-XOR
fn ogg_crc(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 { r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 }; }
            *e = r;
        }
        t
    });
    data.iter().fold(0u32, |crc, &b| (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequenz_aus_pfad() {
        assert_eq!(parse_freq("145700"), Some(145_700_000));
        assert_eq!(parse_freq("145700.5"), Some(145_700_500));
        assert_eq!(parse_freq("145.700"), Some(145_700_000));
        assert_eq!(parse_freq("438,925"), Some(438_925_000));
        assert_eq!(parse_freq("x"), None);
    }

    #[test]
    fn rauschsperre_parameter() {
        assert!(matches!(parse_squelch(None, DemodMode::Fm).mode, SquelchMode::Auto));
        assert!(matches!(parse_squelch(None, DemodMode::Usb).mode, SquelchMode::Off));
        let s = parse_squelch(Some("auto:12"), DemodMode::Usb);
        assert!(matches!(s.mode, SquelchMode::Auto) && (s.margin_db - 12.0).abs() < 0.01);
        let s = parse_squelch(Some("-85"), DemodMode::Fm);
        assert!(matches!(s.mode, SquelchMode::Manual) && (s.db + 85.0).abs() < 0.01);
        assert!(matches!(parse_squelch(Some("off"), DemodMode::Fm).mode, SquelchMode::Off));
    }

    #[test]
    fn ogg_seiten_aufbau() {
        let mut m = OggOpus::new(7, 1);
        let h = m.headers(&Tags::default());
        assert_eq!(&h[0..4], b"OggS");
        assert_eq!(h[5], 0x02);                                   // erste Seite = Beginn des Stroms
        assert_eq!(h[26], 1);                                     // ein Segment
        assert_eq!(h[27], 19);                                    // OpusHead ist 19 Byte
        assert_eq!(&h[28..36], b"OpusHead");
        let second = 28 + 19;
        assert_eq!(&h[second..second + 4], b"OggS");
        assert_eq!(&h[second + 28..second + 36], b"OpusTags");
        for _ in 0..5 { m.add(vec![1u8; 300]); }
        let page = m.flush();
        assert_eq!(u64::from_le_bytes(page[6..14].try_into().unwrap()), 5 * FRAME as u64);
        assert_eq!(page[26], 10);                                 // je Paket 255 + 45
        assert_eq!(page.len(), 27 + 10 + 5 * 300);
        let mut z = page.clone(); z[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(ogg_crc(&z).to_le_bytes(), page[22..26]);
    }

    #[test]
    fn ogg_crc_bekannter_wert() {
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }

    /// Kodierer: Stille und ein Ton ergeben gültige Opus-Pakete und Seiten mit wachsender Granule
    #[test]
    fn opus_kodierer_laeuft() {
        let mut e = Encoder::new(32_000, 1, 1, Tags { title: "Test".into(), artist: "crabSDR".into(), picture: Some(("image/png".into(), vec![1, 2, 3])) }).expect("Opus-Kodierer");
        let h = e.headers(); assert!(h.len() > 60);
        let silence = vec![0i16; FRAME];
        let tone: Vec<i16> = (0..FRAME).map(|i| ((i as f32 * 0.1).sin() * 8000.0) as i16).collect();
        for _ in 0..5 { e.push(&silence); }
        let p1 = e.flush(); assert!(p1.len() > 27, "Seite {}", p1.len());
        for _ in 0..5 { e.push(&tone); }
        let p2 = e.flush();
        assert_eq!(u64::from_le_bytes(p2[6..14].try_into().unwrap()), 10 * FRAME as u64);
        assert!(p2.len() > p1.len(), "Ton braucht mehr Bytes als Stille");
    }

    #[test]
    fn base64_bekannt() {
        assert_eq!(base64(b"Man"), "TWFu"); assert_eq!(base64(b"Ma"), "TWE="); assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn wav_kopf() {
        let h = wav_header(2, 48_000, 16);
        assert_eq!(h.len(), 44);
        assert_eq!(&h[0..4], b"RIFF"); assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 4);
    }
}
