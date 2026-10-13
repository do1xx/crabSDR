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
use crate::client::{Squelch, SquelchMode, WaterfallSub};
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
/// Vorlauf des Opus-Kodierers (Abtastwerte bei 48 kHz), die der Abspieler am Anfang jedes logischen Stroms verwirft
const PRE_SKIP: u16 = 312;

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
    /// Stationsbild in den OpusTags: 1 = mit, 0 = ohne. Ohne Angabe: mit für Abspielprogramme (VLC, mpv), ohne für Browser
    /// (User-Agent „Mozilla/…“): Chrome legt für das eingebettete PNG eine Videospur an, die nie Bilder bekommt, und spielt
    /// den Ton dann nicht ab
    pub pic: Option<u8>,
    /// Live-Titel per Ogg-Kettung (TITLE wechselt mit Pegel, Rauschsperre, Rufzeichen): 1 = an, 0 = aus. Ohne Angabe: an für
    /// Abspielprogramme, aus für Browser (die bekommen auch kein Bild)
    pub live: Option<u8>,
    /// `wf`: alle 10 s der Wasserfall um die Frequenz als Titelbild im neuen Kettenglied (nur mit Live-Titeln)
    pub cover: Option<String>,
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
struct Tap { pipeline: Arc<SdrPipeline>, id: u64, rx: mpsc::Receiver<Vec<u8>>, principal: access::Principal }

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
    Ok(Tap { pipeline, id, rx, principal: p })
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
    let browser = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).is_some_and(|ua| ua.starts_with("Mozilla/"));
    let with_pic = q.pic.map(|p| p != 0).unwrap_or(!browser);
    let (bitrate, station, picture) = { let c = state.config.read().await; (q.br.map(|b| b.clamp(8, if iq { 256 } else { 128 }) * 1000).unwrap_or(if iq { 160_000 } else { c.opus_bitrate }), c.station.name.clone(), if with_pic { station_picture(c.site_dir.as_deref()) } else { None }) };
    let title = format!("{} {}{} · {}", fmt_khz(sp.freq), sp.mode.as_str().to_uppercase(), if iq { format!(" {} kHz", rate / 1000) } else { String::new() }, station);
    // Live-Titel (Ogg-Kettung) nur für Abspielprogramme und nur beim Ton; Browser bekommen weder Kettung noch Bild
    let live_on = matches!(format, Format::Ogg) && !iq && q.live.map(|v| v != 0).unwrap_or(!browser);
    let cover_on = live_on && q.cover.as_deref() == Some("wf");
    let mut live = Live::new(sp.freq, sp.mode);
    let mut tags = Tags { title: if live_on { live.title() } else { title.clone() }, artist: format!("{} · crabSDR", station), album: station.clone(), picture, pad: if browser { 48 * 1024 } else { 0 } };
    let hub = state.decoders.clone();
    let (freq, bandwidth) = (sp.freq, sp.bandwidth);
    let (out_tx, out_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let ctype = match format { Format::Ogg => "audio/ogg", Format::Wav => "audio/wav" };
    tokio::spawn(async move {
        let mut enc = match format { Format::Ogg => Some(match Encoder::new(bitrate, channels, tap.id as u32, tags.clone()) { Some(e) => e, None => return }), Format::Wav => None };
        let head = match &mut enc { Some(e) => e.headers(), None => wav_header(channels, rate, bits) };
        if out_tx.send(Ok(Bytes::from(head))).await.is_err() { return; }
        let mut cover = if cover_on { Some(Cover::new(&tap.pipeline, freq)) } else { None };
        if let Some(c) = &cover { tap.pipeline.clients.lock().await.set_waterfall(tap.id, Some(c.sub)); }
        let now0 = tokio::time::Instant::now();
        let mut cur_title = tags.title.clone();
        // erster Titelwechsel sofort, sobald der Pegel bekannt ist; danach höchstens alle 5 s; Bild alle 10 s (erstes nach 5 s)
        let (mut last_chain, mut next_cover, mut next_call) = (now0 - LIVE_MIN_GAP, now0 + Duration::from_secs(5), now0);
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
                f = tap.rx.recv() => match f {
                    Some(f) => match f.first() {
                        Some(&protocol::TAG_AUDIO) => push_pcm(&mut buf, &f, max_buf),
                        Some(&protocol::TAG_JSON) if live_on => live.feed(&f[1..]),
                        Some(&protocol::TAG_WATERFALL) => if let Some(c) = &mut cover { c.feed(&f); },
                        _ => {}
                    },
                    None => break,
                },
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
                        let out = match &mut enc {
                            Some(e) if live_on => {
                                let now = tokio::time::Instant::now();
                                if now >= next_call { live.call = hub.recent_call(freq, &tap.principal).await; next_call = now + Duration::from_secs(2); }
                                let want = live.title();
                                let cover_due = cover.is_some() && now >= next_cover;
                                if cover_due || (want != cur_title && now >= last_chain + LIVE_MIN_GAP) {
                                    if let (true, Some(c)) = (cover_due, &mut cover) {
                                        if c.retune(&tap.pipeline, freq) { tap.pipeline.clients.lock().await.set_waterfall(tap.id, Some(c.sub)); }
                                        if let Some(jpg) = c.render(freq, bandwidth) { tags.picture = Some(("image/jpeg".into(), jpg)); }
                                        next_cover = now + Duration::from_secs(10);
                                    }
                                    tags.title = want.clone(); cur_title = want; last_chain = now;
                                    e.chain(&tags);
                                }
                                e.flush()
                            }
                            Some(e) => e.flush(),
                            None => std::mem::take(&mut block),
                        };
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

/// `GET /stream/decoder/<id>.ogg` – dekodierter Ton eines Decoders (z. B. FreeDV) als Ogg/Opus; öffentliche Decoder frei,
/// andere mit Sysop-Token oder Stream-Schlüssel. Opus kommt fertig aus dem Verteiler (24 kHz), hier nur Ogg und Stille.
pub async fn decoder(
    Path(file): Path<String>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(id) = file.strip_suffix(".ogg") else { return err(StatusCode::NOT_FOUND, "nur .ogg").into_response() };
    let Some(public) = state.decoders.is_public(id).await else { return err(StatusCode::NOT_FOUND, "Decoder gibt es nicht").into_response() };
    if !public {
        let tok = access::bearer(&headers).or_else(|| q.token.clone());
        let key = state.config.read().await.stream_key.clone();
        let admin = (!key.is_empty() && key.len() >= 16 && tok.as_deref() == Some(key.as_str()))
            || access::resolve(&state, tok.as_deref()).await.map(|p| p.is_admin()).unwrap_or(false);
        if !admin { return err(StatusCode::FORBIDDEN, "Decoder nur für den Sysop (?token=…)").into_response(); }
    }
    let client_id = crate::NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    if !state.decoders.subscribe_audio(id, client_id, tx).await { return err(StatusCode::NOT_FOUND, "Dieser Decoder liefert keinen Ton").into_response(); }
    let station = state.config.read().await.station.name.clone();
    let title = format!("{} · {}", id, station);
    let title_resp = title.clone();
    let hub = state.decoders.clone();
    let (out_tx, out_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut mux = OggOpus::new(client_id as u32 ^ 0x6465_6364, 1);
        let silence = { let mut e = crate::dsp_thread::OpusEnc::new(32_000, 5); e.as_mut().and_then(|e| e.encode(&vec![0i16; (crate::dsp_thread::OPUS_RATE / 50) as usize]).pop()).unwrap_or_else(|| vec![0xf8, 0xff, 0xfe]) };
        if out_tx.send(Ok(Bytes::from(mux.headers(&Tags { title: title.clone(), artist: format!("{station} · crabSDR"), album: station.clone(), picture: None, pad: 0 })))).await.is_err() { hub.unsubscribe_audio(client_id); return; }
        let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        loop {
            tokio::select! {
                f = rx.recv() => match f { Some(f) if f.first() == Some(&protocol::TAG_AUDIO_OPUS) && f.len() > 1 => { if queue.len() >= 25 { queue.pop_front(); } queue.push_back(f[1..].to_vec()); }, Some(_) => {}, None => break },
                _ = tick.tick() => {
                    mux.add(queue.pop_front().unwrap_or_else(|| silence.clone()));
                    if mux.pending.len() >= PER_PAGE && out_tx.send(Ok(Bytes::from(mux.flush()))).await.is_err() { break; }
                }
            }
        }
        hub.unsubscribe_audio(client_id);
    });
    respond("audio/ogg", &title_resp, out_rx)
}

/// Kürzester Abstand zweier Kettenglieder (Titelwechsel)
const LIVE_MIN_GAP: Duration = Duration::from_secs(5);

/// Live-Titel eines Kanals aus den Pegel-Meldungen des DSP-Threads (`{"type":"level","db","floor","sq"}`, 4× je s) und
/// dem jüngsten Rufzeichen eines Decoders: „145700 kHz FM · DO1XX · +18 dB“ bzw. „… · Rauschsperre zu“. Der Rauschabstand
/// steht in 6-dB-Stufen mit 2 dB Hysterese, damit der Titel nicht bei jedem Zucken des Pegels wechselt
struct Live { base: String, level: f32, floor: f32, sq: bool, known: bool, step: i32, call: Option<String> }

impl Live {
    fn new(freq: u64, mode: DemodMode) -> Self {
        Self { base: format!("{} {}", fmt_khz(freq), mode.as_str().to_uppercase()), level: -200.0, floor: -200.0, sq: true, known: false, step: 0, call: None }
    }
    /// JSON-Meldung des DSP-Threads (ohne Tag-Byte); andere Typen werden übergangen
    fn feed(&mut self, json: &[u8]) {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else { return };
        if v.get("type").and_then(|t| t.as_str()) != Some("level") { return; }
        if let Some(db) = v.get("db").and_then(|d| d.as_f64()) { self.level = db as f32; }
        if let Some(fl) = v.get("floor").and_then(|d| d.as_f64()) { self.floor = fl as f32; }
        self.sq = v.get("sq").and_then(|s| s.as_bool()).unwrap_or(true);
        self.known = true;
    }
    /// Rauschabstand in 6-dB-Stufen, Wechsel erst 2 dB jenseits der Stufengrenze
    fn snr_step(&mut self) -> i32 {
        let snr = if self.level > -150.0 && self.floor > -150.0 { (self.level - self.floor).max(0.0) } else { 0.0 };
        let (lo, hi) = (self.step as f32 - 2.0, self.step as f32 + 8.0);
        if snr < lo || snr >= hi { self.step = (snr / 6.0).floor() as i32 * 6; }
        self.step
    }
    fn title(&mut self) -> String {
        let mut t = self.base.clone();
        if let Some(c) = &self.call { t.push_str(" · "); t.push_str(c); }
        if self.known {
            if self.sq { let s = self.snr_step(); t.push_str(&format!(" · +{s} dB")); } else { t.push_str(" · Rauschsperre zu"); }
        }
        t
    }
}

/// Breite und Höhe des Wasserfall-Titelbilds; 180 Zeilen = 18 s bei 10 Zeilen/s
const COVER_W: usize = 320;
const COVER_H: usize = 180;
/// Gewünschter Ausschnitt um die Frequenz des Hörers (Hz); die Zoomstufe rundet auf eine Zweierpotenz der Abtastrate
const COVER_SPAN: u32 = 250_000;

/// Wasserfall-Titelbild: der Stream meldet sich wie ein Browser für einen Zoom-Ausschnitt an, sammelt die Zeilen
/// (absolut oder Differenz zur vorigen, wie im Protokoll) und rendert daraus alle 10 s ein JPEG mit Kanalmarkierung
struct Cover { sub: WaterfallSub, nb: usize, rate: u32, center: u64, prev: Option<Vec<u8>>, lines: VecDeque<Vec<u8>> }

impl Cover {
    fn new(p: &SdrPipeline, freq: u64) -> Self {
        let (nb, rate, center) = Self::band(p);
        Self { sub: cover_sub(nb, rate, center, freq), nb, rate, center, prev: None, lines: VecDeque::with_capacity(COVER_H + 1) }
    }
    fn band(p: &SdrPipeline) -> (usize, u32, u64) {
        (p.fft_size, p.sample_rate.load(Ordering::Relaxed), corrected_center(p.center_freq.load(Ordering::Relaxed), p.corr_ppm))
    }
    /// Band umgestimmt oder Abtastrate geändert → neuer Ausschnitt (Aufrufer meldet ihn an), alte Zeilen weg
    fn retune(&mut self, p: &SdrPipeline, freq: u64) -> bool {
        let (nb, rate, center) = Self::band(p);
        let sub = cover_sub(nb, rate, center, freq);
        if (nb, rate, center, sub) == (self.nb, self.rate, self.center, self.sub) { return false; }
        *self = Self { sub, nb, rate, center, prev: None, lines: VecDeque::with_capacity(COVER_H + 1) };
        true
    }
    fn feed(&mut self, frame: &[u8]) {
        let Some((zoom, start, delta, line)) = protocol::decode_waterfall(frame) else { return };
        if zoom != self.sub.zoom || start != self.sub.start_bin || line.len() != crate::dsp_thread::WF_PX { return; }
        let line = if delta {
            let Some(prev) = &self.prev else { return };
            prev.iter().zip(&line).map(|(&p, &d)| if d == 0 { p } else { (p as i16 + (d as i8) as i16).clamp(0, 255) as u8 }).collect()
        } else { line };
        self.prev = Some(line.clone());
        self.lines.push_back(line);
        if self.lines.len() > COVER_H { self.lines.pop_front(); }
    }
    /// Spalte (0 … COVER_W) einer Frequenz im Ausschnitt
    fn column(&self, hz: u64) -> i64 {
        let bin_hz = self.rate as f64 / self.nb as f64;
        let bin = (hz as f64 - self.center as f64) / bin_hz + self.nb as f64 / 2.0;
        let bpp = ((self.nb / crate::dsp_thread::WF_PX) >> self.sub.zoom).max(1) as f64;
        ((bin - self.sub.start_bin as f64) / bpp * COVER_W as f64 / crate::dsp_thread::WF_PX as f64).round() as i64
    }
    /// JPEG COVER_W × COVER_H, neueste Zeile oben, Farbzuordnung je Zeile wie im Browser; der Kanal (Frequenz ± Bandbreite/2)
    /// bekommt einen roten Balken am oberen Rand und zwei Kantenlinien
    fn render(&self, freq: u64, bandwidth: u32) -> Option<Vec<u8>> {
        if self.lines.is_empty() { return None; }
        let pal = crate::dsp_thread::palette();
        let mut rgb = vec![0u8; COVER_W * COVER_H * 3];
        let mut row = vec![0u8; COVER_W];
        for (r, line) in self.lines.iter().rev().take(COVER_H).enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                let (a, b) = (x * crate::dsp_thread::WF_PX / COVER_W, (x + 1) * crate::dsp_thread::WF_PX / COVER_W);
                *v = *line[a..b].iter().max().unwrap_or(&0);
            }
            let mut sample: Vec<u8> = row.iter().step_by(4).cloned().collect();
            sample.sort_unstable();
            // Fenster wie der Browser-Look „Kontrast“ (28 dB über dem Rauschboden): auf dem kleinen Bild bleibt so auch ein
            // leises Band lesbar, starke Signale laufen in Weiß
            let lo0 = sample[sample.len() / 5] as f32 - 4.0;
            for (x, &v) in row.iter().enumerate() {
                let t = (((v as f32 - lo0) / 28.0 * 255.0).round()).clamp(0.0, 255.0) as usize;
                let o = (r * COVER_W + x) * 3; rgb[o..o + 3].copy_from_slice(&pal[t]);
            }
        }
        let (xl, xr) = (self.column(freq.saturating_sub(bandwidth as u64 / 2)), self.column(freq + bandwidth as u64 / 2));
        let (xl, xr) = (xl.clamp(0, COVER_W as i64 - 1) as usize, xr.clamp(0, COVER_W as i64 - 1) as usize);
        for x in xl..=xr { for r in 0..3 { let o = (r * COVER_W + x) * 3; rgb[o..o + 3].copy_from_slice(&[255, 70, 70]); } }
        for x in [xl, xr] { for r in 3..COVER_H { let o = (r * COVER_W + x) * 3; for (k, c) in [255u8, 70, 70].iter().enumerate() { rgb[o + k] = ((rgb[o + k] as u16 + *c as u16) / 2) as u8; } } }
        let mut out = Vec::with_capacity(24 * 1024);
        let enc = jpeg_encoder::Encoder::new(&mut out, 60);
        enc.encode(&rgb, COVER_W as u16, COVER_H as u16, jpeg_encoder::ColorType::Rgb).ok()?;
        Some(out)
    }
}

/// Zoom-Ausschnitt für das Titelbild: größte Zoomstufe, deren Breite (Abtastrate ≫ Stufe) noch COVER_SPAN deckt, höchstens so
/// weit, wie der Server ohne eigene Zoom-FFT rechnet (1024 Pixel ≤ 1 Bin je Pixel); mittig um die Frequenz, am Bandrand angeschlagen
fn cover_sub(nb: usize, rate: u32, center: u64, freq: u64) -> WaterfallSub {
    let srv_max = (nb / crate::dsp_thread::WF_PX).max(1).ilog2();
    let mut zoom = 0u32;
    while zoom < srv_max && (rate >> (zoom + 1)) >= COVER_SPAN { zoom += 1; }
    let w = (nb >> zoom) as i64;
    let bin_hz = rate as f64 / nb as f64;
    let center_bin = ((freq as f64 - center as f64) / bin_hz).round() as i64 + nb as i64 / 2;
    let start = (center_bin - w / 2).clamp(0, nb as i64 - w);
    WaterfallSub { zoom: zoom as u8, start_bin: start as u16 }
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

/// Opus-Kodierer je Stream plus Ogg-Verpackung (RFC 3533 / RFC 7845): Kopfseiten OpusHead + OpusTags, dann Tonseiten.
/// Die Kettung (neuer logischer Strom mit neuen Tags) geschieht mit dem nächsten Rahmen, siehe `do_chain`
struct Encoder {
    enc: OpusRaw, mux: OggOpus, buf: Vec<u8>, channels: usize, tags: Tags,
    /// noch nicht kodierte Abtastwerte (verschränkt); nach einer Kettung bleibt bis zu ein Rahmen liegen
    fifo: VecDeque<i16>,
    /// zuletzt kodierter Rahmen – zum Anwärmen des frischen Kodierers nach der Kettung
    last: Vec<i16>,
    /// gewünschte Kettung, ausgeführt beim nächsten `push`
    chain_req: Option<Tags>,
    /// fertige Seiten (EOS, neue Köpfe), die der nächste `flush` voranstellt
    out: Vec<u8>,
    /// der erste Rahmen des neuen Stroms wird ohne Vorhersage (intra) kodiert, damit ein frischer Dekodierer ihn sauber bekommt
    fresh: bool,
}

impl Encoder {
    fn new(bitrate: u32, channels: u16, serial: u32, tags: Tags) -> Option<Self> {
        let enc = OpusRaw::new(channels as i32, bitrate as i32)?;
        Some(Self { enc, mux: OggOpus::new(serial, channels), buf: vec![0u8; 4000], channels: channels as usize, tags, fifo: VecDeque::with_capacity(FRAME * 3 * channels as usize), last: Vec::new(), chain_req: None, out: Vec::new(), fresh: false })
    }
    fn headers(&mut self) -> Vec<u8> { self.mux.headers(&self.tags) }
    /// ein 20-ms-Rahmen (mono: FRAME Werte, stereo: 2·FRAME verschränkt)
    fn push(&mut self, pcm: &[i16]) {
        debug_assert_eq!(pcm.len(), FRAME * self.channels);
        self.fifo.extend(pcm);
        if let Some(t) = self.chain_req.take() { self.do_chain(t); }
        let n = FRAME * self.channels;
        while self.fifo.len() >= n {
            let frame: Vec<i16> = self.fifo.drain(..n).collect();
            self.encode(&frame, FRAME as u64);
            self.last = frame;
        }
    }
    fn encode(&mut self, pcm: &[i16], samples: u64) {
        if self.fresh { self.enc.prediction_disabled(true); }
        if let Some(n) = self.enc.encode(pcm, &mut self.buf) { self.mux.add_samples(self.buf[..n].to_vec(), samples); }
        if self.fresh { self.enc.prediction_disabled(false); self.fresh = false; }
    }
    /// Kettung anfordern (neue Tags); das Browser-Polster gehört nur in den ersten Kopf
    fn chain(&mut self, tags: &Tags) { self.chain_req = Some(Tags { pad: 0, ..tags.clone() }); }
    /// Nahtstelle ohne Verlust und ohne Sprung: der alte Kodierer bekommt noch einen ganzen Rahmen (960 Werte) und gibt
    /// damit seine Verzögerung (PRE_SKIP = 312 Werte) plus 648 neue Werte aus; der alte Strom endet damit lückenlos. Die
    /// 312 Werte, die in seiner Verzögerung stecken bleiben, wandern zurück in den Vorrat. Der frische Kodierer wird mit
    /// dem letzten Rahmen vor der Naht angewärmt (Paket verworfen): seine Verzögerung enthält dann genau die Werte, die der
    /// Pre-Skip des neuen Stroms verwirft, und die Ausgabe des Abspielers setzt beim nächsten Wert fort – für einen
    /// frischen Dekodierer (VLC) dank intra-kodiertem erstem Rahmen, für einen weiterlaufenden (ffmpeg/mpv) dank
    /// Pre-Skip-Schnitt je Strom. (Ein kürzerer 10-ms-Nachlauf wäre sparsamer, aber libopus liefert beim Wechsel der
    /// Rahmenlänge 20 → 10 ms hörbaren Müll, gemessen am 50-Hz-Ton: Fehler −8 dB statt −38 dB)
    fn do_chain(&mut self, tags: Tags) {
        let (ch, n) = (self.channels, FRAME * self.channels);
        let skip = PRE_SKIP as usize * ch;
        if self.fifo.len() < n { self.chain_req = Some(tags); return; }
        let tail: Vec<i16> = self.fifo.drain(..n).collect();
        self.encode(&tail, FRAME as u64);
        for v in tail[n - skip..].iter().rev() { self.fifo.push_front(*v); }
        self.enc.reset();
        if self.last.len() == n {
            let mut prime = self.last[n - skip..].to_vec();
            prime.extend_from_slice(&tail[..n - skip]);
            let _ = self.enc.encode(&prime, &mut self.buf);
        }
        self.fresh = true;
        self.tags = tags;
        let bytes = self.mux.chain(&self.tags);
        self.out.extend(bytes);
    }
    fn flush(&mut self) -> Vec<u8> {
        let mut o = std::mem::take(&mut self.out);
        o.extend(self.mux.flush());
        o
    }
}

/// libopus-Kodierer direkt über die C-Schnittstelle: die Hülle `opus` kennt `OPUS_SET_PREDICTION_DISABLED` nicht, das aber
/// braucht der erste Rahmen nach einer Kettung
struct OpusRaw { st: *mut audiopus_sys::OpusEncoder, channels: i32 }
unsafe impl Send for OpusRaw {}

impl OpusRaw {
    fn new(channels: i32, bitrate: i32) -> Option<Self> {
        let mut err = 0i32;
        // SAFETY: Argumente gültig (48 kHz, 1–2 Kanäle); Zeiger wird nur hier gehalten und in Drop freigegeben
        let st = unsafe { audiopus_sys::opus_encoder_create(RATE as i32, channels, audiopus_sys::OPUS_APPLICATION_AUDIO, &mut err) };
        if st.is_null() || err != audiopus_sys::OPUS_OK { return None; }
        let e = Self { st, channels };
        unsafe { audiopus_sys::opus_encoder_ctl(e.st, audiopus_sys::OPUS_SET_BITRATE_REQUEST, bitrate); }
        Some(e)
    }
    /// ein Rahmen (verschränkt) → Opus-Paket in `out`; None bei Fehler
    fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> Option<usize> {
        let frames = pcm.len() as i32 / self.channels;
        // SAFETY: Puffer und Längen stammen aus den Slices
        let n = unsafe { audiopus_sys::opus_encode(self.st, pcm.as_ptr(), frames, out.as_mut_ptr(), out.len() as i32) };
        (n > 0).then_some(n as usize)
    }
    fn reset(&mut self) { unsafe { audiopus_sys::opus_encoder_ctl(self.st, audiopus_sys::OPUS_RESET_STATE); } }
    /// an: nächster Rahmen ohne Vorhersage aus dem Vorgänger (CELT intra, SILK unabhängig)
    fn prediction_disabled(&mut self, on: bool) { unsafe { audiopus_sys::opus_encoder_ctl(self.st, audiopus_sys::OPUS_SET_PREDICTION_DISABLED_REQUEST, on as i32); } }
}

impl Drop for OpusRaw {
    fn drop(&mut self) { unsafe { audiopus_sys::opus_encoder_destroy(self.st); } }
}

/// Metadaten für die OpusTags-Seite
#[derive(Clone, Default)]
struct Tags { title: String, artist: String, album: String, picture: Option<(String, Vec<u8>)>, pad: usize }

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

struct OggOpus { serial: u32, seq: u32, granule: u64, channels: u16, pending: Vec<Vec<u8>>, pending_samples: u64 }

impl OggOpus {
    fn new(serial: u32, channels: u16) -> Self { Self { serial, seq: 0, granule: 0, channels, pending: Vec::new(), pending_samples: 0 } }

    fn headers(&mut self, t: &Tags) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"OpusHead");
        head.push(1);                                              // Version
        head.push(self.channels as u8);
        head.extend_from_slice(&PRE_SKIP.to_le_bytes());           // Pre-Skip (Kodierer-Vorlauf)
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
        if !t.album.is_empty() { comments.push(format!("ALBUM={}", t.album)); }
        if let Some((mime, data)) = &t.picture {
            let mut pic = Vec::with_capacity(32 + mime.len() + data.len());
            pic.extend_from_slice(&3u32.to_be_bytes());                       // Typ 3 = Titelbild
            pic.extend_from_slice(&(mime.len() as u32).to_be_bytes()); pic.extend_from_slice(mime.as_bytes());
            pic.extend_from_slice(&0u32.to_be_bytes());                       // Beschreibung leer
            for _ in 0..4 { pic.extend_from_slice(&0u32.to_be_bytes()); }     // Breite, Höhe, Farbtiefe, Farben: unbekannt
            pic.extend_from_slice(&(data.len() as u32).to_be_bytes()); pic.extend_from_slice(data);
            comments.push(format!("METADATA_BLOCK_PICTURE={}", base64(&pic)));
        }
        // Polster für Browser: Chrome prüft den Strom erst, wenn einige zehn Kilobyte angekommen sind – bei einem leisen
        // Kanal (Opus-Ruhepakete, ~1,5 kB/s) dauerte das eine halbe Minute „Laden“. Ein langer Kommentar füllt den Puffer sofort
        if t.pad > 0 { comments.push(format!("PADDING={}", " ".repeat(t.pad))); }
        tags.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in &comments { tags.extend_from_slice(&(c.len() as u32).to_le_bytes()); tags.extend_from_slice(c.as_bytes()); }
        let mut out = self.page(0x02, 0, &[head]);
        out.extend(self.page(0x00, 0, &[tags]));
        out
    }

    /// ein 20-ms-Paket
    fn add(&mut self, packet: Vec<u8>) { self.add_samples(packet, FRAME as u64); }
    /// Paket mit so vielen gültigen Abtastwerten (48 kHz) – kürzere Rahmen oder ein getrimmter Nachlauf
    fn add_samples(&mut self, packet: Vec<u8>, samples: u64) { self.pending.push(packet); self.pending_samples += samples; }

    fn flush(&mut self) -> Vec<u8> { self.flush_as(false) }

    /// Wartende Pakete als Seite; `eos` schließt den logischen Strom (Kopf-Bit 0x04, notfalls als leere Seite)
    fn flush_as(&mut self, eos: bool) -> Vec<u8> {
        if self.pending.is_empty() && !eos { return Vec::new(); }
        let packets = std::mem::take(&mut self.pending);
        self.granule += std::mem::take(&mut self.pending_samples); // 48-kHz-Abtastwerte
        self.page(if eos { 0x04 } else { 0x00 }, self.granule, &packets)
    }

    /// Kettung (RFC 3533 „chained“, wie Icecast beim Titelwechsel): laufenden logischen Strom mit EOS beenden, neuen mit
    /// neuer Seriennummer, OpusHead/OpusTags, Seitenzähler 0 und Granule 0 beginnen. VLC und mpv übernehmen die neuen
    /// Tags als laufenden Titel. (Die Granule weiterlaufen zu lassen schien verlockend, aber VLC rechnet je Glied von 0
    /// und verwarf dann Rahmen: „Could not convert timestamp“)
    fn chain(&mut self, t: &Tags) -> Vec<u8> {
        let mut out = self.flush_as(true);
        self.serial = self.serial.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15) | 1;
        self.seq = 0;
        self.granule = 0;
        out.extend(self.headers(t));
        out
    }

    fn page(&mut self, header_type: u8, granule: u64, packets: &[Vec<u8>]) -> Vec<u8> {
        // Lacing: je Paket 255er-Werte und ein Abschluss < 255. Mehr als 255 Segmente passen nicht auf eine Seite (das
        // Zählbyte ist 8 Bit) – dann wird auf mehrere Seiten verteilt, Folgeseiten tragen das Fortsetzungs-Flag, wenn ein
        // Paket über die Seitengrenze läuft, und Granule −1, wenn auf ihnen kein Paket endet (Ogg-Regel). Vorher lief ein
        // OpusTags-Kopf über 65 kB (Polster für Browser) in eine kaputte Seite mit falscher Prüfsumme (13.10.)
        let mut lacing = Vec::new();
        for p in packets {
            let mut n = p.len();
            while n >= 255 { lacing.push(255u8); n -= 255; }
            lacing.push(n as u8);                                  // letzter Wert < 255 schließt das Paket ab
        }
        let data: Vec<u8> = packets.concat();
        let mut out = Vec::with_capacity(27 + lacing.len() + data.len() + 27 * (lacing.len() / 255));
        let (mut li, mut di, mut continued) = (0usize, 0usize, false);
        loop {
            let take = (lacing.len() - li).min(255);
            let seg = &lacing[li..li + take];
            let bytes: usize = seg.iter().map(|&v| v as usize).sum();
            let last = li + take >= lacing.len();
            let ends_packet = seg.last().is_some_and(|&v| v < 255);
            let g = if last || ends_packet { granule } else { u64::MAX };
            let start = out.len();
            out.extend_from_slice(b"OggS");
            out.push(0);
            out.push(header_type | if continued { 0x01 } else { 0 });
            out.extend_from_slice(&g.to_le_bytes());
            out.extend_from_slice(&self.serial.to_le_bytes());
            out.extend_from_slice(&self.seq.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);                  // CRC, kommt gleich
            out.push(take as u8);
            out.extend_from_slice(seg);
            out.extend_from_slice(&data[di..di + bytes]);
            let crc = ogg_crc(&out[start..]).to_le_bytes();
            out[start + 22..start + 26].copy_from_slice(&crc);
            self.seq = self.seq.wrapping_add(1);
            li += take; di += bytes; continued = !ends_packet;
            if last { break; }
        }
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
    fn ogg_grosser_kopf_auf_mehrere_seiten() {
        // OpusTags mit 70 kB Polster: mehr als 255 Segmente → mehrere Seiten, alle mit gültiger Prüfsumme, Fortsetzungs-Flag
        // auf den Folgeseiten, Granule −1 ohne Paketende, Seitenzähler fortlaufend
        let mut m = OggOpus::new(9, 1);
        let out = m.headers(&Tags { title: "T".into(), artist: "A".into(), album: String::new(), picture: None, pad: 70 * 1024 });
        let (mut i, mut pages) = (0usize, Vec::new());
        while i < out.len() {
            assert_eq!(&out[i..i + 4], b"OggS");
            let segs = out[i + 26] as usize; let body: usize = out[i + 27..i + 27 + segs].iter().map(|&v| v as usize).sum();
            let end = i + 27 + segs + body;
            let mut copy = out[i..end].to_vec(); copy[22..26].copy_from_slice(&[0, 0, 0, 0]);
            assert_eq!(ogg_crc(&copy).to_le_bytes(), out[i + 22..i + 26], "Prüfsumme Seite {}", pages.len());
            pages.push((out[i + 5], u64::from_le_bytes(out[i + 6..i + 14].try_into().unwrap()), u32::from_le_bytes(out[i + 18..i + 22].try_into().unwrap()), segs));
            i = end;
        }
        assert!(pages.len() >= 3, "{:?}", pages);   // Kopf + 2 Tag-Seiten (255 × 255 B passen auf die erste)
        assert_eq!(pages[0], (2, 0, 0, 1));                                  // OpusHead: BOS, ein Segment
        assert_eq!((pages[1].0, pages[1].1, pages[1].3), (0, u64::MAX, 255));  // Tags, erste Seite: voll, kein Paketende
        assert_eq!(pages[2].0 & 1, 1);                                       // Folgeseite: Fortsetzung
        assert_eq!(pages.last().unwrap().1, 0);                              // letzte Kopfseite: Granule 0
        for (k, p) in pages.iter().enumerate() { assert_eq!(p.2 as usize, k); }
    }

    /// Kettung: EOS-Seite schließt den ersten Strom, danach BOS mit neuer Seriennummer, Seitenzähler 0, Granule läuft weiter
    #[test]
    fn ogg_kettung_neuer_strom() {
        let mut m = OggOpus::new(7, 1);
        let _ = m.headers(&Tags { title: "eins".into(), ..Tags::default() });
        for _ in 0..5 { m.add(vec![1u8; 100]); }
        let _ = m.flush();
        for _ in 0..5 { m.add(vec![2u8; 100]); }
        let out = m.chain(&Tags { title: "zwei".into(), ..Tags::default() });
        // 1. Seite: letzte Tonseite des alten Stroms mit EOS
        assert_eq!(&out[0..4], b"OggS");
        assert_eq!(out[5], 0x04, "EOS-Bit");
        assert_eq!(u64::from_le_bytes(out[6..14].try_into().unwrap()), 10 * FRAME as u64);
        assert_eq!(u32::from_le_bytes(out[14..18].try_into().unwrap()), 7, "alte Seriennummer");
        assert_eq!(u32::from_le_bytes(out[18..22].try_into().unwrap()), 3, "Seitenzähler 3 (Kopf, Tags, Ton, EOS)");
        assert_eq!(out[26], 5);
        let eos_len = 27 + 5 + 500;
        // 2. Seite: OpusHead des neuen Stroms, BOS, Seriennummer neu, Zähler 0, Granule 0
        let h = &out[eos_len..];
        assert_eq!(&h[0..4], b"OggS");
        assert_eq!(h[5], 0x02, "BOS-Bit");
        assert_eq!(u64::from_le_bytes(h[6..14].try_into().unwrap()), 0);
        let serial = u32::from_le_bytes(h[14..18].try_into().unwrap());
        assert_ne!(serial, 7); assert_eq!(serial, m.serial);
        assert_eq!(u32::from_le_bytes(h[18..22].try_into().unwrap()), 0);
        assert_eq!(&h[28..36], b"OpusHead");
        // 3. Seite: OpusTags mit neuem Titel
        let t = &h[28 + 19..];
        assert_eq!(&t[0..4], b"OggS"); assert_eq!(t[5], 0x00); assert_eq!(&t[28..36], b"OpusTags");
        assert!(t.windows(10).any(|w| w == b"TITLE=zwei"));
        assert_eq!(u32::from_le_bytes(t[14..18].try_into().unwrap()), serial);
        assert_eq!(u32::from_le_bytes(t[18..22].try_into().unwrap()), 1);
        // Ton danach: Granule beginnt im neuen Strom bei 0 (5 Rahmen), Zähler 2, Prüfsumme stimmt
        for _ in 0..5 { m.add(vec![3u8; 100]); }
        let page = m.flush();
        assert_eq!(u64::from_le_bytes(page[6..14].try_into().unwrap()), 5 * FRAME as u64);
        assert_eq!(u32::from_le_bytes(page[18..22].try_into().unwrap()), 2);
        let mut z = page.clone(); z[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(ogg_crc(&z).to_le_bytes(), page[22..26]);
        // Kettung ohne wartende Pakete: leere EOS-Seite
        let out = m.chain(&Tags::default());
        assert_eq!(out[5], 0x04); assert_eq!(out[26], 0);
    }

    /// Live-Titel: Rauschsperre, 6-dB-Stufen mit Hysterese, Rufzeichen
    #[test]
    fn live_titel() {
        let mut l = Live::new(145_700_000, DemodMode::Fm);
        assert_eq!(l.title(), "145700 kHz FM");
        l.feed(br#"{"type":"level","db":-80.0,"floor":-100.0,"sq":false}"#);
        assert_eq!(l.title(), "145700 kHz FM · Rauschsperre zu");
        l.feed(br#"{"type":"level","db":-80.0,"floor":-100.0,"sq":true}"#);
        assert_eq!(l.title(), "145700 kHz FM · +18 dB");
        l.feed(br#"{"type":"level","db":-77.0,"floor":-100.0,"sq":true}"#);   // 23 dB: noch in der Stufe 18 (bis < 26)
        assert_eq!(l.title(), "145700 kHz FM · +18 dB");
        l.feed(br#"{"type":"level","db":-73.0,"floor":-100.0,"sq":true}"#);   // 27 dB → 24
        assert_eq!(l.title(), "145700 kHz FM · +24 dB");
        l.feed(br#"{"type":"level","db":-77.0,"floor":-100.0,"sq":true}"#);   // 23 dB: bleibt 24 (Hysterese bis < 22)
        assert_eq!(l.title(), "145700 kHz FM · +24 dB");
        l.feed(br#"{"type":"squelch","x":1}"#);                              // anderer Typ: keine Änderung
        l.call = Some("DO1XX".into());
        assert_eq!(l.title(), "145700 kHz FM · DO1XX · +24 dB");
    }

    /// Wasserfall-Titelbild: Ausschnitt um die Frequenz, Zeilen (absolut + Delta) → JPEG unter 40 kB
    #[test]
    fn wasserfall_cover() {
        let (nb, rate, center) = (4096usize, 2_048_000u32, 145_000_000u64);
        let sub = cover_sub(nb, rate, center, 145_700_000);
        assert_eq!(sub.zoom, 2, "2,048 MHz ≫ 2 = 512 kHz ≥ 250 kHz, Stufe 3 wäre über der Bin-Grenze");
        let bin_hz = rate as f64 / nb as f64;
        let w = nb >> 2;
        let center_bin = ((145_700_000.0 - 145_000_000.0) / bin_hz).round() as usize + nb / 2;
        assert_eq!(sub.start_bin as usize, center_bin - w / 2);
        assert_eq!(cover_sub(nb, rate, center, 144_000_000).start_bin, 0, "am Bandrand angeschlagen");
        let mut c = Cover { sub, nb, rate, center, prev: None, lines: VecDeque::new() };
        assert_eq!(c.column(145_700_000), COVER_W as i64 / 2);
        assert!(c.render(145_700_000, 12_500).is_none(), "ohne Zeilen kein Bild");
        let line: Vec<u8> = (0..crate::dsp_thread::WF_PX).map(|i| 40 + (i % 7) as u8 + if (500..524).contains(&i) { 30 } else { 0 }).collect();
        c.feed(&protocol::encode_waterfall(sub.zoom, sub.start_bin, false, &line));
        let delta: Vec<u8> = (0..crate::dsp_thread::WF_PX).map(|i| if i == 512 { 10 } else { 0 }).collect();
        c.feed(&protocol::encode_waterfall(sub.zoom, sub.start_bin, true, &delta));
        let zero = vec![0u8; crate::dsp_thread::WF_PX];
        for _ in 0..200 { c.feed(&protocol::encode_waterfall(sub.zoom, sub.start_bin, true, &zero)); }
        assert_eq!(c.lines.len(), COVER_H);
        assert_eq!(c.lines.back().unwrap()[512], line[512] + 10, "Delta auf die vorige Zeile, Null = unverändert");
        c.feed(&protocol::encode_waterfall(sub.zoom + 1, sub.start_bin, false, &line));   // fremder Ausschnitt: ignoriert
        assert_eq!(c.lines.len(), COVER_H);
        let t0 = std::time::Instant::now();
        let jpg = c.render(145_700_000, 12_500).expect("JPEG");
        let dt = t0.elapsed();
        assert_eq!(&jpg[0..2], &[0xFF, 0xD8]);
        assert!(jpg.len() <= 40 * 1024, "Bild {} Byte", jpg.len());
        eprintln!("Cover: {} Byte in {:?}", jpg.len(), dt);
    }

    /// Kodierer-Kettung: der Wechsel geschieht mit dem nächsten Rahmen – EOS-Seite mit dem Nachlaufrahmen (Granule 6 Rahmen),
    /// neue Köpfe, und der neue Strom nimmt die 312 Werte aus der Verzögerung mit (kein Abtastwert geht verloren)
    #[test]
    fn kette_nachlauf() {
        let mut e = Encoder::new(32_000, 1, 1, Tags::default()).expect("Opus-Kodierer");
        let _ = e.headers();
        for _ in 0..5 { e.push(&vec![100i16; FRAME]); }
        let _ = e.flush();
        e.chain(&Tags { title: "neu".into(), ..Tags::default() });
        assert!(e.flush().is_empty(), "noch nichts: der Nachlauf braucht den nächsten Rahmen");
        e.push(&vec![100i16; FRAME]);
        let out = e.flush();
        assert_eq!(out[5], 0x04, "EOS-Seite zuerst");
        assert_eq!(u64::from_le_bytes(out[6..14].try_into().unwrap()), 6 * FRAME as u64);
        assert_eq!(out[26], 1, "ein Paket: der Nachlauf");
        let eos_len = 27 + 1 + out[27] as usize;
        assert_eq!(&out[eos_len + 28..eos_len + 36], b"OpusHead");
        assert!(out.windows(9).any(|w| w == b"TITLE=neu"));
        assert_eq!(e.fifo.len(), PRE_SKIP as usize, "Übertrag aus der Verzögerung des alten Kodierers");
        assert_eq!(e.mux.granule, 0, "neuer Strom beginnt bei 0");
        e.push(&vec![100i16; FRAME]);
        let page = e.flush();
        assert_eq!(u64::from_le_bytes(page[6..14].try_into().unwrap()), FRAME as u64, "erster Tonrahmen des neuen Stroms");
        assert!(!e.fresh);
    }

    /// Kettung mit echtem Ton: 50-Hz-Sinus, alle 1 s ein neues Glied – mit CRAB_CHAIN_OUT=<datei> als Ogg geschrieben,
    /// zum Abhören und zur Prüfung auf Knacken/Lücken (ffmpeg → WAV → Sprünge zwischen Abtastwerten)
    #[test]
    fn kette_sinus() {
        let mut e = Encoder::new(32_000, 1, 1, Tags { title: "Glied 0".into(), ..Tags::default() }).expect("Opus-Kodierer");
        let mut out = e.headers();
        for i in 0..250 {                                             // 5 s
            let f: Vec<i16> = (0..FRAME).map(|k| ((2.0 * std::f64::consts::PI * 50.0 * (i * FRAME + k) as f64 / RATE as f64).sin() * 12000.0) as i16).collect();
            e.push(&f);
            if i % 50 == 49 { e.chain(&Tags { title: format!("Glied {}", (i + 1) / 50), ..Tags::default() }); }
            if i % 5 == 4 { out.extend(e.flush()); }
        }
        assert!(out.len() > 5000);
        if let Ok(p) = std::env::var("CRAB_CHAIN_OUT") { std::fs::write(p, &out).expect("schreiben"); }
    }

    #[test]
    fn ogg_crc_bekannter_wert() {
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }

    /// Kodierer: Stille und ein Ton ergeben gültige Opus-Pakete und Seiten mit wachsender Granule
    #[test]
    fn opus_kodierer_laeuft() {
        let mut e = Encoder::new(32_000, 1, 1, Tags { title: "Test".into(), artist: "crabSDR".into(), album: String::new(), picture: Some(("image/png".into(), vec![1, 2, 3])), pad: 0 }).expect("Opus-Kodierer");
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
