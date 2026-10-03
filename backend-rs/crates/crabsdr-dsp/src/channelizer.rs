//! FFT-Kanalisierer mit Overlap-Save (25 % Überlappung: Hop = 3N/4, je Block werden N/4 Samples verworfen).
//!
//! Eine FFT-Folge je Band liefert (a) das Anzeige-Spektrum und (b) die Blöcke, aus denen beliebig viele Kanäle
//! ausgeschnitten werden. Ein Kanal wird durch einen `ChannelPlan` beschrieben (Bins, Filter, iFFT, Scratch);
//! `extract_with_plan` braucht nur `&self` und kann deshalb für viele Kanäle parallel laufen.
//!
//! Normierung: ein Vollausschlag-Sinus (|IQ| = 1) ergibt 0 dBFS im Spektrum und Kanalleistung 0 dBFS.

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Ergebnis eines Rahmens.
pub struct FrameOutput {
    /// Anzeige-Spektrum in dBFS, fftshift (Index 0 = tiefste Frequenz), über die Anzeige-FFTs des Rahmens gemittelt
    pub spectrum_db: Vec<f32>,
    /// Rohe (ungefensterte) FFT-Blöcke mit 50 % Überlappung für die Kanal-Extraktion
    pub fft_blocks: Vec<Vec<Complex32>>,
}

/// Vorberechneter Ausschnitt eines Kanals.
pub struct ChannelPlan {
    pub n_bins: usize,
    indices: Vec<usize>,
    filter: Vec<f32>,
    ifft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    /// Abtastrate des Kanals in Hz (= n_bins × bin_hz)
    pub channel_rate: f32,
    /// Frequenzfehler durch Bin-Rasterung in Hz
    pub residual_hz: f64,
    /// Phasenfortschritt je Block in Umdrehungen: Das Verschieben von Bin k0 nach 0 mischt in Blockzeit, der Block
    /// beginnt aber bei m·hop. Ohne Ausgleich springt die Phase an jeder Blockgrenze um 2π·k0·hop/N – bei hop = 3N/4
    /// für jede Frequenz außerhalb des 2-kHz-Rasters (FM: Knacken im Blocktakt = Pfeifton mit Oberwellen).
    block_step_cyc: f64,
    block_phase_cyc: f64,
}

pub struct Channelizer {
    fft_size: usize,
    sample_rate: u32,
    bin_hz: f32,
    window: Vec<f32>,
    /// 1 / Summe(Fenster): kohärente Verstärkung, damit ein Sinus mit Amplitude 1 → 0 dBFS
    window_norm: f32,
    fft_forward: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    /// Noch nicht verarbeitete Samples (Überhang + Rest des letzten Rahmens): Blöcke laufen über Rahmengrenzen
    pending: Vec<Complex32>,
    hop_size: usize,
    /// Anzahl der Anzeige-FFTs seit Start (für den Takt)
    hops_total: usize,
    /// Alle wie viele Hops eine gefensterte Anzeige-FFT gerechnet wird
    display_every: usize,
}

impl Channelizer {
    pub fn new(fft_size: usize, sample_rate: u32) -> Self {
        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let scratch_len = fft_forward.get_inplace_scratch_len();
        // Blackman-Fenster für die Anzeige
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let x = std::f32::consts::PI * 2.0 * i as f32 / (fft_size - 1) as f32;
                0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
            })
            .collect();
        let window_norm = 1.0 / window.iter().sum::<f32>();
        let hop_size = fft_size * 3 / 4;
        Self {
            fft_size,
            sample_rate,
            bin_hz: sample_rate as f32 / fft_size as f32,
            window,
            window_norm,
            fft_forward,
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            pending: Vec::with_capacity(fft_size * 2),
            hop_size,
            hops_total: 0,
            display_every: 13,
        }
    }

    /// Einen Rahmen verarbeiten: Overlap-Save-Blöcke + Anzeige-Spektrum.
    /// Blöcke laufen über Rahmengrenzen (Überhang wird aufbewahrt), es geht kein Sample verloren.
    pub fn process(&mut self, iq: &[Complex32]) -> FrameOutput {
        let n = self.fft_size;
        let hop = self.hop_size;
        self.pending.extend_from_slice(iq);
        let mut fft_blocks = Vec::with_capacity(self.pending.len() / hop + 1);
        let mut acc = vec![0.0f32; n];
        let mut n_disp = 0usize;
        let mut pos = 0usize;
        while pos + n <= self.pending.len() {
            let mut block: Vec<Complex32> = self.pending[pos..pos + n].to_vec();
            if self.hops_total.is_multiple_of(self.display_every) {
                let mut win: Vec<Complex32> = block.iter().zip(&self.window).map(|(s, w)| s * w).collect();
                self.fft_forward.process_with_scratch(&mut win, &mut self.scratch);
                for (i, v) in win.iter().enumerate() {
                    let shifted = (i + n / 2) % n;
                    let m = v.norm() * self.window_norm;
                    acc[shifted] += m * m;
                }
                n_disp += 1;
            }
            self.fft_forward.process_with_scratch(&mut block, &mut self.scratch);
            fft_blocks.push(block);
            pos += hop;
            self.hops_total += 1;
        }
        // Überhang: alles ab dem Anfang des nächsten (noch unvollständigen) Blocks
        self.pending.drain(..pos);

        let spectrum_db = if n_disp > 0 {
            let inv = 1.0 / n_disp as f32;
            acc.iter().map(|&p| 10.0 * (p * inv + 1e-20).log10()).collect()
        } else {
            Vec::new()
        };
        FrameOutput { spectrum_db, fft_blocks }
    }

    /// Kanalfilter im Frequenzbereich: 80 % der Bins voll, je 10 % Raised-Cosine-Flanke.
    /// (Ein echter FIR mit ≤ N/2 Taps wäre bei N = 4096 zu flach für 12-kHz-Kanäle; die weitere Kanalformung
    /// übernimmt der FIR-ZF-Filter im Demodulator bei Kanalrate.)
    fn make_channel_filter(n_bins: usize) -> Vec<f32> {
        let n = n_bins;
        let pass_bins = (n as f32 * 0.80 / 2.0).floor() as usize;
        let rolloff_bins = n / 2 - pass_bins;
        (0..n)
            .map(|i| {
                let fi = if i <= n / 2 { i as isize } else { i as isize - n as isize };
                let a = fi.unsigned_abs();
                if a <= pass_bins {
                    1.0
                } else if rolloff_bins > 0 && a <= pass_bins + rolloff_bins {
                    let x = (a - pass_bins) as f32 / rolloff_bins as f32;
                    0.5 * (1.0 + (std::f32::consts::PI * x).cos())
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Plan für einen Kanal (Bins um die Frequenz, Filter, iFFT). None, wenn außerhalb des Bandes.
    pub fn plan_channel(&self, center_freq: u64, tune_freq: u64, bandwidth: u32) -> Option<ChannelPlan> {
        let min_bins = ((12000.0f32 / self.bin_hz).ceil() as usize).max(4);
        let mut n_bins = ((bandwidth as f32 / self.bin_hz).ceil() as usize).max(min_bins);
        // Vielfaches von 4: je Block kommen n_bins·hop/N = ¾·n_bins Kanal-Samples heraus – das muss ganzzahlig sein.
        // Bei 2,048 MS/s (500-Hz-Bins) war es das zufällig immer; bei 8 MS/s ergab FM 14 Bins → 10,5 Samples, je Block
        // fehlte ein halbes Sample → Knacken im Blocktakt (≈ 2,6 kHz Pfeifton, MSi2500 auf Gartow 29.09.).
        n_bins = n_bins.div_ceil(4) * 4;
        n_bins = n_bins.min(self.fft_size);

        let offset_hz = tune_freq as f64 - center_freq as f64;
        if offset_hz.abs() > self.sample_rate as f64 / 2.0 {
            return None;
        }
        let center_bin_exact = offset_hz / self.bin_hz as f64;
        let center_bin = center_bin_exact.round() as isize;
        let residual_hz = (center_bin_exact - center_bin as f64) * self.bin_hz as f64;
        let half = (n_bins / 2) as isize;
        let indices: Vec<usize> = (-half..half)
            .map(|i| (i + center_bin).rem_euclid(self.fft_size as isize) as usize)
            .collect();
        let mut planner = FftPlanner::new();
        let ifft = planner.plan_fft_inverse(n_bins);
        let scratch = vec![Complex32::new(0.0, 0.0); ifft.get_inplace_scratch_len()];
        Some(ChannelPlan {
            n_bins,
            indices,
            filter: Self::make_channel_filter(n_bins),
            ifft,
            scratch,
            channel_rate: n_bins as f32 * self.bin_hz,
            residual_hz,
            block_step_cyc: (center_bin as f64 * self.hop_size as f64 / self.fft_size as f64).rem_euclid(1.0),
            block_phase_cyc: 0.0,
        })
    }

    /// Kanal aus den FFT-Blöcken ausschneiden (Overlap-Save: je Block werden die ersten n_bins/4 Samples verworfen).
    /// Braucht nur `&self` → parallel für viele Kanäle nutzbar.
    pub fn extract_with_plan(&self, plan: &mut ChannelPlan, fft_blocks: &[Vec<Complex32>]) -> Vec<Complex32> {
        let n_bins = plan.n_bins;
        let valid_start = n_bins - n_bins * self.hop_size / self.fft_size;
        let inv = 1.0 / self.fft_size as f32; // Dezimation: 1/N hält die Amplitude (0 dBFS = Vollausschlag)
        let mut out = Vec::with_capacity((n_bins - valid_start) * fft_blocks.len());
        let mut ch: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); n_bins];
        for block in fft_blocks {
            // Bins holen, dabei ifftshift (DC nach Index 0) und Filter anwenden
            let half = n_bins / 2;
            for (j, &idx) in plan.indices.iter().enumerate() {
                let k = (j + half) % n_bins;
                ch[k] = block[idx] * plan.filter[k];
            }
            plan.ifft.process_with_scratch(&mut ch, &mut plan.scratch);
            // Blockphase fortsetzen (siehe block_step_cyc); Schritt ist bei hop = 3N/4 ein Vielfaches von 90° → exakt
            let ph = -std::f64::consts::TAU * plan.block_phase_cyc;
            let rot = Complex32::new((ph.cos() as f32) * inv, (ph.sin() as f32) * inv);
            out.extend(ch[valid_start..].iter().map(|s| s * rot));
            plan.block_phase_cyc = (plan.block_phase_cyc + plan.block_step_cyc).rem_euclid(1.0);
        }
        out
    }

    /// Komfort-Variante (Tests, Einzelaufrufe): Plan anlegen und extrahieren.
    /// Gibt (Kanal-IQ, Kanalrate, Restfehler) zurück.
    pub fn extract_channel(
        &mut self,
        fft_blocks: &[Vec<Complex32>],
        center_freq: u64,
        tune_freq: u64,
        bandwidth: u32,
    ) -> Option<(Vec<Complex32>, f32, f64)> {
        if fft_blocks.is_empty() {
            return None;
        }
        let mut plan = self.plan_channel(center_freq, tune_freq, bandwidth)?;
        let iq = self.extract_with_plan(&mut plan, fft_blocks);
        Some((iq, plan.channel_rate, plan.residual_hz))
    }

    pub fn fft_size(&self) -> usize { self.fft_size }
    pub fn sample_rate(&self) -> u32 { self.sample_rate }
    pub fn bin_hz(&self) -> f32 { self.bin_hz }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(n: usize, fs: f32, f: f32, amp: f32) -> Vec<Complex32> {
        (0..n).map(|i| Complex32::new(0.0, 2.0 * PI * f * i as f32 / fs).exp() * amp).collect()
    }

    #[test]
    fn test_channelizer_basic() {
        let mut ch = Channelizer::new(256, 256_000);
        let iq: Vec<Complex32> = (0..512).map(|_| Complex32::new(0.1, 0.0)).collect();
        let out = ch.process(&iq);
        assert_eq!(out.spectrum_db.len(), 256);
        assert!(out.fft_blocks.len() >= 2);
        // Streaming: 100 Samples nachschieben ergibt keinen neuen Block, nichts geht verloren
        let out2 = ch.process(&iq[..100]);
        assert!(out2.fft_blocks.is_empty() && out2.spectrum_db.is_empty());
    }

    #[test]
    fn test_spectrum_dbfs_calibrated() {
        // Sinus mit Amplitude 1 bei +10 kHz → Spitze im Spektrum ≈ 0 dBFS
        let mut ch = Channelizer::new(256, 256_000);
        let iq = tone(1024, 256_000.0, 10_000.0, 1.0);
        let out = ch.process(&iq);
        let peak = out.spectrum_db.iter().cloned().fold(f32::MIN, f32::max);
        assert!((peak).abs() < 1.0, "Spitze sollte ≈ 0 dBFS sein, ist {peak}");
    }

    #[test]
    fn test_extract_channel_amplitude() {
        // Amplitude 0,5 bei Bin-Mitte → Kanal-IQ-Betrag ≈ 0,5 (Normierung 1/N)
        let mut ch = Channelizer::new(256, 256_000);
        let iq = tone(256 * 6, 256_000.0, 10_000.0, 0.5);
        let out = ch.process(&iq);
        let (c, rate, residual) = ch.extract_channel(&out.fft_blocks, 100_000_000, 100_010_000, 12_500).unwrap();
        assert!(rate > 0.0 && residual.abs() < 1.0);
        let mags: Vec<f32> = c.iter().skip(20).map(|s| s.norm()).collect();
        let mean = mags.iter().sum::<f32>() / mags.len() as f32;
        assert!((mean - 0.5).abs() < 0.05, "Betrag sollte ≈ 0,5 sein, ist {mean}");
    }

    #[test]
    fn test_overlap_save_continuity() {
        let mut ch = Channelizer::new(256, 256_000);
        let iq = tone(256 * 6, 256_000.0, 10_000.0, 1.0);
        let mut all = Vec::new();
        for frame in 0..3 {
            let out = ch.process(&iq[frame * 512..(frame + 1) * 512]);
            if let Some((c, _, _)) = ch.extract_channel(&out.fft_blocks, 100_000_000, 100_010_000, 12_500) {
                all.extend(c);
            }
        }
        assert!(all.len() > 30);
        let m: Vec<f32> = all.iter().map(|s| s.norm()).collect();
        for i in 21..m.len() {
            assert!((m[i] - m[i - 1]).abs() < 0.05, "Amplitudensprung bei {i}: {} -> {}", m[i - 1], m[i]);
        }
    }

    #[test]
    fn test_channel_filter_rejection() {
        // Störer 40 kHz neben dem Kanal (12,5 kHz Kanal bei 1 kHz Bins → 12 Bins) muss ≥ 40 dB unterdrückt sein
        let fs = 256_000.0;
        let mut ch = Channelizer::new(256, 256_000);
        let n = 256 * 8;
        let want = tone(n, fs, 0.0, 0.1);
        let bad = tone(n, fs, 40_000.0, 1.0);
        let iq: Vec<Complex32> = want.iter().zip(&bad).map(|(a, b)| a + b).collect();
        let out = ch.process(&iq);
        let (c, _, _) = ch.extract_channel(&out.fft_blocks, 100_000_000, 100_000_000, 12_500).unwrap();
        let p: f32 = c.iter().skip(20).map(|s| s.norm_sqr()).sum::<f32>() / (c.len() - 20) as f32;
        let p_db = 10.0 * p.log10();
        // Nutzsignal 0,1 → −20 dBFS; Störer bei 0 dBFS müsste < −60 dBFS erscheinen → Summe ≈ −20 dB
        assert!(p_db < -19.0 && p_db > -21.0, "Kanalleistung {p_db} dB, Störer nicht unterdrückt?");
    }
    #[test]
    fn test_hohe_abtastrate_ohne_pfeifton() {
        // 8 MS/s (MSi2500, 29.09.): Bins 1953 Hz breit, FM braucht 14 Bins → 14·¾ = 10,5 Samples je Block. Die Bin-Zahl
        // muss ein Vielfaches von 4 sein, sonst fehlt je Block ein halbes Sample (Knacken im Blocktakt ≈ 2,6 kHz).
        for &fs in &[8_000_000u32, 6_000_000, 2_048_000] {
            let n = 4096;
            let mut ch = Channelizer::new(n, fs);
            let blocks = 60;
            let iq = tone(n * blocks, fs as f32, 331_700.0, 0.5);
            let out = ch.process(&iq);
            let c = 1_000_000u64;
            // 24 kHz wie der FM-Ausschnitt (extraction_bw: FM mindestens 24 kHz) → bei 8 MS/s 14 Bins
            let (x, rate, resid) = ch.extract_channel(&out.fft_blocks, c, c + 331_700 - 400, 24000).unwrap();
            // Anzahl: jeder Block liefert genau hop/N der Kanal-Samples (Kanalrate × Blockdauer)
            let per_block = rate as f64 * (n * 3 / 4) as f64 / fs as f64;
            assert!((x.len() as f64 / out.fft_blocks.len() as f64 - per_block).abs() < 1e-6,
                "{fs}: {} Samples je Block, erwartet {per_block}", x.len() as f64 / out.fft_blocks.len() as f64);
            let step = std::f64::consts::TAU * (400.0 + resid) / rate as f64;   // Ton im Kanal: Ablage + Rasterfehler
            let mut worst = 0.0f64;
            for w in x[100..].windows(2) {
                let d = (w[1] * w[0].conj()).arg() as f64;
                worst = worst.max(((d - step + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI).abs());
            }
            assert!(worst < 0.2, "{fs}: größter Phasenfehler {worst:.3} rad");
        }
    }

    #[test]
    fn test_block_phase_continuous_off_grid() {
        // Ton genau auf einem Bin, der kein Vielfaches von 4 ist (425 kHz bei 500 Hz/Bin = Bin 850 → Sprung 180°),
        // plus 300 Hz Ablage: das Kanalsignal muss eine glatte Drehung sein, ohne Phasensprünge an den Blockgrenzen.
        let fs = 2_048_000u32; let n = 4096;
        for &(off, extra) in &[(425_000.0f32, 300.0f32), (425_500.0, 300.0), (200_000.0, 300.0), (-312_500.0, -700.0)] {
            let mut ch = Channelizer::new(n, fs);
            let iq = tone(n * 40, fs as f32, off + extra, 0.5);
            let out = ch.process(&iq);
            let c = 1_000_000u64;   // Mittenfrequenz beliebig, Ablage relativ dazu
            let (x, rate, resid) = ch.extract_channel(&out.fft_blocks, c, (c as f64 + off as f64) as u64, 12500).unwrap();
            let f_exp = extra as f64 - resid;
            let step = std::f64::consts::TAU * f_exp / rate as f64;
            let mut worst = 0.0f64;
            for w in x[200..].windows(2) {
                let d = (w[1] * w[0].conj()).arg() as f64;
                let e = (d - step + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                worst = worst.max(e.abs());
            }
            assert!(worst < 0.2, "Ablage {off} Hz: größter Phasenfehler {worst:.3} rad");
        }
    }

}
