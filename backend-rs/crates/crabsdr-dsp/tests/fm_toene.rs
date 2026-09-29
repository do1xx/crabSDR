//! FM-Kette wie im Server (Kanalfilter → 24-kHz-Ausschnitt → Demodulator → 24 kHz Ton) mit unmoduliertem Träger:
//! im Ton darf kein Pfeifton stehen, bei keiner Abtastrate und keiner Lage im Bin-Raster (MSi2500 bei 8 MS/s, 29.09.).
use crabsdr_dsp::channelizer::Channelizer;
use crabsdr_core::{AgcMode, DemodMode};
use crabsdr_dsp::demod::Demodulator;
use num_complex::Complex32;

fn noise(seed: &mut u64) -> f32 {
    // einfacher Zufall (xorshift), Summe aus 4 gleichverteilten ≈ Gauß
    let mut s = 0.0;
    for _ in 0..4 { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17; s += (*seed % 10000) as f32 / 10000.0 - 0.5; }
    s
}

/// stärkster Ton über 300 Hz im Ton-Spektrum, in dB über dem Median (300–3500 Hz)
fn worst_tone(audio: &[f32], fs: f32) -> (f32, f32) {
    let n = 4096; let segs = audio.len() / n;
    let mut p = vec![0f32; n / 2];
    let mut planner = rustfft::FftPlanner::<f32>::new(); let fft = planner.plan_fft_forward(n);
    for k in 0..segs {
        let mut b: Vec<Complex32> = (0..n).map(|i| { let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos(); Complex32::new(audio[k * n + i] * w, 0.0) }).collect();
        fft.process(&mut b);
        for i in 0..n / 2 { p[i] += b[i].norm_sqr(); }
    }
    let bin = fs / n as f32;
    // schmale Spitze gegenüber ihrer Umgebung (±40 Bins, ohne ±2): so zählt die ansteigende FM-Rauschkante nicht
    let (mut bf, mut bdb) = (0f32, f32::MIN);
    for i in 45..(n / 2 - 45) {
        let f = i as f32 * bin; if f < 300.0 || f > 4500.0 { continue; }   // Sprachbereich: hier war der Pfeifton (2,6/5,2 kHz bei 8 MS/s)
        let mut nb: Vec<f32> = (i - 40..i + 40).filter(|&j| j + 2 < i || j > i + 2).map(|j| p[j]).collect();
        nb.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let db = 10.0 * (p[i] / nb[nb.len() / 2]).log10();
        if db > bdb { bdb = db; bf = f; }
    }
    (bf, bdb)
}

fn run(fs: u32, offset: f64, n: usize) -> (f32, f32) {
    let _ = 0; let frame = (fs / 50) as usize;
    let mut ch = Channelizer::new(n, fs);
    let center = 147_000_000u64; let tune = (center as f64 + offset) as u64;
    let mut plan = ch.plan_channel(center, tune, 24000).unwrap();
    let mut dem = Demodulator::new();
    let mut audio = Vec::new(); let mut seed = 12345u64; let mut t = 0u64;
    let w = std::f64::consts::TAU * (offset + 150.0) / fs as f64;     // Träger 150 Hz neben der Abstimmung
    for _ in 0..150 {
        let iq: Vec<Complex32> = (0..frame).map(|_| { let ph = w * t as f64; t += 1;
            Complex32::new(0.05 * ph.cos() as f32 + 0.02 * noise(&mut seed), 0.05 * ph.sin() as f32 + 0.02 * noise(&mut seed)) }).collect();
        let out = ch.process(&iq);
        let c = ch.extract_with_plan(&mut plan, &out.fft_blocks);
        if let Some(a) = dem.demodulate(&c, plan.channel_rate, DemodMode::Fm, 1, tune, 12000, 24000, false, plan.residual_hz, AgcMode::Medium) {
            audio.extend(a.iter().map(|&s| s as f32 / 32768.0));
        }
    }
    worst_tone(&audio[24000..], 24000.0)
}

#[test]
fn fm_ohne_pfeifton() {
    let mut bad = Vec::new();
    for &fs in &[8_000_000u32, 6_000_000, 2_048_000] {
        for &off in &[-1_787_500.0f64, 212_500.0, 725_000.0, 1_506_250.0, 333_333.0] {
            if off.abs() > fs as f64 * 0.45 { continue; }
            let n = crabsdr_core::config::auto_fft_size(fs);   // wie der Server: Bins ≈ 500 Hz
            let (f, db) = run(fs, off, n);
            println!("{fs} S/s, FFT {n}, Ablage {off:>10} Hz: stärkster Ton {f:.0} Hz, {db:+.1} dB");
            if db > 15.0 { bad.push(format!("{fs}/{off}: {f:.0} Hz {db:+.1} dB")); }
        }
    }
    assert!(bad.is_empty(), "Pfeiftöne: {bad:?}");
    // Gegenprobe: die alte Einstellung (FFT 4096 bei 8 MS/s, Bins 1953 Hz) muss der Test als Pfeifton erkennen
    let (f, db) = run(8_000_000, 725_000.0, 4096);
    println!("Gegenprobe 8 MS/s mit FFT 4096: {f:.0} Hz, {db:+.1} dB");
    assert!(db > 20.0, "Gegenprobe: Pfeifton nicht erkannt ({f:.0} Hz, {db:+.1} dB)");
}
