//! Golden-Test: kompletter DSP-Pfad (Kanalisierer → Kanal → NFM-Demod) auf `testdata/synth.cu8`.
//! Signal A: NFM, 1-kHz-Ton, Hub 2,5 kHz, −20 dBFS bei 438,900 MHz (Mitte 438,500), Nachbarkanal +10 dB in 25 kHz Abstand.
//! Referenz-Demod (numpy) erreicht 40,4 dB SINAD; hier werden ≥ 38 dB verlangt. Übersprungen, wenn die Datei fehlt (Git-LFS).

use crabsdr_core::{AgcMode, DemodMode};
use crabsdr_dsp::channelizer::Channelizer;
use crabsdr_dsp::demod::Demodulator;
use num_complex::Complex32;
use std::path::PathBuf;

fn synth_path() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/synth.cu8");
    let md = std::fs::metadata(&p).ok()?;
    if md.len() < 10_000_000 { return None; } // LFS-Zeiger statt Daten
    Some(p)
}

/// SINAD in dB: Tonleistung (±3 Bins um `tone`) gegen Rest im Band 300–3000 Hz.
fn sinad_db(audio: &[i16], fs: f32, tone: f32) -> f32 {
    let n = 8192usize;
    let mut p = vec![0.0f64; n / 2 + 1];
    let mut segs = 0;
    let mut start = fs as usize; // erste Sekunde (Einschwingen, AGC) weg
    while start + n <= audio.len() {
        let mut buf: Vec<Complex32> = (0..n).map(|i| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            Complex32::new(audio[start + i] as f32 / 32768.0 * w, 0.0)
        }).collect();
        let mut planner = rustfft::FftPlanner::new();
        planner.plan_fft_forward(n).process(&mut buf);
        for (i, v) in buf.iter().take(n / 2 + 1).enumerate() { p[i] += v.norm_sqr() as f64; }
        segs += 1; start += n;
    }
    assert!(segs > 0, "zu wenig Audio");
    let bin_hz = fs / n as f32;
    let (mut pt, mut pr) = (0.0f64, 0.0f64);
    for (i, &v) in p.iter().enumerate() {
        let f = i as f32 * bin_hz;
        if f < 300.0 || f > 3000.0 { continue; }
        if (f - tone).abs() <= 3.0 * bin_hz { pt += v } else { pr += v }
    }
    (10.0 * ((pt + pr) / pr.max(1e-30)).log10()) as f32
}

#[test]
fn golden_synth_a_sinad() {
    let Some(path) = synth_path() else { eprintln!("testdata/synth.cu8 fehlt – Golden-Test übersprungen"); return; };
    let fs = 2_048_000u32;
    let center = 438_500_000u64;
    let tune = 438_900_000u64;
    let seconds = 8usize;
    let raw = std::fs::read(&path).unwrap();
    let raw = &raw[..(fs as usize * 2 * seconds).min(raw.len())];

    let mut chz = Channelizer::new(4096, fs);
    let mut plan = chz.plan_channel(center, tune, 24_000).expect("Plan");
    let mut demod = Demodulator::new();
    let frame = (fs / 50) as usize;
    let mut audio: Vec<i16> = Vec::new();
    let mut iq: Vec<Complex32> = Vec::with_capacity(frame);
    for chunk in raw.chunks(frame * 2) {
        iq.clear();
        iq.extend(chunk.chunks_exact(2).map(|p| Complex32::new((p[0] as f32 - 127.5) / 127.5, (p[1] as f32 - 127.5) / 127.5)));
        let out = chz.process(&iq);
        let ch = chz.extract_with_plan(&mut plan, &out.fft_blocks);
        if let Some(a) = demod.demodulate(&ch, plan.channel_rate, DemodMode::Fm, 1, tune, 12_500, 24_000, false, plan.residual_hz, AgcMode::Medium) {
            audio.extend(a);
        }
    }
    assert!((plan.channel_rate - 24_000.0).abs() < 1.0, "Kanalrate {}", plan.channel_rate);
    assert!(audio.len() > 24_000 * (seconds - 1), "zu wenig Audio: {}", audio.len());
    let s = sinad_db(&audio, 24_000.0, 1000.0);
    eprintln!("Golden Synth A: SINAD {s:.1} dB");
    assert!(s >= 38.0, "SINAD {s:.1} dB < 38 dB");
}
