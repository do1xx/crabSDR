//! Ton als Ogg/Opus-Stream über HTTP, wie Webradio: `/stream/<kHz>/<betriebsart>.ogg` spielt VLC, mpv, jeder Browser,
//! Home Assistant. Das Band sucht der Server anhand der Frequenz. Rechte wie im Browser (Band-Freigabe, `?token=`),
//! der Stream zählt als Hörer. Bei geschlossener Rauschsperre läuft Stille weiter, sonst bricht der Spieler ab.
//! `/stream/presets.m3u` liefert die Schnellwahl der Station als Senderliste.
//!
//! Parameter: `bw` Bandbreite Hz · `pb=lo,hi` SSB-Durchlass Hz · `sq=auto|auto:<dB>|<dBFS>|off` · `agc=fast|medium|slow|off`
//! · `name` Anzeigename · `band` Band-ID (nur nötig, wenn zwei Bänder die Frequenz abdecken) · `token`.

use crate::access;
use crate::client::{Squelch, SquelchMode};
use crate::sdr_pipeline::{corrected_center, SdrPipeline};
use crate::AppState;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use crabsdr_core::{protocol, AgcMode, DemodMode};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

/// Opus-Rahmen 20 ms; Ogg zählt immer in 48-kHz-Abtastwerten
const GRANULE_PER_PACKET: u64 = 960;
/// Rahmen je Ogg-Seite (100 ms): wenig Kopfdaten, kaum zusätzliche Verzögerung
const PACKETS_PER_PAGE: usize = 5;
/// Mehr als so viele wartende Rahmen (0,5 s) werden verworfen, sonst wächst die Verzögerung bei Netzstau
const MAX_QUEUE: usize = 25;

#[derive(Deserialize, Default)]
pub struct StreamQuery {
    pub token: Option<String>,
    pub bw: Option<u32>,
    pub pb: Option<String>,
    pub sq: Option<String>,
    pub agc: Option<String>,
    pub name: Option<String>,
    pub band: Option<String>,
}

fn err(code: StatusCode, msg: &str) -> Response { (code, msg.to_string()).into_response() }

/// Frequenz aus dem Pfad: kHz („145700“, „145700.5“) oder MHz („145.700“); Werte unter 1000 gelten als MHz
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

/// `GET /stream/<freq>/<mode>.ogg`
pub async fn ogg(
    Path((freq_s, file)): Path<(String, String)>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(stem) = file.strip_suffix(".ogg") else { return err(StatusCode::NOT_FOUND, "nur .ogg") };
    let Some(mode) = DemodMode::from_str(stem) else { return err(StatusCode::BAD_REQUEST, "Betriebsart unbekannt (fm, data, am, sam, usb, lsb, cw, wfm)") };
    let Some(freq) = parse_freq(&freq_s) else { return err(StatusCode::BAD_REQUEST, "Frequenz in kHz, z. B. 145700") };
    let Some(pipeline) = find_pipeline(&state, freq, q.band.as_deref()).await else {
        return err(StatusCode::NOT_FOUND, "Kein Band dieser Station deckt die Frequenz ab");
    };
    let tok = access::bearer(&headers).or_else(|| q.token.clone());
    let Ok(p) = access::resolve(&state, tok.as_deref()).await else { return err(StatusCode::UNAUTHORIZED, "Sitzung abgelaufen") };
    let (public, admin_only) = (pipeline.guest.load(Ordering::Relaxed), pipeline.admin_only.load(Ordering::Relaxed));
    if !p.may_band(&pipeline.id, public, admin_only) {
        return err(StatusCode::FORBIDDEN, "Band nur für angemeldete Hörer (?token=…)");
    }

    // Durchlass: SSB wie im Teilen-Link (lo,hi), sonst Bandbreite
    let (mut bandwidth, mut pass_lo) = (q.bw.unwrap_or_else(|| mode.default_bandwidth()).clamp(100, 250_000), 300u32);
    if let Some((lo, hi)) = q.pb.as_deref().and_then(|pb| pb.split_once(',')) {
        if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<u32>(), hi.trim().parse::<u32>()) {
            if hi > lo { pass_lo = lo.min(5000); bandwidth = (hi - lo).clamp(100, 20_000); }
        }
    }
    let agc = q.agc.as_deref().and_then(AgcMode::from_str).unwrap_or(AgcMode::Medium);
    let squelch = parse_squelch(q.sq.as_deref(), mode);
    let name = q.name.clone().unwrap_or_else(|| "Stream".into());

    let client_id = crate::NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(32);
    {
        let mut clients = pipeline.clients.lock().await;
        clients.add(client_id, audio_tx);
        clients.set_opus(client_id, true);
        clients.set_name(client_id, &name);
        clients.set_session(client_id, &format!("stream{client_id}"));
        clients.set_agc_mode(client_id, agc);
        clients.set_squelch(client_id, squelch);
        clients.update_tune_lo(client_id, freq, mode, bandwidth, pass_lo);
    }
    info!("[{}] Stream {} ({}): {} Hz {} bw {}", pipeline.id, client_id, name, freq, mode.as_str(), bandwidth);

    let (out_tx, out_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let pipe = pipeline.clone();
    tokio::spawn(async move {
        let mut mux = OggOpus::new(client_id as u32 ^ 0x6372_6162);
        let silence = encode_silence();
        if out_tx.send(Ok(Bytes::from(mux.headers()))).await.is_err() { pipe.clients.lock().await.remove(client_id); return; }
        let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut started = false;
        let start = tokio::time::Instant::now();
        loop {
            tokio::select! {
                frame = audio_rx.recv() => {
                    match frame {
                        Some(f) if f.first() == Some(&protocol::TAG_AUDIO_OPUS) && f.len() > 1 => {
                            if queue.len() >= MAX_QUEUE { queue.pop_front(); }
                            queue.push_back(f[1..].to_vec());
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                _ = tick.tick() => {
                    // Anlauf: erst mit zwei Rahmen Vorrat senden, damit die DSP-Stöße (3 Rahmen je 60 ms) nicht lückeln
                    if !started { if queue.len() >= 2 || start.elapsed() > Duration::from_millis(200) { started = true; } else { continue; } }
                    let pkt = queue.pop_front().unwrap_or_else(|| silence.clone());
                    mux.add(pkt);
                    if mux.pending() >= PACKETS_PER_PAGE && out_tx.send(Ok(Bytes::from(mux.flush()))).await.is_err() { break; }
                }
            }
        }
        pipe.clients.lock().await.remove(client_id);
        info!("[{}] Stream {} beendet", pipe.id, client_id);
    });

    let body = Body::from_stream(futures_util::stream::unfold(out_rx, |mut rx| async { rx.recv().await.map(|b| (b, rx)) }));
    let title = format!("crabSDR {} {} {}", pipeline.label, fmt_khz(freq), mode.as_str().to_uppercase());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/ogg")
        .header(header::CACHE_CONTROL, "no-store")
        .header("icy-name", title.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect::<String>())
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Antwort"))
}

fn fmt_khz(hz: u64) -> String {
    let khz = hz as f64 / 1000.0;
    if hz.is_multiple_of(1000) { format!("{} kHz", hz / 1000) } else { format!("{khz:.1} kHz") }
}

/// 20 ms Stille als Opus-Rahmen, wird bei geschlossener Rauschsperre wiederholt
fn encode_silence() -> Vec<u8> {
    let mut out = vec![0u8; 400];
    if let Ok(mut enc) = opus::Encoder::new(crate::dsp_thread::OPUS_RATE, opus::Channels::Mono, opus::Application::Audio) {
        let zeros = vec![0i16; (crate::dsp_thread::OPUS_RATE / 50) as usize];
        if let Ok(n) = enc.encode(&zeros, &mut out) { out.truncate(n); return out; }
    }
    vec![0xf8, 0xff, 0xfe]   // Notnagel: leerer CELT-Rahmen
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

/// Ogg-Seiten für Opus von Hand (RFC 3533 / RFC 7845): ein Strom, Kopfseiten OpusHead + OpusTags, dann Tonseiten.
struct OggOpus {
    serial: u32,
    seq: u32,
    granule: u64,
    pending: Vec<Vec<u8>>,
}

impl OggOpus {
    fn new(serial: u32) -> Self { Self { serial, seq: 0, granule: 0, pending: Vec::new() } }

    fn headers(&mut self) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"OpusHead");
        head.push(1);                                              // Version
        head.push(1);                                              // Kanäle
        head.extend_from_slice(&312u16.to_le_bytes());             // Pre-Skip (Encoder-Vorlauf)
        head.extend_from_slice(&crate::dsp_thread::OPUS_RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());               // Verstärkung
        head.push(0);                                              // Kanalabbildung
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        let vendor = b"crabSDR";
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        let mut out = self.page(0x02, 0, &[head]);
        out.extend(self.page(0x00, 0, &[tags]));
        out
    }

    fn add(&mut self, packet: Vec<u8>) { self.pending.push(packet); }
    fn pending(&self) -> usize { self.pending.len() }

    fn flush(&mut self) -> Vec<u8> {
        let packets = std::mem::take(&mut self.pending);
        self.granule += GRANULE_PER_PACKET * packets.len() as u64;
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

    /// Bekannter Prüfwert: Ogg-CRC der ersten Kopfseite eines Opus-Stroms mit Seriennummer 0 (Wert aus opusenc nachgerechnet)
    #[test]
    fn ogg_seiten_aufbau() {
        let mut m = OggOpus::new(7);
        let h = m.headers();
        assert_eq!(&h[0..4], b"OggS");
        assert_eq!(h[5], 0x02);                                   // erste Seite = Beginn des Stroms
        assert_eq!(h[26], 1);                                     // ein Segment
        assert_eq!(h[27], 19);                                    // OpusHead ist 19 Byte
        assert_eq!(&h[28..36], b"OpusHead");
        // zweite Seite folgt direkt und trägt OpusTags
        let second = 28 + 19;
        assert_eq!(&h[second..second + 4], b"OggS");
        assert_eq!(&h[second + 28..second + 36], b"OpusTags");
        // Tonseite: Granule zählt 960 je Rahmen, Lacing teilt lange Pakete in 255er
        for _ in 0..5 { m.add(vec![1u8; 300]); }
        let page = m.flush();
        assert_eq!(u64::from_le_bytes(page[6..14].try_into().unwrap()), 5 * 960);
        assert_eq!(page[26], 10);                                 // je Paket 255 + 45
        assert_eq!(page.len(), 27 + 10 + 5 * 300);
        // CRC über die Seite mit genullten CRC-Bytes muss dem eingetragenen Wert entsprechen
        let mut z = page.clone(); z[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(ogg_crc(&z).to_le_bytes(), page[22..26]);
    }

    #[test]
    fn ogg_crc_bekannter_wert() {
        // Referenz aus der Ogg-Spezifikation: CRC("123456789") mit diesem Verfahren = 0x89a1897f
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }
}
