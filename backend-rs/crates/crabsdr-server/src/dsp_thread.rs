//! DSP-Thread je Band: ein FFT-Durchlauf je Rahmen (20 ms), daraus Wasserfall und alle Kanäle.
//!
//! Arbeit fällt je **Band** und je **belegtem Kanal** an (Frequenz/Modus/Bandbreite/AGC/Ausgaberate), nicht je Hörer:
//! Hörer auf demselben Kanal teilen sich Ausschnitt, Demodulation und Opus-Paket; je Hörer bleibt nur das Kopieren
//! fertiger Pakete in seinen Kanal. Kanäle werden parallel gerechnet (rayon), der Wasserfall einmal je Zoom-Ausschnitt.

use crate::client::{ChannelKey, ClientManager, ClientView, SquelchMode, WaterfallSub, PLUGIN_CLIENT_BASE};
use crabsdr_core::{protocol, DemodMode, IqBuffer};
use crabsdr_dsp::channelizer::{ChannelPlan, Channelizer, ZoomSpectrum};
use crabsdr_dsp::demod::Demodulator;
use crabsdr_dsp::resample::Resampler;
use num_complex::Complex32;
use rayon::prelude::*;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};

/// Opus-Eingangsrate und Bitrate (Ziel: ≤ 60 kbit/s je Hörer inkl. Wasserfall)
pub const OPUS_RATE: u32 = 24000;
/// Wasserfall: Pixel je Zeile (Oberfläche: 1024 px fest) und Zeilen je Sekunde
const WF_PX: usize = 1024;
const WF_LINES_PER_S: u32 = 10;
/// Rauschsperre „auto“: Schwelle = Rauschboden + Abstand
/// Ab so vielen Kanälen parallel (rayon) rechnen, darunter seriell
const PAR_CHANNELS_MIN: usize = 64;
/// Kanäle ohne Hörer nach so vielen Rahmen freigeben (5 s bei 50 fps)
const CHANNEL_IDLE_FRAMES: u64 = 250;

/// CPU-Zeit des eigenen Prozesses in Sekunden (Linux /proc/self/stat; sonst None)
fn read_proc_cpu_s() -> Option<f64> {
    let st = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after = st.rsplit(')').next()?;
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

/// Gesamtlast des Rechners: (beschäftigte, alle) Jiffies aller Kerne aus /proc/stat (Linux; sonst None)
fn read_sys_cpu_ticks() -> Option<(u64, u64)> {
    let st = std::fs::read_to_string("/proc/stat").ok()?;
    let line = st.lines().next()?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 { return None; }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);   // idle + iowait
    let total: u64 = v.iter().sum();
    Some((total - idle, total))
}

fn read_cpu_load() -> Option<[f32; 3]> {
    let contents = std::fs::read_to_string("/proc/loadavg").ok()?;
    let parts: Vec<&str> = contents.split_whitespace().collect();
    if parts.len() >= 3 {
        Some([parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?])
    } else {
        None
    }
}

/// Opus-Encoder je Kanal (nicht je Hörer): 24 kHz mono, 20-ms-Pakete.
struct OpusEnc {
    enc: opus::Encoder,
    buf: Vec<i16>,
    frame: usize,
}

impl OpusEnc {
    fn new(bitrate: i32, complexity: i32) -> Option<Self> {
        let mut enc = opus::Encoder::new(OPUS_RATE, opus::Channels::Mono, opus::Application::Audio).ok()?;
        let _ = enc.set_bitrate(opus::Bitrate::Bits(bitrate));
        let _ = enc.set_complexity(complexity);
        Some(Self { enc, buf: Vec::with_capacity(2048), frame: (OPUS_RATE / 50) as usize })
    }
    fn encode(&mut self, pcm: &[i16]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(pcm);
        let mut out = Vec::new();
        let mut tmp = [0u8; 4000];
        while self.buf.len() >= self.frame {
            let chunk: Vec<i16> = self.buf.drain(..self.frame).collect();
            match self.enc.encode(&chunk, &mut tmp) {
                Ok(n) => out.push(tmp[..n].to_vec()),
                Err(e) => warn!("Opus: {}", e),
            }
        }
        out
    }
}

/// Ein belegter Kanal mit allem Zustand.
struct Channel {
    key: ChannelKey,
    plan: ChannelPlan,
    demod: Demodulator,
    opus: Option<OpusEnc>,
    /// 24 kHz → out_rate für PCM-Abnehmer (Browser ohne Opus)
    pcm_up: Option<Resampler>,
    level_db: f32,
    floor_db: f32,
    last_used: u64,
}

/// Ergebnis eines Kanals je Rahmen.
struct ChannelOut {
    level_db: f32,
    /// ungeglätteter Pegel dieses Rahmens (schnelles Schließen der Rauschsperre)
    level_inst_db: f32,
    floor_db: f32,
    opus: Vec<Vec<u8>>,
    pcm: Option<Vec<u8>>,
}

impl Channel {
    fn new(key: ChannelKey, plan: ChannelPlan) -> Self {
        Self { key, plan, demod: Demodulator::new(), opus: None, pcm_up: None, level_db: -200.0, floor_db: f32::NAN, last_used: 0 }
    }

    /// Breite des Ausschnitts: SSB/CW doppelt (einseitig), NFM mindestens 24 kHz (Überabtastung für den Diskriminator).
    fn extraction_bw(key: &ChannelKey) -> u32 {
        match key.mode {
            DemodMode::Usb | DemodMode::Lsb | DemodMode::Cw => key.bandwidth * 2,
            DemodMode::Fm | DemodMode::Data => key.bandwidth.max(24000),
            _ => key.bandwidth,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process(&mut self, chz: &Channelizer, blocks: &[Vec<Complex32>], frame: u64, want_opus: bool, want_pcm: bool, floor_bin: f32, bin_hz: f32, opus_bitrate: i32, opus_complexity: i32) -> ChannelOut {
        self.last_used = frame;
        let iq = chz.extract_with_plan(&mut self.plan, blocks);
        let p = if iq.is_empty() { 0.0 } else { iq.iter().map(|s| s.norm_sqr()).sum::<f32>() / iq.len() as f32 };
        let level = 10.0 * (p + 1e-20).log10();
        self.level_db = if self.level_db < -150.0 { level } else { 0.6 * self.level_db + 0.4 * level };
        // Rauschboden: Bin-Boden (20. Perzentil der Nachbarschaft) + Kanalbreite + Korrektur.
        // Korrektur empirisch (leerer Kanal, Synthese und echte RTL-Daten): Perzentil liegt unter dem Mittel,
        // Blackman-ENBW 1,73 Bins, Kanalfilter-Flanken → zusammen ≈ +5,5 dB.
        let bins_in_bw = (self.key.bandwidth as f32 / bin_hz).max(1.0);
        let fl = floor_bin + 10.0 * bins_in_bw.log10() + 5.5;
        self.floor_db = if self.floor_db.is_nan() { fl } else { 0.9 * self.floor_db + 0.1 * fl };
        let mut out = ChannelOut { level_db: self.level_db, level_inst_db: level, floor_db: self.floor_db, opus: Vec::new(), pcm: None };
        let out_rate = if self.key.raw { self.key.out_rate } else { OPUS_RATE };
        let audio = self.demod.demodulate(
            &iq, self.plan.channel_rate, self.key.mode, 1, self.key.freq, self.key.bandwidth,
            out_rate, self.key.raw, self.plan.residual_hz, self.key.agc, self.key.pass_lo,
        );
        if let Some(a) = audio {
            if self.key.raw {
                if want_pcm { out.pcm = Some(protocol::encode_audio(&a)); }
            } else {
                if want_opus {
                    if self.opus.is_none() { self.opus = OpusEnc::new(opus_bitrate, opus_complexity); }
                    if let Some(enc) = self.opus.as_mut() { out.opus = enc.encode(&a); }
                }
                if want_pcm {
                    let out_rate = self.key.out_rate;
                    let up = self.pcm_up.get_or_insert_with(|| Resampler::new(OPUS_RATE, out_rate));
                    let f: Vec<f32> = a.iter().map(|&s| s as f32 / 32768.0).collect();
                    let r = up.process(&f);
                    let pcm: Vec<i16> = r.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
                    out.pcm = Some(protocol::encode_audio(&pcm));
                }
            }
        }
        out
    }
}

/// Laufzeitzustand je Hörer (Rauschsperre).
struct ClientRt {
    open_until: u64,
    was_open: bool,
}

/// Wasserfall-Zustand je Zoom-Ausschnitt.
struct WfState {
    prev: Option<Vec<u8>>,
    /// (Hörer, Anmeldungszähler): neue Anmeldung → Absolut-Zeile
    subs: HashSet<(u64, u32)>,
    lines: u32,
}

/// Pixelwert: 1-dB-Stufen ab −140 dBFS (0 … 255 → −140 … +115 dBFS)
fn quantize_db(db: f32) -> u8 {
    (db + 140.0).clamp(0.0, 255.0) as u8
}

/// Rauschboden je Bin (dBFS) in der Umgebung eines Kanals: 20. Perzentil der Spektrum-Bins ±200 kHz um den Kanal,
/// ohne die Kanalbins selbst. Damit hängt die Auto-Rauschsperre nicht vom eigenen Signal ab.
fn neighbourhood_floor(spec: &[f32], center_bin: isize, half_ch_bins: isize, bin_hz: f32) -> f32 {
    let n = spec.len() as isize;
    let reach = ((200_000.0 / bin_hz) as isize).max(half_ch_bins * 4);
    let mut v: Vec<f32> = Vec::with_capacity((2 * reach) as usize);
    let mut b = center_bin - reach;
    while b <= center_bin + reach {
        if b >= 0 && b < n && (b - center_bin).abs() > half_ch_bins {
            v.push(spec[b as usize]);
        }
        b += 1;
    }
    if v.is_empty() {
        return -200.0;
    }
    let k = v.len() / 5;
    v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[k]
}

/// Wasserfall-Verlauf je Band: so viele Zeilen in voller Auflösung (u8 je Bin), 1200 = 2 min bei 10 Zeilen/s
const WF_HIST_LINES: usize = 1200;

/// Aus dem Verlauf (volle Auflösung, älteste zuerst) die letzten `rows` Anzeigezeilen für einen Ausschnitt bauen,
/// je `slow` Server-Zeilen gemittelt (wie der Client im laufenden Betrieb). Ergebnis älteste zuerst.
fn build_hist(hist: &std::collections::VecDeque<Vec<u8>>, sub: WaterfallSub, rows: usize, slow: usize) -> Vec<Vec<u8>> {
    let slow = slow.max(1);
    let avail = hist.len() / slow;
    let n = rows.min(avail);
    if n == 0 { return Vec::new(); }
    let first = hist.len() - n * slow;
    let nbins = hist[0].len();
    let mut out = Vec::with_capacity(n);
    let mut acc = vec![0u32; WF_PX];
    for r in 0..n {
        for a in acc.iter_mut() { *a = 0; }
        for k in 0..slow {
            let line = &hist[first + r * slow + k];
            for (px, a) in acc.iter_mut().enumerate().take(WF_PX) {
                let Some((b0, b1)) = px_bins(nbins, sub, px) else { continue };
                *a += *line[b0..b1].iter().max().unwrap_or(&0) as u32;
            }
        }
        out.push(acc.iter().map(|&v| (v / slow as u32) as u8).collect());
    }
    out
}

/// Farbtabelle wie im Browser (core.js `_crabPalette`): schwarz → dunkelblau → blau → türkis → gelb → orange → weiß
fn palette() -> [[u8; 3]; 256] {
    let stops: [(f32, [f32; 3]); 7] = [(0.0, [0.0, 0.0, 0.0]), (0.18, [0.0, 0.0, 70.0]), (0.38, [0.0, 40.0, 170.0]), (0.55, [0.0, 190.0, 200.0]),
                                       (0.72, [230.0, 230.0, 0.0]), (0.88, [255.0, 90.0, 0.0]), (1.0, [255.0, 255.0, 255.0])];
    let mut p = [[0u8; 3]; 256];
    for (i, px) in p.iter_mut().enumerate() {
        let t = i as f32 / 255.0;
        let mut k = 0; while k < stops.len() - 2 && t > stops[k + 1].0 { k += 1; }
        let (a, b) = (stops[k], stops[k + 1]); let u = (t - a.0) / (b.0 - a.0);
        for (c, p) in px.iter_mut().enumerate().take(3) { *p = (a.1[c] + (b.1[c] - a.1[c]) * u) as u8; }
    }
    p
}

/// Verlauf (älteste zuerst) als JPEG rendern, neueste Zeile oben – gleiche Zuordnung wie der Browser je Zeile
/// (Rauschboden = 20. Perzentil, Fenster je Modus/Look).
fn render_hist_jpeg(lines: &[Vec<u8>], mode: u8) -> Option<Vec<u8>> {
    let n = lines.len(); if n == 0 { return None; }
    let pal = palette();
    let mut rgb = vec![0u8; n * WF_PX * 3];
    for (r, line) in lines.iter().rev().enumerate() {
        let mut sample: Vec<u8> = line.iter().step_by(8).cloned().collect();
        sample.sort_unstable();
        let fl = sample[(sample.len() as f32 * 0.2) as usize] as f32;
        let (lo0, span) = match mode { 2 => (fl - 4.0, 28.0), 3 => (fl + 6.0, 70.0), _ => (fl - 4.0, 50.0) };
        for (px, &v) in line.iter().enumerate().take(WF_PX) {
            let t = (((v as f32 - lo0) / span * 255.0).round()).clamp(0.0, 255.0) as usize;
            let o = (r * WF_PX + px) * 3; rgb[o..o + 3].copy_from_slice(&pal[t]);
        }
    }
    let mut out = Vec::with_capacity(32 * 1024);
    let enc = jpeg_encoder::Encoder::new(&mut out, 70);
    enc.encode(&rgb, WF_PX as u16, n as u16, jpeg_encoder::ColorType::Rgb).ok()?;
    Some(out)
}

/// Eine Wasserfall-Zeile aus dem (fftshift-)Spektrum: je Pixel das Maximum seiner Bins.
/// Bins eines Pixels im Gesamtspektrum: bis Stufe srv_max mehrere Bins je Pixel, darüber mehrere Pixel je Bin
fn px_bins(n: usize, sub: WaterfallSub, px: usize) -> Option<(usize, usize)> {
    let srv_max = (n / WF_PX).max(1).ilog2();
    let start = sub.start_bin as usize;
    let (b0, b1) = if sub.zoom as u32 <= srv_max {
        let bpp = (n / WF_PX) >> sub.zoom;
        (start + px * bpp, start + px * bpp + bpp)
    } else {
        let ppb = 1usize << (sub.zoom as u32 - srv_max);
        (start + px / ppb, start + px / ppb + 1)
    };
    if b0 >= n { None } else { Some((b0, b1.min(n))) }
}

fn build_line(spec: &[f32], sub: WaterfallSub) -> Vec<u8> {
    let n = spec.len();
    (0..WF_PX)
        .map(|px| match px_bins(n, sub, px) {
            Some((b0, b1)) => quantize_db(spec[b0..b1].iter().cloned().fold(f32::MIN, f32::max)),
            None => 0,
        })
        .collect()
}

pub struct DspThread {
    fft_size: usize,
    sample_rate: Arc<AtomicU32>,
    center_freq: Arc<AtomicU64>,
    /// Software-Frequenzkorrektur in ppm (siehe Config)
    corr_ppm: f64,
    fft_fps: u32,
    opus_bitrate: i32,
    opus_complexity: i32,
}

impl DspThread {
    pub fn new(fft_size: usize, sample_rate: Arc<AtomicU32>, center_freq: Arc<AtomicU64>, corr_ppm: f64, fft_fps: u32, opus_bitrate: u32, opus_complexity: u32) -> Self {
        Self { fft_size, sample_rate, center_freq, corr_ppm, fft_fps,
               opus_bitrate: opus_bitrate.clamp(8_000, 128_000) as i32, opus_complexity: opus_complexity.min(10) as i32 }
    }

    /// Hauptschleife auf einem eigenen OS-Thread (nicht Tokio).
    pub fn run(self, mut iq_rx: mpsc::Receiver<IqBuffer>, spectrum_tx: broadcast::Sender<Vec<u8>>, clients: Arc<Mutex<ClientManager>>, band_id: String) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("DSP-Runtime");
        let fps = self.fft_fps.max(1);
        let mut rate = self.sample_rate.load(Ordering::Relaxed);
        let mut channelizer = Channelizer::new(self.fft_size, rate);
        let mut samples_per_frame = (rate / fps) as usize;
        let mut acc: Vec<Complex32> = Vec::with_capacity(samples_per_frame * 2);
        let mut frame: u64 = 0;
        let mut channels: HashMap<ChannelKey, Channel> = HashMap::new();
        let mut client_rt: HashMap<u64, ClientRt> = HashMap::new();
        let mut wf: HashMap<WaterfallSub, WfState> = HashMap::new();
        let mut zooms: HashMap<WaterfallSub, ZoomSpectrum> = HashMap::new();   // Zoom-Spektren je Ausschnitt jenseits der Bin-Grenze
        let mut wf_hist: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::with_capacity(WF_HIST_LINES + 1);
        let mut spec_ema: Option<Vec<f32>> = None;
        let mut cpu_prev: Option<(std::time::Instant, f64)> = None;
        let mut sys_prev: Option<(u64, u64)> = None;
        let mut sys_pct: Option<f32> = None;   // Gesamtlast in % (alle Kerne), None außerhalb von Linux
        let mut cpu_pct: f32 = 0.0;
        let mut listeners_sig: Option<String> = None;
        let mut listeners_sent: u64 = 0;
        let wf_every = (fps / WF_LINES_PER_S).max(1) as u64;
        let level_every = (fps / 4).max(1) as u64;
        let name = std::thread::current().name().unwrap_or("dsp-?").to_string();
        info!("[{}] DSP-Thread: FFT {}, {} S/s, {} Rahmen/s ({} Samples), Mitte {} Hz, Opus {} Hz/{} bit/s",
            name, self.fft_size, rate, fps, samples_per_frame, self.center_freq.load(Ordering::Relaxed), OPUS_RATE, self.opus_bitrate);

        loop {
            let chunk = match rt.block_on(iq_rx.recv()) {
                Some(c) => c,
                None => { info!("[{}] IQ-Kanal geschlossen, DSP-Thread endet", name); break; }
            };
            let new_rate = self.sample_rate.load(Ordering::Relaxed);
            if new_rate != rate {
                info!("[{}] Abtastrate {} → {}, Kanalisierer neu", name, rate, new_rate);
                rate = new_rate;
                samples_per_frame = (rate / fps) as usize;
                channelizer = Channelizer::new(self.fft_size, rate);
                channels.clear();
                acc.clear();
                continue;
            }
            acc.extend_from_slice(&chunk);

            while acc.len() >= samples_per_frame {
                let data: Vec<Complex32> = acc.drain(..samples_per_frame).collect();
                frame += 1;
                let out = channelizer.process(&data);

                // Anzeige-Spektrum glätten
                if !out.spectrum_db.is_empty() {
                    match &mut spec_ema {
                        Some(e) => for (a, &d) in e.iter_mut().zip(&out.spectrum_db) { *a = 0.25 * d + 0.75 * *a; },
                        None => spec_ema = Some(out.spectrum_db.clone()),
                    }
                }
                let spec: &Vec<f32> = match spec_ema.as_ref() { Some(s) => s, None => continue };

                // Hörer-Momentaufnahme (ein Lock je Rahmen)
                let views: Vec<ClientView> = rt.block_on(async { clients.lock().await.snapshot() });
                let mut groups: HashMap<ChannelKey, Vec<&ClientView>> = HashMap::new();
                for v in &views {
                    if let Some(k) = v.tuned { groups.entry(k).or_default().push(v); }
                }
                let center = crate::sdr_pipeline::corrected_center(self.center_freq.load(Ordering::Relaxed), self.corr_ppm);
                for key in groups.keys() {
                    if !channels.contains_key(key) {
                        if let Some(plan) = channelizer.plan_channel(center, key.freq, Channel::extraction_bw(key)) {
                            info!("[{}] Kanal neu: {} Hz {} bw {} ({} Bins, {:.0} S/s, Hörer {})",
                                name, key.freq, key.mode.as_str(), key.bandwidth, plan.n_bins, plan.channel_rate, groups[key].len());
                            channels.insert(*key, Channel::new(*key, plan));
                        }
                    }
                }
                // Was braucht jeder Kanal? (Opus und/oder PCM)
                let need: HashMap<ChannelKey, (bool, bool)> = groups.iter().map(|(k, vs)| {
                    let opus = !k.raw && vs.iter().any(|v| v.use_opus);
                    let pcm = k.raw || vs.iter().any(|v| !v.use_opus);
                    (*k, (opus, pcm))
                }).collect();

                // === Kanäle parallel rechnen ===
                let chz = &channelizer;
                let blocks = &out.fft_blocks;
                let bin_hz = rate as f32 / self.fft_size as f32;
                let floors: HashMap<ChannelKey, f32> = need.keys().map(|k| {
                    let center_bin = ((k.freq as f64 - center as f64) / bin_hz as f64).round() as isize + (self.fft_size / 2) as isize;
                    let half = ((k.bandwidth as f32 / bin_hz / 2.0).ceil() as isize).max(1);
                    (*k, neighbourhood_floor(spec, center_bin, half, bin_hz))
                }).collect();
                // Gemessen (M2): je Kanal ~1 % CPU, rayon-Worker verbrennen bei so kleinen Jobs mehr im Leerlauf-Spin
                // als sie sparen (25 Kanäle: 51 % statt 31 %). Deshalb seriell, parallel erst ab vielen Kanälen.
                let bitrate = self.opus_bitrate;
                let complexity = self.opus_complexity;
                let results: Vec<(ChannelKey, ChannelOut)> = if need.len() >= PAR_CHANNELS_MIN {
                    channels
                        .par_iter_mut()
                        .filter(|(k, _)| need.contains_key(k))
                        .map(|(k, ch)| { let (o, p) = need[k]; (*k, ch.process(chz, blocks, frame, o, p, floors[k], bin_hz, bitrate, complexity)) })
                        .collect()
                } else {
                    channels
                        .iter_mut()
                        .filter(|(k, _)| need.contains_key(k))
                        .map(|(k, ch)| { let (o, p) = need[k]; (*k, ch.process(chz, blocks, frame, o, p, floors[k], bin_hz, bitrate, complexity)) })
                        .collect()
                };

                // === Verteilen (je Hörer nur Kopieren + Rauschsperre) ===
                let hang_frames = |ms: u32| (ms as u64 * fps as u64 / 1000).max(1);
                for (key, o) in &results {
                    for v in &groups[key] {
                        let rt_c = client_rt.entry(v.id).or_insert(ClientRt { open_until: 0, was_open: true });
                        let open_now = match v.squelch.mode {
                            SquelchMode::Off => true,
                            SquelchMode::Manual => o.level_db > v.squelch.db,
                            SquelchMode::Auto => o.level_db > o.floor_db + v.squelch.margin_db,
                        };
                        if open_now { rt_c.open_until = frame + hang_frames(v.squelch.hang_ms); }
                        // Zu: sofort, wenn der ungeglättete Pegel unter die Schwelle fällt (Trägerende) – die Haltezeit
                        // überbrückt nur kurze Einbrüche, in denen der Pegel knapp bleibt. Sonst rauscht es nach jedem
                        // Durchgang so lange wie Glättung + Haltezeit (vorher ≈ 0,5 s).
                        let thr = match v.squelch.mode { SquelchMode::Manual => v.squelch.db, SquelchMode::Auto => o.floor_db + v.squelch.margin_db, SquelchMode::Off => f32::MIN };
                        let still_near = o.level_inst_db > thr - 3.0;
                        let send = open_now || (frame <= rt_c.open_until && still_near);
                        if send {
                            if v.use_opus && !key.raw {
                                for p in &o.opus { let _ = v.tx.try_send(protocol::encode_opus_audio(p)); }
                            } else if let Some(pcm) = &o.pcm {
                                let _ = v.tx.try_send(pcm.clone());
                            }
                        }
                        if frame.is_multiple_of(level_every) || send != rt_c.was_open {
                            let msg = json!({"type": "level", "db": (o.level_db * 10.0).round() / 10.0,
                                             "floor": (o.floor_db * 10.0).round() / 10.0, "sq": send});
                            let _ = v.tx.try_send(protocol::encode_json(&msg.to_string()));
                        }
                        rt_c.was_open = send;
                    }
                }

                // === Zoom-Spektren: Ausschnitte jenseits der Bin-Grenze (1 Pixel = 1 Bin) bekommen eine eigene FFT ===
                {
                    let nb = spec.len();
                    let srv_max = (nb / WF_PX).max(1).ilog2();
                    let mut wanted: HashSet<WaterfallSub> = HashSet::new();
                    for v in &views { if let Some(s) = v.waterfall { if s.zoom as u32 > srv_max { wanted.insert(s); } } }
                    zooms.retain(|s, _| wanted.contains(s));
                    for s in wanted {
                        if let std::collections::hash_map::Entry::Vacant(e) = zooms.entry(s) {
                            let w = nb >> s.zoom;                                   // Breite des Ausschnitts in Bins
                            let mid_bin = s.start_bin as i64 + (w / 2) as i64;
                            let f_mid = (center as i64 + ((mid_bin - (nb / 2) as i64) as f64 * bin_hz as f64).round() as i64).max(0) as u64;
                            if let Some(z) = ZoomSpectrum::new(&channelizer, center, f_mid, w as f32 * bin_hz) { e.insert(z); }
                        }
                        if let Some(z) = zooms.get_mut(&s) { z.push(&channelizer, &out.fft_blocks); }
                    }
                }

                // === Wasserfall-Zeilen je Zoom-Ausschnitt (einmal rechnen, an alle Abonnenten) ===
                if frame.is_multiple_of(wf_every) {
                    // Verlauf immer mitschreiben (auch ohne Hörer), damit neue Hörer sofort einen vollen Wasserfall bekommen
                    wf_hist.push_back(spec.iter().map(|&db| quantize_db(db)).collect());
                    if wf_hist.len() > WF_HIST_LINES { wf_hist.pop_front(); }
                    let mut by_sub: HashMap<WaterfallSub, Vec<&ClientView>> = HashMap::new();
                    for v in &views { if let Some(s) = v.waterfall { by_sub.entry(s).or_default().push(v); } }
                    wf.retain(|s, _| by_sub.contains_key(s));
                    for (sub, vs) in &by_sub {
                        let st = wf.entry(*sub).or_insert(WfState { prev: None, subs: HashSet::new(), lines: 0 });
                        // Zoom-Spektrum, sobald sein Puffer voll ist; davor (und im Verlauf) das gestreckte Gesamtspektrum
                        let line = zooms.get_mut(sub).and_then(|z| z.line(WF_PX)).map(|v| v.iter().map(|&d| quantize_db(d)).collect::<Vec<u8>>()).unwrap_or_else(|| build_line(spec, *sub));
                        let newcomer = vs.iter().any(|v| !st.subs.contains(&(v.id, v.wf_seq)));
                        // Neuankömmlinge (neu, Zoom- oder Bandwechsel) bekommen zuerst den Verlauf ihres Ausschnitts
                        for v in vs.iter().filter(|v| !st.subs.contains(&(v.id, v.wf_seq)) && v.wf_hist_rows > 0) {
                            let lines = build_hist(&wf_hist, *sub, v.wf_hist_rows as usize, v.wf_slow as usize);
                            if lines.is_empty() { continue; }
                            if v.wf_jpeg {
                                if let Some(jpg) = render_hist_jpeg(&lines, v.wf_mode) {
                                    let mut m = Vec::with_capacity(6 + jpg.len());
                                    m.push(protocol::TAG_WATERFALL_JPEG); m.push(sub.zoom);
                                    m.extend_from_slice(&sub.start_bin.to_le_bytes()); m.extend_from_slice(&(lines.len() as u16).to_le_bytes());
                                    m.extend_from_slice(&jpg);
                                    let _ = v.tx.try_send(m);
                                }
                            } else {
                                let _ = v.tx.try_send(protocol::encode_waterfall_hist(sub.zoom, sub.start_bin, &lines));
                            }
                        }
                        let delta = st.prev.is_some() && !newcomer && !st.lines.is_multiple_of(100);
                        // Delta mit Totzone: ±2 dB Flimmern wird nicht gesendet (Client behält den alten Wert);
                        // `prev` ist immer der Stand, den der Client hat → kein Drift.
                        let (payload, sent): (Vec<u8>, Vec<u8>) = if delta {
                            let prev = st.prev.as_ref().unwrap();
                            let mut sent = prev.clone();
                            let payload = line.iter().zip(prev).enumerate().map(|(i, (&a, &b))| {
                                let d = a as i16 - b as i16;
                                if d.abs() <= 2 { 0u8 } else { sent[i] = a; (d as i8) as u8 }
                            }).collect();
                            (payload, sent)
                        } else { (line.clone(), line.clone()) };
                        let frame_bytes = protocol::encode_waterfall(sub.zoom, sub.start_bin, delta, &payload);
                        for v in vs { let _ = v.tx.try_send(frame_bytes.clone()); }
                        st.subs = vs.iter().map(|v| (v.id, v.wf_seq)).collect();
                        st.prev = Some(sent);
                        st.lines += 1;
                    }
                }
                // === Alle 2 s Systemwerte; Hörerliste [id, name, freq, mode] sofort bei Änderung (max. 2×/s), sonst alle 5 s ===
                if spectrum_tx.receiver_count() > 0 {
                    let real: Vec<_> = views.iter().filter(|v| v.id < PLUGIN_CLIENT_BASE).collect();
                    if frame.is_multiple_of(2 * fps as u64) {
                        if let Some(c) = read_proc_cpu_s() {
                            let now = std::time::Instant::now();
                            if let Some((t0, c0)) = cpu_prev { cpu_pct = ((c - c0) / now.duration_since(t0).as_secs_f64() * 100.0) as f32; }
                            cpu_prev = Some((now, c));
                        }
                        if let Some((busy, total)) = read_sys_cpu_ticks() {
                            if let Some((b0, t0)) = sys_prev { if total > t0 { sys_pct = Some(((busy - b0) as f32 / (total - t0) as f32 * 100.0 * 10.0).round() / 10.0); } }
                            sys_prev = Some((busy, total));
                        }
                        let stats = json!({"type": "system_stats", "cpu_load": read_cpu_load().unwrap_or([0.0; 3]),
                                           "cpu_pct": (cpu_pct * 10.0).round() / 10.0, "sys_pct": sys_pct, "clients": real.len(), "channels": results.len()});
                        let _ = spectrum_tx.send(protocol::encode_json(&stats.to_string()));
                    }
                    let list: Vec<_> = real.iter().map(|v| json!([v.id, v.name,
                        v.tuned.map(|k| k.freq).unwrap_or(0), v.tuned.map(|k| k.mode.as_str()).unwrap_or("")])).collect();
                    let sig = list.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
                    let changed = listeners_sig.as_deref() != Some(sig.as_str());
                    if (changed && frame >= listeners_sent + fps as u64 / 2) || frame >= listeners_sent + 5 * fps as u64 {
                        let _ = spectrum_tx.send(protocol::encode_json(&json!({"type": "listeners", "band": band_id, "list": list}).to_string()));
                        listeners_sig = Some(sig); listeners_sent = frame;
                    }
                }

                // Aufräumen
                if frame.is_multiple_of(50) {
                    channels.retain(|_, ch| frame - ch.last_used < CHANNEL_IDLE_FRAMES);
                    let ids: HashSet<u64> = views.iter().map(|v| v.id).collect();
                    client_rt.retain(|id, _| ids.contains(id));
                }
                if frame <= 2 || frame.is_multiple_of(3000) {
                    info!("[{}] Rahmen {}: {} Hörer, {} Kanäle, {} Wasserfall-Abos", name, frame, views.len(), results.len(), wf.len());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_1db_steps() {
        assert_eq!(quantize_db(-140.0), 0);
        assert_eq!(quantize_db(-100.0), 40);
        assert_eq!(quantize_db(0.0), 140);
        assert_eq!(quantize_db(-300.0), 0);
        assert_eq!(quantize_db(500.0), 255);
    }

    #[test]
    fn line_zoom0_takes_max_of_4_bins() {
        // 4096 Bins, Zoom 0 → 4 Bins je Pixel; ein Träger in Bin 1001 muss in Pixel 250 landen
        let mut spec = vec![-100.0f32; 4096];
        spec[1001] = -20.0;
        let line = build_line(&spec, WaterfallSub { zoom: 0, start_bin: 0 });
        assert_eq!(line.len(), WF_PX);
        assert_eq!(line[250], quantize_db(-20.0));
        assert_eq!(line[249], quantize_db(-100.0));
    }

    #[test]
    fn line_zoom2_is_one_bin_per_pixel_with_offset() {
        let mut spec = vec![-100.0f32; 4096];
        spec[1536 + 7] = -30.0;
        let line = build_line(&spec, WaterfallSub { zoom: 2, start_bin: 1536 });
        assert_eq!(line[7], quantize_db(-30.0));
        assert_eq!(line[8], quantize_db(-100.0));
        // Über das Bandende hinaus → 0
        let line = build_line(&spec, WaterfallSub { zoom: 2, start_bin: 4000 });
        assert_eq!(line[200], 0);
    }

    #[test]
    fn line_zoom4_bei_fft_16384_ein_bin_je_pixel() {
        // 8 MS/s → FFT 16384: der Server zoomt bis Stufe 4, dann ist ein Pixel ein Bin
        let mut spec = vec![-100.0f32; 16384];
        spec[9000 + 3] = -25.0;
        let line = build_line(&spec, WaterfallSub { zoom: 4, start_bin: 9000 });
        assert_eq!(line[3], quantize_db(-25.0));
        assert_eq!(line[4], quantize_db(-100.0));
    }

    #[test]
    fn floor_ignores_own_channel_and_strong_neighbours() {
        // Rauschen −90 dB, starker Träger im Kanal (Bin 2048) und ein Nachbar bei 2060 → Boden bleibt ≈ −90
        let mut spec = vec![-90.0f32; 4096];
        for b in 2040..2056 { spec[b] = -20.0; }
        spec[2060] = -30.0;
        let fl = neighbourhood_floor(&spec, 2048, 12, 500.0);
        assert!((fl + 90.0).abs() < 0.5, "Boden {fl}");
    }

    #[test]
    fn deadzone_delta_reconstructs_without_drift() {
        // Server-Seite wie im Thread: prev = Stand des Clients; Client addiert Deltas
        let mut prev: Vec<u8> = vec![50; 8];
        let mut client: Vec<u8> = prev.clone();
        for step in 0..20u8 {
            let line: Vec<u8> = (0..8).map(|i| 50 + ((i as u8 + step) % 5)).collect(); // ±2 Flimmern + Drift
            let mut sent = prev.clone();
            let payload: Vec<u8> = line.iter().zip(&prev).enumerate().map(|(i, (&a, &b))| {
                let d = a as i16 - b as i16;
                if d.abs() <= 2 { 0u8 } else { sent[i] = a; (d as i8) as u8 }
            }).collect();
            for (c, &d) in client.iter_mut().zip(&payload) { *c = c.wrapping_add(d); }
            prev = sent;
            for (c, l) in client.iter().zip(&line) { assert!((*c as i16 - *l as i16).abs() <= 2, "Drift > Totzone"); }
        }
    }
}
