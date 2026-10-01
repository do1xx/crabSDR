//! Demodulator — takes narrowband IQ from channelizer, outputs audio.
//!
//! Demodulatoren
//!
//! No mixer/filter/decimation — the channelizer handles that via FFT.

use crabsdr_core::{AgcMode, DemodMode};
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::collections::HashMap;
use std::f32::consts::PI;

use crate::resample::Resampler;
use crate::design_lowpass_fir;

/// Obergrenze der AGC-Verstärkung (60 dB): Rauschen auf leerem Kanal wird nicht endlos hochgezogen.
const AGC_MAX_GAIN: f32 = 1000.0;
/// Haltezeit der AM/SSB-Regelung nach einer lauten Stelle (Rahmen à 20 ms → 1 s)
const AGC_HANG_FRAMES: u32 = 50;

/// FIR-Filter (Direktform, Ringpuffer) für reelle Signale; für IQ je einer für I und Q.
struct Fir {
    taps: Vec<f32>,
    buf: Vec<f32>,
    pos: usize,
}

impl Fir {
    fn new(taps: Vec<f32>) -> Self {
        let n = taps.len();
        Self { taps, buf: vec![0.0; n], pos: 0 }
    }
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        self.buf[self.pos] = x;
        let mut acc = 0.0f32;
        let mut idx = self.pos;
        for &t in &self.taps {
            acc += t * self.buf[idx];
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        self.pos = (self.pos + 1) % n;
        acc
    }
}

/// 2nd-order Biquad IIR filter (direct form II transposed).
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate;
        let sin_omega = omega.sin();
        let cos_omega = omega.cos();
        let q = core::f32::consts::FRAC_1_SQRT_2;
        let alpha = sin_omega / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos_omega) / 2.0 / a0,
            b1: (1.0 - cos_omega) / a0,
            b2: (1.0 - cos_omega) / 2.0 / a0,
            a1: -2.0 * cos_omega / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// FM de-emphasis filter (single-pole IIR lowpass).
    /// tau = 750µs for NFM (amateur/PMR), 50µs for WFM (broadcast).
    fn deemphasis(tau_us: f32, sample_rate: f32) -> Self {
        let dt = 1.0 / sample_rate;
        let tau = tau_us * 1e-6;
        let a = dt / (tau + dt);
        Self {
            b0: a,
            b1: 0.0,
            b2: 0.0,
            a1: -(1.0 - a),
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Highpass filter (Butterworth, Q=1/sqrt(2)).
    fn highpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate;
        let sin_omega = omega.sin();
        let cos_omega = omega.cos();
        let q = core::f32::consts::FRAC_1_SQRT_2;
        let alpha = sin_omega / (2.0 * q);

        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos_omega) / 2.0 / a0,
            b1: -(1.0 + cos_omega) / a0,
            b2: (1.0 + cos_omega) / 2.0 / a0,
            a1: -2.0 * cos_omega / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

}

// --- WFM Stereo + RDS ---

/// Goertzel single-frequency energy detector (more efficient than FFT for one freq).
fn goertzel_energy(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
    let n = samples.len();
    if n == 0 { return 0.0; }
    let k = (freq * n as f32 / sample_rate).round();
    let w = 2.0 * PI * k / n as f32;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (n as f32 * n as f32)
}

/// RDS CRC polynomial: x^10 + x^8 + x^7 + x^5 + x^4 + x^3 + 1
const RDS_POLY: u32 = 0x5B9;
const RDS_OFFSET_A: u16 = 0x0FC;
const RDS_OFFSET_B: u16 = 0x198;
const RDS_OFFSET_C: u16 = 0x168;
const RDS_OFFSET_C_PRIME: u16 = 0x350;
const RDS_OFFSET_D: u16 = 0x1B4;

fn rds_syndrome(block: u32) -> u16 {
    // Standard polynomial division: shift each bit into a register,
    // reduce modulo g(x) when bit 10 overflows.
    let mut reg = 0u32;
    for i in (0..26).rev() {
        let bit = (block >> i) & 1;
        reg = (reg << 1) | bit;
        if reg & (1 << 10) != 0 {
            reg ^= RDS_POLY; // g(x) has x^10 term, clears bit 10
        }
    }
    (reg & 0x3FF) as u16
}

/// Compute RDS check bits for 16-bit data word with given offset.
/// Uses LFSR formulation which implicitly handles x^10 multiplication.
#[cfg(test)]
fn rds_encode(data: u16, offset: u16) -> u32 {
    let mut reg = 0u32;
    for i in (0..16).rev() {
        let bit = ((data as u32) >> i) & 1;
        let fb = ((reg >> 9) ^ bit) & 1;
        reg = (reg << 1) & 0x3FF;
        if fb == 1 {
            // XOR with generator polynomial without leading x^10 term
            reg ^= 0x1B9; // x^8+x^7+x^5+x^4+x^3+1
        }
    }
    let crc = (reg & 0x3FF) as u16;
    let check = crc ^ offset;
    ((data as u32) << 10) | (check as u32)
}

/// RDS decoder state machine.
///
/// RDS uses biphase (Manchester) coding at 2375 symbols/sec (2× the 1187.5 bps data rate).
/// Each data bit consists of two PSK symbols of opposite polarity. The processing chain:
///   1. Pilot PLL locks to 19 kHz → 3× phase = 57 kHz reference
///   2. BPSK demod: multiply RDS bandpass output with 57 kHz reference
///   3. Integrate-and-dump at SYMBOL rate (2375 sym/s, not 1187.5 bps!)
///   4. Gardner TED for symbol clock recovery
///   5. Biphase decode: difference of consecutive PSK symbols → data bit every 2 symbols
///   6. Differential decode: XOR consecutive biphase bits → final data
struct RdsDecoder {
    // 19 kHz pilot PLL
    pilot_bpf: Biquad,
    pilot_phase: f32,
    pilot_freq: f32,
    pilot_locked: bool,
    pilot_lock_energy: f32,
    // 57 kHz RDS bandpass filter (2x biquad)
    rds_bpf1: Biquad,
    rds_bpf2: Biquad,
    // Post-mixer lowpass filter (2400 Hz, 4th-order = 2x cascaded biquad)
    // Rejects stereo subcarrier leakage and wideband noise after mixing.
    rds_lpf1: Biquad,
    rds_lpf2: Biquad,
    // Symbol integrator (integrate-and-dump at 2375 sym/s)
    symbol_integrator: f32,
    symbol_clock: f32,
    samples_per_symbol: f32,
    // Gardner TED for symbol timing recovery
    timing_offset: f32,
    last_symbol_val: f32,   // Previous symbol decision value
    mid_sample: f32,        // Demod at midpoint for Gardner TED
    mid_sampled: bool,
    // Biphase decoder (2 PSK symbols → 1 data bit)
    prev_psk_symbol: f32,   // Previous PSK symbol value
    biphase_clock: u32,     // Counts PSK symbols (even/odd)
    biphase_polarity: u32,  // 0 or 1: which phase produces data bits
    biphase_history: [f32; 128], // For polarity detection
    biphase_hist_idx: usize,
    // Differential decoder
    prev_biphase_bit: bool,
    // Block sync
    block_buf: u32,
    bits_in_block: u8,
    group_buf: [u16; 4],
    group_idx: u8,
    synced: bool,
    good_blocks: u16,
    bad_blocks: u16,
    // Decoded data
    pi_code: u16,
    ps_name: [u8; 8],
    ps_segments: u8,
    radiotext: [u8; 64],
    rt_segments: u16,
    pty: u8,
    // Change detection
    last_sent_ps: [u8; 8],
    last_sent_rt: [u8; 64],
    last_sent_pty: u8,
    has_update: bool,
    // Debug counters
    total_bits: u64,
    total_symbols: u64,
    // Signal diagnostics
    diag_bpf_sum: f32,
    diag_demod_sum: f32,
    diag_integ_sum: f32,
    diag_integ_count: u32,
    diag_sample_count: u32,
    diag_timing_err_sum: f32,
    diag_even_sum: f32,
    diag_odd_sum: f32,
    // Log a few raw symbol values on first sync for debugging
    diag_symbol_log: Vec<f32>,
    diag_logged_symbols: bool,
}

impl RdsDecoder {
    fn new(sample_rate: f32) -> Self {
        let pilot_bpf = Biquad::bandpass(19000.0, 2000.0, sample_rate);
        let rds_bpf1 = Biquad::bandpass(57000.0, 4000.0, sample_rate);
        let rds_bpf2 = Biquad::bandpass(57000.0, 4000.0, sample_rate);
        // Symbol rate = 2× bit rate (biphase coding)
        let samples_per_symbol = sample_rate / 2375.0;
        Self {
            pilot_bpf,
            pilot_phase: 0.0,
            pilot_freq: 2.0 * PI * 19000.0 / sample_rate,
            pilot_locked: false,
            pilot_lock_energy: 0.0,
            rds_bpf1, rds_bpf2,
            rds_lpf1: Biquad::lowpass(2400.0, sample_rate),
            rds_lpf2: Biquad::lowpass(2400.0, sample_rate),
            symbol_integrator: 0.0,
            symbol_clock: 0.0,
            samples_per_symbol,
            timing_offset: 0.0,
            last_symbol_val: 0.0,
            mid_sample: 0.0,
            mid_sampled: false,
            prev_psk_symbol: 0.0,
            biphase_clock: 0,
            biphase_polarity: 0,
            biphase_history: [0.0; 128],
            biphase_hist_idx: 0,
            prev_biphase_bit: false,
            block_buf: 0,
            bits_in_block: 0,
            group_buf: [0u16; 4],
            group_idx: 0,
            synced: false,
            good_blocks: 0,
            bad_blocks: 0,
            pi_code: 0,
            ps_name: [b' '; 8],
            ps_segments: 0,
            radiotext: [b' '; 64],
            rt_segments: 0,
            pty: 0,
            last_sent_ps: [0; 8],
            last_sent_rt: [0; 64],
            last_sent_pty: 0xFF,
            has_update: false,
            total_bits: 0,
            total_symbols: 0,
            diag_bpf_sum: 0.0,
            diag_demod_sum: 0.0,
            diag_integ_sum: 0.0,
            diag_integ_count: 0,
            diag_sample_count: 0,
            diag_timing_err_sum: 0.0,
            diag_even_sum: 0.0,
            diag_odd_sum: 0.0,
            diag_symbol_log: Vec::with_capacity(200),
            diag_logged_symbols: false,
        }
    }

    /// Process FM discriminator output samples (before de-emphasis!).
    ///
    /// Processing chain:
    /// 1. PLL locks onto 19 kHz pilot → 3× phase = 57 kHz reference
    /// 2. BPSK demod: multiply RDS bandpass output with 57 kHz reference
    /// 3. 4th-order LPF at 2400 Hz (noise rejection)
    /// 4. Integrate-and-dump at 2375 sym/s + Gardner TED
    /// 5. Biphase decode + differential decode
    fn process(&mut self, audio: &[f32], sample_rate: f32) {
        // Pilot PLL parameters (~30 Hz loop BW)
        let pilot_bw = 30.0f32;
        let omega_n = 2.0 * PI * pilot_bw / sample_rate;
        let zeta = 0.707f32;
        let pll_alpha = 2.0 * zeta * omega_n;
        let pll_beta = omega_n * omega_n;

        // Gardner TED gain for symbol timing recovery
        let timing_gain = 0.01f32;
        let half_symbol = self.samples_per_symbol * 0.5;

        for &x in audio {
            // === Always process bandpass filters (IIR state must be continuous!) ===
            let pilot_filtered = self.pilot_bpf.process(x);
            let rds_filtered = self.rds_bpf2.process(self.rds_bpf1.process(x));

            // === 19 kHz Pilot PLL ===
            let pilot_error = pilot_filtered * (-self.pilot_phase.sin());
            let pilot_i = pilot_filtered * self.pilot_phase.cos();

            self.pilot_freq += pll_beta * pilot_error;
            self.pilot_phase += self.pilot_freq + pll_alpha * pilot_error;
            while self.pilot_phase > PI { self.pilot_phase -= 2.0 * PI; }
            while self.pilot_phase < -PI { self.pilot_phase += 2.0 * PI; }

            self.pilot_lock_energy = self.pilot_lock_energy * 0.9999 + pilot_i.abs() * 0.0001;
            self.pilot_locked = self.pilot_lock_energy > 1e-4;

            if !self.pilot_locked {
                continue;
            }

            // === 57 kHz BPSK Demod ===
            let ref_57k = (3.0 * self.pilot_phase).cos();
            let demod = rds_filtered * ref_57k * 2.0;

            // === 4th-order LPF at 2400 Hz ===
            let filtered = self.rds_lpf2.process(self.rds_lpf1.process(demod));

            // Diagnostics
            self.diag_bpf_sum += rds_filtered * rds_filtered;
            self.diag_demod_sum += filtered.abs();
            self.diag_sample_count += 1;

            // === Symbol clock at 2375 symbols/sec (with Gardner TED) ===
            self.symbol_clock += 1.0;
            self.symbol_integrator += filtered;

            // Gardner TED: sample at midpoint of symbol period
            if !self.mid_sampled && self.symbol_clock >= half_symbol {
                self.mid_sample = filtered;
                self.mid_sampled = true;
            }

            if self.symbol_clock >= self.samples_per_symbol {
                self.symbol_clock -= self.samples_per_symbol;
                self.total_symbols += 1;

                let symbol_val = self.symbol_integrator;

                // Gardner TED
                let ted_error = (symbol_val.signum() - self.last_symbol_val.signum()) * self.mid_sample;
                self.diag_timing_err_sum += ted_error;
                self.timing_offset = self.timing_offset * 0.95 + ted_error * timing_gain;
                self.timing_offset = self.timing_offset.clamp(-2.0, 2.0);
                self.symbol_clock += self.timing_offset;

                self.last_symbol_val = symbol_val;
                self.mid_sampled = false;
                self.symbol_integrator = 0.0;

                // Log first 200 raw symbol values for debugging
                if !self.diag_logged_symbols && self.diag_symbol_log.len() < 200 {
                    self.diag_symbol_log.push(symbol_val);
                    if self.diag_symbol_log.len() == 200 {
                        self.diag_logged_symbols = true;
                        // Log a compact representation of symbol values
                        let positive = self.diag_symbol_log.iter().filter(|&&v| v > 0.0).count();
                        let mag_sum: f32 = self.diag_symbol_log.iter().map(|v| v.abs()).sum();
                        let mag_avg = mag_sum / 200.0;
                        // Show first 52 symbols as +/- pattern (one RDS block = 26 bits = 52 PSK symbols)
                        let pattern: String = self.diag_symbol_log[..52].iter()
                            .map(|&v| if v > 0.0 { '+' } else { '-' })
                            .collect();
                        tracing::info!(
                            "RDS symbol dump: pos={}/200 mag_avg={:.4} first52=[{}]",
                            positive, mag_avg, pattern
                        );
                    }
                }

                // Diagnostic
                self.diag_integ_sum += symbol_val.abs();
                self.diag_integ_count += 1;

                // === Biphase decoding ===
                let biphase_value = (symbol_val - self.prev_psk_symbol) * 0.5;
                self.prev_psk_symbol = symbol_val;

                // Polarity detection
                let hist_idx = self.biphase_hist_idx;
                if hist_idx < 128 {
                    self.biphase_history[hist_idx] = biphase_value.abs();
                    self.biphase_hist_idx += 1;
                }

                if self.biphase_hist_idx >= 128 {
                    let mut even_sum = 0.0f32;
                    let mut odd_sum = 0.0f32;
                    for i in (0..128).step_by(2) {
                        even_sum += self.biphase_history[i];
                        odd_sum += self.biphase_history[i + 1];
                    }
                    // Log the ratio for debugging
                    self.diag_even_sum = even_sum;
                    self.diag_odd_sum = odd_sum;
                    if even_sum > odd_sum {
                        self.biphase_polarity = 0;
                    } else if odd_sum > even_sum {
                        self.biphase_polarity = 1;
                    }
                    self.biphase_hist_idx = 0;
                }

                let is_data_symbol = (self.biphase_clock % 2) == self.biphase_polarity;
                self.biphase_clock = self.biphase_clock.wrapping_add(1);

                if is_data_symbol {
                    let biphase_bit = biphase_value >= 0.0;
                    let data_bit = biphase_bit != self.prev_biphase_bit;
                    self.prev_biphase_bit = biphase_bit;

                    self.total_bits += 1;
                    self.push_bit(data_bit);
                }
            }
        }
    }

    fn push_bit(&mut self, bit: bool) {
        self.block_buf = ((self.block_buf << 1) | (bit as u32)) & 0x03FF_FFFF; // 26-bit window
        self.bits_in_block += 1;

        if !self.synced {
            // Try to sync: check if current 26 bits match any offset word
            let syn = rds_syndrome(self.block_buf);
            if syn == RDS_OFFSET_A as u16 {
                self.synced = true;
                self.group_idx = 0;
                self.bits_in_block = 0;
                self.group_buf[0] = (self.block_buf >> 10) as u16;
                self.group_idx = 1;
                self.good_blocks = 1;
                self.bad_blocks = 0;
                tracing::info!("RDS: sync acquired after {} bits, PI=0x{:04X}", self.total_bits, (self.block_buf >> 10) as u16);
            }
            return;
        }

        if self.bits_in_block < 26 { return; }
        self.bits_in_block = 0;

        let syn = rds_syndrome(self.block_buf);
        let expected_offset = match self.group_idx {
            0 => RDS_OFFSET_A,
            1 => RDS_OFFSET_B,
            2 => RDS_OFFSET_C, // also check C' below
            3 => RDS_OFFSET_D,
            _ => 0xFFFF,
        };

        let valid = syn == expected_offset
            || (self.group_idx == 2 && syn == RDS_OFFSET_C_PRIME);

        if valid {
            self.good_blocks += 1;
            self.group_buf[self.group_idx as usize] = (self.block_buf >> 10) as u16;
            self.group_idx += 1;

            if self.group_idx >= 4 {
                self.decode_group();
                self.group_idx = 0;
            }
        } else {
            self.bad_blocks += 1;
            if self.bad_blocks > 20 {
                // Lost sync
                self.synced = false;
                self.good_blocks = 0;
                self.bad_blocks = 0;
            }
            self.group_idx = 0; // restart group
        }
    }

    fn decode_group(&mut self) {
        let [a, b, _c, d] = self.group_buf;
        self.pi_code = a;
        let group_type = (b >> 12) & 0x0F;
        let version = (b >> 11) & 0x01; // 0=A, 1=B
        tracing::info!(
            "RDS group {}{}  PI=0x{:04X}  good={} bad={}",
            group_type, if version == 0 { "A" } else { "B" },
            a, self.good_blocks, self.bad_blocks
        );
        let new_pty = ((b >> 5) & 0x1F) as u8;
        if new_pty != self.pty {
            self.pty = new_pty;
            self.has_update = true;
        }

        match group_type {
            0 => {
                // Group 0A/0B: Programme Service Name
                let seg = (b & 0x03) as u8; // 0-3, 2 chars each
                let c1 = (d >> 8) as u8;
                let c2 = (d & 0xFF) as u8;
                let idx = seg as usize * 2;
                if idx + 1 < 8 {
                    // Only accept printable ASCII
                    if c1 >= 0x20 && c1 < 0x7F { self.ps_name[idx] = c1; }
                    if c2 >= 0x20 && c2 < 0x7F { self.ps_name[idx + 1] = c2; }
                    self.ps_segments |= 1 << seg;
                    let ps_str = String::from_utf8_lossy(&self.ps_name);
                    tracing::info!("RDS PS seg={} chars='{}{}'  ps_so_far=\"{}\"  segs=0x{:02X}",
                        seg, c1 as char, c2 as char, ps_str.trim(), self.ps_segments);
                    if self.ps_segments == 0x0F {
                        // All 4 segments received
                        if self.ps_name != self.last_sent_ps {
                            self.has_update = true;
                        }
                    }
                }
            }
            2 => {
                // Group 2A/2B: RadioText
                if version == 0 {
                    // 2A: 4 chars from blocks C and D
                    let seg = (b & 0x0F) as usize;
                    let idx = seg * 4;
                    let c = self.group_buf[2]; // block C
                    let chars = [
                        (c >> 8) as u8,
                        (c & 0xFF) as u8,
                        (d >> 8) as u8,
                        (d & 0xFF) as u8,
                    ];
                    for (i, &ch) in chars.iter().enumerate() {
                        if idx + i < 64 && ch >= 0x20 && ch < 0x7F {
                            self.radiotext[idx + i] = ch;
                        }
                    }
                    self.rt_segments |= 1 << seg;
                } else {
                    // 2B: 2 chars from block D
                    let seg = (b & 0x0F) as usize;
                    let idx = seg * 2;
                    let c1 = (d >> 8) as u8;
                    let c2 = (d & 0xFF) as u8;
                    if idx + 1 < 64 {
                        if c1 >= 0x20 && c1 < 0x7F { self.radiotext[idx] = c1; }
                        if c2 >= 0x20 && c2 < 0x7F { self.radiotext[idx + 1] = c2; }
                    }
                    self.rt_segments |= 1 << seg;
                }
                // Check for change
                if self.rt_segments != 0 && self.radiotext != self.last_sent_rt {
                    self.has_update = true;
                }
            }
            _ => {} // Ignore other group types for now
        }
    }

    /// Returns JSON update if any RDS data changed since last call.
    fn take_update(&mut self, stereo: bool) -> Option<String> {
        // Always send if stereo status or any RDS data is available
        if !self.has_update && self.ps_segments == 0 && !stereo {
            return None;
        }
        self.has_update = false;
        self.last_sent_ps = self.ps_name;
        self.last_sent_rt = self.radiotext;
        self.last_sent_pty = self.pty;

        let ps = String::from_utf8_lossy(&self.ps_name).to_string();
        let rt_len = self.radiotext.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1);
        let rt = String::from_utf8_lossy(&self.radiotext[..rt_len]).to_string();

        Some(format!(
            r#"{{"type":"rds","ps":"{}","rt":"{}","pi":"0x{:04X}","pty":{},"stereo":{}}}"#,
            ps.replace('"', "\\\""),
            rt.replace('"', "\\\""),
            self.pi_code,
            self.pty,
            stereo,
        ))
    }
}

impl Biquad {
    /// Bandpass filter centered at `center_hz` with given `bandwidth_hz`.
    fn bandpass(center_hz: f32, bandwidth_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * PI * center_hz / sample_rate;
        let sin_omega = omega.sin();
        let cos_omega = omega.cos();
        let alpha = sin_omega * (2.0f32.ln() / 2.0 * bandwidth_hz / center_hz * omega / sin_omega).sinh();
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: -2.0 * cos_omega / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

/// Per-client demodulator state.
struct ClientState {
    last_iq: Option<Complex32>,
    agc_level: f32,
    freq: u64,
    mode: DemodMode,
    bandwidth: u32,
    channel_rate: u32,
    output_rate: u32,
    raw_audio: bool,
    resampler: Resampler,
    /// CW BFO phase (continuous across frames)
    bfo_phase: f32,
    /// Cascaded biquad lowpass for audio bandwidth limiting (narrowband modes)
    audio_lpf1: Option<Biquad>,
    audio_lpf2: Option<Biquad>,
    /// FM de-emphasis filter
    deemph: Option<Biquad>,
    /// DC-blocking filter state for FM (removes DC offset → prevents AGC hum)
    dc_x1: f32,
    dc_y1: f32,
    /// Running phase for SSB/CW frequency correction (compensates FFT bin quantization)
    correction_phase: f64,
    /// SAM PLL phase accumulator
    sam_phase: f32,
    /// SAM PLL estimated frequency offset (rad/sample)
    sam_freq: f32,
    /// WFM: stereo pilot detected (19 kHz)
    stereo_detected: bool,
    /// WFM: pilot energy smoother
    pilot_energy: f32,
    /// WFM: RDS decoder
    rds_decoder: Option<RdsDecoder>,
    /// WFM: frame counter for periodic RDS updates
    rds_frame_count: u32,
    /// NFM: ZF-Filter vor dem Diskriminator (FIR, Kaiser ~80 dB), je I und Q
    iq_fir: Option<(Fir, Fir)>,
    /// AM/SSB-Regelung: verbleibende Halte-Rahmen
    agc_hang: u32,
    /// NFM: Audio highpass to remove CTCSS subaudible tones (< 300 Hz)
    audio_hpf: Option<Biquad>,
    /// SSB: durchlaufendes Seitenbandfilter (Mischer um die Bandmitte, komplexer FIR-Tiefpass, zurückmischen)
    ssb: Option<SsbFilter>,
}

/// SSB-Demodulation ohne Blockartefakte: Das gewünschte Seitenband [lo, hi] (USB positiv, LSB negativ) wird um seine
/// Mitte auf 0 Hz gemischt, mit einem komplexen FIR-Tiefpass (Kaiser, 127 Taps) ausgeschnitten und wieder hoch-
/// gemischt; der Realteil ist der Ton. Mischerphase und Filterspeicher laufen über die Rahmen hinweg weiter.
struct SsbFilter {
    fi: Fir,
    fq: Fir,
    phase: f64,
    step: f64,
}

impl SsbFilter {
    fn new(upper: bool, lo_hz: f32, hi_hz: f32, rate: f32) -> Self {
        let center = (lo_hz + hi_hz) / 2.0 * if upper { 1.0 } else { -1.0 };
        let cutoff = ((hi_hz - lo_hz) / 2.0).max(100.0).min(rate * 0.45);
        let taps = design_lowpass_fir(127, cutoff / rate, 7.86);
        Self { fi: Fir::new(taps.clone()), fq: Fir::new(taps), phase: 0.0, step: 2.0 * std::f64::consts::PI * center as f64 / rate as f64 }
    }
    fn process(&mut self, iq: &[Complex32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(iq.len());
        for s in iq {
            let (sn, cs) = self.phase.sin_cos();
            let down = s * Complex32::new(cs as f32, -sn as f32);           // Bandmitte → 0 Hz
            let f = Complex32::new(self.fi.process(down.re), self.fq.process(down.im));
            let up = f * Complex32::new(cs as f32, sn as f32);               // zurück an die Stelle → Ton = Realteil
            out.push(2.0 * up.re);
            self.phase += self.step;
            if self.phase > 2.0 * std::f64::consts::PI { self.phase -= 2.0 * std::f64::consts::PI; }
        }
        out
    }
}

impl ClientState {
    /// Create audio bandwidth filter based on mode and bandwidth.
    fn make_audio_filter(mode: DemodMode, bandwidth: u32, channel_rate: u32) -> (Option<Biquad>, Option<Biquad>) {
        let ch_rate = channel_rate as f32;
        let cutoff = match mode {
            // SSB: das Seitenbandfilter (SsbFilter) begrenzt schon exakt auf [lo, hi]; ein zusätzlicher Tiefpass bei hi
            // nähme nur Höhen weg (dumpf). Deshalb keiner.
            DemodMode::Usb | DemodMode::Lsb => return (None, None),
            // AM/SAM: RF bandwidth is double the audio bandwidth
            DemodMode::Am | DemodMode::Sam => (bandwidth as f32 * 0.5).min(ch_rate * 0.45),
            // CW: narrow around the 700 Hz BFO tone
            DemodMode::Cw => 1200.0f32.min(ch_rate * 0.45),
            // NFM: voice audio up to ~4 kHz, cut discriminator noise above
            DemodMode::Fm => 4000.0f32.min(ch_rate * 0.45),
            // WFM: broadcast FM audio up to ~15 kHz
            DemodMode::Wfm => 15000.0f32.min(ch_rate * 0.45),
        };

        // Don't filter if cutoff is near Nyquist (no benefit)
        if cutoff >= ch_rate * 0.4 {
            return (None, None);
        }

        (
            Some(Biquad::lowpass(cutoff, ch_rate)),
            Some(Biquad::lowpass(cutoff, ch_rate)),
        )
    }

    /// Create FM de-emphasis filter. Returns None for non-FM modes.
    fn make_deemphasis(mode: DemodMode, channel_rate: u32) -> Option<Biquad> {
        match mode {
            // NFM: no de-emphasis. Amateur radio / scanner signals don't use pre-emphasis.
            DemodMode::Fm => None,
            // WFM: 50µs (Europe/worldwide) — broadcast FM uses pre-emphasis
            DemodMode::Wfm => Some(Biquad::deemphasis(50.0, channel_rate as f32)),
            _ => None,
        }
    }

    /// ZF-Filter für NFM: FIR-Tiefpass bei bw/2 auf Kanalrate (63 Taps, Kaiser β 7,86 ≈ 80 dB Sperrdämpfung).
    /// Unterdrückt Nachbarkanäle und Breitbandrauschen vor dem Diskriminator. None, wenn der Kanal ohnehin
    /// kaum breiter als die Bandbreite ist.
    fn make_iq_filter(mode: DemodMode, bandwidth: u32, channel_rate: u32) -> Option<(Fir, Fir)> {
        if mode != DemodMode::Fm {
            return None;
        }
        let ch_rate = channel_rate as f32;
        let cutoff = (bandwidth as f32 * 0.5).min(ch_rate * 0.45);
        if cutoff >= ch_rate * 0.4 {
            return None;
        }
        let taps = design_lowpass_fir(63, cutoff / ch_rate, 7.86);
        Some((Fir::new(taps.clone()), Fir::new(taps)))
    }

    /// Create audio highpass to remove CTCSS subaudible tones (67-254 Hz).
    fn make_audio_hpf(mode: DemodMode, channel_rate: u32) -> Option<Biquad> {
        if mode == DemodMode::Fm {
            Some(Biquad::highpass(300.0, channel_rate as f32))
        } else {
            None
        }
    }
}

pub struct Demodulator {
    states: HashMap<u64, ClientState>,
    /// Cached FFT planner — rustfft caches plans internally, but we avoid
    /// re-creating the planner object on every demod_ssb call (Pi 4 perf).
    fft_planner: FftPlanner<f32>,
}

impl Demodulator {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            fft_planner: FftPlanner::new(),
        }
    }

    pub fn remove_client(&mut self, client_id: u64) {
        self.states.remove(&client_id);
    }

    /// Get RDS update JSON for a WFM client, if there's new data.
    /// Called periodically (e.g. every 25 frames = ~1s).
    pub fn get_rds_update(&mut self, client_id: u64) -> Option<String> {
        let state = self.states.get_mut(&client_id)?;
        if state.rds_frame_count < 25 { return None; } // Only check once per ~second
        state.rds_frame_count = 0;
        let stereo = state.stereo_detected;
        if let Some(rds) = &mut state.rds_decoder {
            // Periodic diagnostics (every ~1s)
            let bpf_rms = if rds.diag_sample_count > 0 {
                (rds.diag_bpf_sum / rds.diag_sample_count as f32).sqrt()
            } else { 0.0 };
            let demod_avg = if rds.diag_sample_count > 0 {
                rds.diag_demod_sum / rds.diag_sample_count as f32
            } else { 0.0 };
            let integ_avg = if rds.diag_integ_count > 0 {
                rds.diag_integ_sum / rds.diag_integ_count as f32
            } else { 0.0 };
            let ratio = if rds.diag_odd_sum > 0.001 { rds.diag_even_sum / rds.diag_odd_sum } else { 0.0 };
            tracing::info!(
                "RDS diag: locked={} sym={} bits={} sync={} good={} bad={} | bpf_rms={:.6} demod={:.6} sym_avg={:.4} timing={:.3} pol={} e/o={:.2} (e={:.1} o={:.1})",
                rds.pilot_locked, rds.total_symbols, rds.total_bits, rds.synced,
                rds.good_blocks, rds.bad_blocks,
                bpf_rms, demod_avg, integ_avg,
                rds.timing_offset, rds.biphase_polarity,
                ratio, rds.diag_even_sum, rds.diag_odd_sum,
            );
            // Reset diagnostic accumulators
            rds.diag_bpf_sum = 0.0;
            rds.diag_demod_sum = 0.0;
            rds.diag_integ_sum = 0.0;
            rds.diag_integ_count = 0;
            rds.diag_sample_count = 0;
            rds.diag_timing_err_sum = 0.0;
            rds.take_update(stereo)
        } else if stereo {
            // No RDS but stereo detected — still send stereo status
            Some(format!(r#"{{"type":"rds","ps":"","rt":"","pi":"0x0000","pty":0,"stereo":true}}"#))
        } else {
            None
        }
    }

    /// Demodulate narrowband IQ to int16 audio.
    ///
    /// `output_rate`: target sample rate (48000 for browser, 22050 for decoders)
    /// `bandwidth`: RF bandwidth in Hz (used for audio lowpass filter)
    /// `raw_audio`: if true, skip AGC (for machine decoders like multimon-ng)
    /// `residual_hz`: frequency error from FFT bin quantization (from channelizer)
    pub fn demodulate(
        &mut self,
        channel_iq: &[Complex32],
        channel_rate: f32,
        mode: DemodMode,
        client_id: u64,
        tune_freq: u64,
        bandwidth: u32,
        output_rate: u32,
        raw_audio: bool,
        residual_hz: f64,
        agc_mode: AgcMode,
        pass_lo: u32,
    ) -> Option<Vec<i16>> {
        if channel_iq.len() < 4 {
            return None;
        }
        let ssb_filter = |m: DemodMode, bw: u32, rate: u32| match m {
            DemodMode::Usb => Some(SsbFilter::new(true, pass_lo as f32, bw as f32, rate as f32)),
            DemodMode::Lsb => Some(SsbFilter::new(false, pass_lo as f32, bw as f32, rate as f32)),
            _ => None,
        };

        let ch_rate = channel_rate as u32;
        let out_rate = output_rate;

        let state = self.states.entry(client_id).or_insert_with(|| {
            let (lpf1, lpf2) = ClientState::make_audio_filter(mode, bandwidth, ch_rate);
            ClientState {
                last_iq: None,
                agc_level: 0.1,
                freq: tune_freq,
                mode,
                bandwidth,
                channel_rate: ch_rate,
                output_rate: out_rate,
                raw_audio,
                resampler: Resampler::new(ch_rate, out_rate),
                bfo_phase: 0.0,
                audio_lpf1: lpf1,
                audio_lpf2: lpf2,
                deemph: ClientState::make_deemphasis(mode, ch_rate),
                dc_x1: 0.0,
                dc_y1: 0.0,
                correction_phase: 0.0,
                sam_phase: 0.0,
                sam_freq: 0.0,
                stereo_detected: false,
                pilot_energy: 0.0,
                rds_decoder: if mode == DemodMode::Wfm { Some(RdsDecoder::new(ch_rate as f32)) } else { None },
                rds_frame_count: 0,
                iq_fir: ClientState::make_iq_filter(mode, bandwidth, ch_rate),
                agc_hang: 0,
                audio_hpf: ClientState::make_audio_hpf(mode, ch_rate),
                ssb: ssb_filter(mode, bandwidth, ch_rate),
            }
        });

        // Reset state on frequency/mode/rate/bandwidth change
        if tune_freq != state.freq || mode != state.mode || ch_rate != state.channel_rate
            || out_rate != state.output_rate || bandwidth != state.bandwidth
        {
            let (lpf1, lpf2) = ClientState::make_audio_filter(mode, bandwidth, ch_rate);
            state.last_iq = None;
            state.agc_level = 0.1;
            state.freq = tune_freq;
            state.mode = mode;
            state.bandwidth = bandwidth;
            state.channel_rate = ch_rate;
            state.output_rate = out_rate;
            state.raw_audio = raw_audio;
            state.resampler = Resampler::new(ch_rate, out_rate);
            state.bfo_phase = 0.0;
            state.audio_lpf1 = lpf1;
            state.audio_lpf2 = lpf2;
            state.deemph = ClientState::make_deemphasis(mode, ch_rate);
            state.dc_x1 = 0.0;
            state.dc_y1 = 0.0;
            state.correction_phase = 0.0;
            state.sam_phase = 0.0;
            state.sam_freq = 0.0;
            state.stereo_detected = false;
            state.pilot_energy = 0.0;
            state.rds_decoder = if mode == DemodMode::Wfm { Some(RdsDecoder::new(ch_rate as f32)) } else { None };
            state.rds_frame_count = 0;
            state.iq_fir = ClientState::make_iq_filter(mode, bandwidth, ch_rate);
            state.agc_hang = 0;
            state.audio_hpf = ClientState::make_audio_hpf(mode, ch_rate);
            state.ssb = ssb_filter(mode, bandwidth, ch_rate);
        }

        // Apply frequency correction for SSB/CW modes.
        // The FFT channelizer quantizes to the nearest bin, causing up to ±bin_hz/2
        // frequency error. For FM/AM this is negligible, but for SSB it shifts all
        // audio tones, making voice sound wrong. We correct by mixing with a complex
        // exponential at the residual frequency.
        let corrected_iq;
        let demod_iq = if matches!(mode, DemodMode::Usb | DemodMode::Lsb | DemodMode::Cw)
            && residual_hz.abs() > 0.1
        {
            let phase_inc = -2.0 * std::f64::consts::PI * residual_hz / channel_rate as f64;
            let mut phase = state.correction_phase;
            corrected_iq = channel_iq
                .iter()
                .map(|s| {
                    let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
                    phase += phase_inc;
                    s * rot
                })
                .collect::<Vec<_>>();
            // Keep phase in [0, 2π) to avoid float precision loss over time
            state.correction_phase = phase % (2.0 * std::f64::consts::PI);
            &corrected_iq
        } else {
            channel_iq
        };

        // NFM: ZF-Filter (FIR) vor dem Diskriminator — begrenzt den 24-kHz-Kanal auf die Bandbreite,
        // sonst rauschen und zischen Nachbarkanal und Breitbandrauschen im Diskriminator.
        let filtered_iq;
        let demod_iq = if mode == DemodMode::Fm && state.iq_fir.is_some() {
            let (fi, fq) = state.iq_fir.as_mut().unwrap();
            filtered_iq = demod_iq
                .iter()
                .map(|s| Complex32::new(fi.process(s.re), fq.process(s.im)))
                .collect::<Vec<_>>();
            &filtered_iq
        } else {
            demod_iq
        };

        // Demodulate to float audio
        let mut audio = match mode {
            DemodMode::Fm | DemodMode::Wfm => demod_fm(demod_iq, state),
            DemodMode::Am => demod_am(demod_iq, state),
            DemodMode::Sam => demod_sam(demod_iq, channel_rate, state),
            DemodMode::Usb | DemodMode::Lsb => match state.ssb.as_mut() {
                Some(f) => f.process(demod_iq),
                None => demod_ssb(demod_iq, mode == DemodMode::Usb, &mut self.fft_planner),
            },
            DemodMode::Cw => demod_cw(demod_iq, channel_rate, state, &mut self.fft_planner),
        };

        if audio.is_empty() {
            return None;
        }

        // WFM: Stereo pilot detection + RDS decoding (before de-emphasis/filtering!)
        if mode == DemodMode::Wfm && ch_rate >= 76000 {
            // Goertzel filter for 19 kHz pilot tone
            let pilot_e = goertzel_energy(&audio, channel_rate, 19000.0);
            state.pilot_energy = state.pilot_energy * 0.9 + pilot_e * 0.1;
            state.stereo_detected = state.pilot_energy > 1e-8;

            // RDS decoding (57 kHz subcarrier)
            if let Some(rds) = &mut state.rds_decoder {
                if rds.total_bits == 0 && rds.diag_sample_count == 0 {
                    tracing::info!("RDS: starting decoder at channel_rate={} Hz, audio_len={}", ch_rate, audio.len());
                }
                rds.process(&audio, channel_rate);
            }
            state.rds_frame_count += 1;
        }

        // DC-blocking filter for FM modes: y[n] = x[n] - x[n-1] + R * y[n-1]
        // Removes DC offset from FM discriminator that causes AGC hum/pumping.
        // R=0.998 gives ~30 Hz highpass at typical channel rates.
        if matches!(mode, DemodMode::Fm | DemodMode::Wfm) {
            let r = 0.998f32;
            let mut x1 = state.dc_x1;
            let mut y1 = state.dc_y1;
            for s in &mut audio {
                let x = *s;
                let y = x - x1 + r * y1;
                x1 = x;
                y1 = y;
                *s = y;
            }
            state.dc_x1 = x1;
            state.dc_y1 = y1;
        }

        // Apply FM de-emphasis (reduces high-frequency noise from discriminator).
        // Must be applied before the audio lowpass for correct frequency response.
        // Skip for raw_audio (decoder plugins need the full MPX signal including
        // 19 kHz pilot, 38 kHz stereo subcarrier, and 57 kHz RDS subcarrier).
        if !raw_audio {
            if let Some(deemph) = &mut state.deemph {
                for s in &mut audio {
                    *s = deemph.process(*s);
                }
            }
        }

        // Apply audio bandwidth filter (skip for raw_audio — decoders need full bandwidth).
        if !raw_audio {
            if let (Some(lpf1), Some(lpf2)) = (&mut state.audio_lpf1, &mut state.audio_lpf2) {
                for s in &mut audio {
                    *s = lpf1.process(*s);
                    *s = lpf2.process(*s);
                }
            }
        }

        // NFM: Highpass to remove CTCSS subaudible tones (67-254 Hz) and residual DC.
        // Applied after LPF so it doesn't affect WFM or decoder pipelines.
        if !raw_audio {
            if let Some(hpf) = &mut state.audio_hpf {
                for s in &mut audio {
                    *s = hpf.process(*s);
                }
            }
        }

        // Resample to target rate using stateful resampler (maintains continuity)
        let mut audio = state.resampler.process(&audio);

        if audio.is_empty() {
            return None;
        }

        if raw_audio {
            // Raw mode for decoders: no AGC, just scale discriminator output to int16
            // FM discriminator outputs [-1, +1], scale to ~±8000 (matching rtl_fm levels)
            Some(
                audio
                    .iter()
                    .map(|&s| (s.clamp(-0.95, 0.95) * 8000.0) as i16)
                    .collect(),
            )
        } else {
            // Hörer-Ton. FM (NFM/WFM): feste Verstärkung aus dem Nennhub – der Diskriminator liefert schon eine vom
            // Signalpegel unabhängige Lautstärke, eine Regelung würde nur in Pausen das Rauschen hochziehen („pumpen“).
            // ±2,5 kHz (NFM) bzw. ±75 kHz (WFM) Hub → Spitzenwert 0,4 (−8 dBFS). arg/π: ±Kanalrate/2 ↔ ±1.
            if matches!(mode, DemodMode::Fm | DemodMode::Wfm) {
                let dev_ref = if mode == DemodMode::Wfm { 75_000.0 } else { 2_500.0 };
                let gain = 0.4 * (state.channel_rate as f32 / 2.0) / dev_ref;
                for s in &mut audio { *s *= gain; }
            } else {
                // AM/SSB/CW: Regelung in dB. Angriff sofort (ein Rahmen = 20 ms, sonst übersteuert ein starkes Signal
                // sekundenlang), Haltezeit 1 s, dann Lösen mit fester Rate in dB je Rahmen – vorher lief das Lösen linear
                // auf einen riesigen Zielwert zu und riss in Sprechpausen in zwei Rahmen das Rauschen hoch (Pumpen).
                let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
                if rms > 1e-6 {
                    let target_gain = (0.15 / rms).min(AGC_MAX_GAIN);
                    match agc_mode {
                        AgcMode::Off => {
                            for s in &mut audio { *s *= 0.5; }
                        }
                        _ => {
                            let release_db = match agc_mode { AgcMode::Fast => 0.3, AgcMode::Slow => 0.04, _ => 0.1 };   // je 20-ms-Rahmen
                            let old = state.agc_level;
                            if target_gain < state.agc_level {
                                state.agc_level = target_gain;
                                state.agc_hang = AGC_HANG_FRAMES;
                            } else if state.agc_hang > 0 {
                                state.agc_hang -= 1;
                            } else {
                                state.agc_level = (state.agc_level * 10f32.powf(release_db / 20.0)).min(target_gain);
                            }
                            // Verstärkung über den Rahmen hinweg gleitend von alt nach neu, kein Sprung alle 20 ms (Zipper)
                            let n = audio.len().max(1) as f32;
                            for (i, s) in audio.iter_mut().enumerate() { *s *= old + (state.agc_level - old) * (i as f32 + 1.0) / n; }
                        }
                    }
                }
            }

            // Clip and convert to i16
            Some(
                audio
                    .iter()
                    .map(|&s| (s.clamp(-0.95, 0.95) * 32767.0) as i16)
                    .collect(),
            )
        }
    }
}

/// FM demod with phase continuity between frames.
fn demod_fm(iq: &[Complex32], state: &mut ClientState) -> Vec<f32> {
    let mut audio = Vec::with_capacity(iq.len());

    if let Some(prev) = state.last_iq {
        // First sample uses previous frame's last sample
        let diff = iq[0] * prev.conj();
        audio.push(diff.arg() / PI);
    } else {
        // No previous frame — duplicate first sample
        if iq.len() >= 2 {
            let diff = iq[1] * iq[0].conj();
            audio.push(diff.arg() / PI);
        }
    }

    for i in 1..iq.len() {
        let diff = iq[i] * iq[i - 1].conj();
        audio.push(diff.arg() / PI);
    }

    state.last_iq = iq.last().copied();
    audio
}

/// AM demod: envelope detection with stateful IIR DC-blocker.
/// Uses the same DC-blocking filter as FM for click-free frame boundaries.
fn demod_am(iq: &[Complex32], state: &mut ClientState) -> Vec<f32> {
    let mut audio: Vec<f32> = iq.iter().map(|s| s.norm()).collect();
    // Stateful DC-blocker: y[n] = x[n] - x[n-1] + R * y[n-1]
    let r = 0.998f32;
    for s in &mut audio {
        let x = *s;
        let y = x - state.dc_x1 + r * state.dc_y1;
        state.dc_x1 = x;
        state.dc_y1 = y;
        *s = y;
    }
    audio
}

/// SAM (Synchronous AM) demod: Costas-loop PLL tracks the AM carrier,
/// then coherent detection extracts audio with better fading resistance than envelope AM.
fn demod_sam(iq: &[Complex32], channel_rate: f32, state: &mut ClientState) -> Vec<f32> {
    // PLL loop parameters for ~30 Hz loop bandwidth
    // zeta = 0.707 (critically damped), omega_n = 2π * 30
    let loop_bw = 30.0f32;
    let omega_n = 2.0 * PI * loop_bw / channel_rate;
    let zeta = 0.707f32;
    let alpha = 2.0 * zeta * omega_n;
    let beta = omega_n * omega_n;

    let mut audio = Vec::with_capacity(iq.len());
    let mut phase = state.sam_phase;
    let mut freq = state.sam_freq;

    for &sample in iq {
        // Rotate input by -phase to align carrier to DC
        let rot = Complex32::new(phase.cos(), -phase.sin());
        let coherent = sample * rot;

        // Audio output is the real part (demodulated AM)
        audio.push(coherent.re);

        // Costas-loop phase error discriminator
        let error = coherent.re * coherent.im;

        // Update PLL
        freq += beta * error;
        // Clamp frequency to prevent runaway
        freq = freq.clamp(-0.5, 0.5);
        phase += freq + alpha * error;

        // Wrap phase to [-π, π)
        if phase > PI { phase -= 2.0 * PI; }
        if phase < -PI { phase += 2.0 * PI; }
    }

    state.sam_phase = phase;
    state.sam_freq = freq;

    // Stateful DC-blocker (same as AM)
    let r = 0.998f32;
    for s in &mut audio {
        let x = *s;
        let y = x - state.dc_x1 + r * state.dc_y1;
        state.dc_x1 = x;
        state.dc_y1 = y;
        *s = y;
    }

    audio
}

/// SSB demod with proper sideband selection via Hilbert transform (FFT method).
///
/// The channelizer extracts a symmetric band around the carrier. We need to
/// select only one sideband to avoid noise from the unwanted side folding in.
///
/// For USB: zero negative frequencies, keep positive → Re() gives audio.
/// For LSB: zero positive frequencies, keep negative → Re() gives audio.
///
/// `upper`: true for USB, false for LSB.
fn demod_ssb(iq: &[Complex32], upper: bool, planner: &mut FftPlanner<f32>) -> Vec<f32> {
    let n = iq.len();
    if n < 4 {
        return iq.iter().map(|s| s.re).collect();
    }

    // FFT the channel IQ
    let fft = planner.plan_fft_forward(n);
    let mut freq: Vec<Complex32> = iq.to_vec();
    let scratch_len = fft.get_inplace_scratch_len();
    let mut scratch = vec![Complex32::new(0.0, 0.0); scratch_len];
    fft.process_with_scratch(&mut freq, &mut scratch);

    // Zero out the unwanted sideband.
    // FFT layout: [DC, +1, +2, ..., +N/2, -N/2+1, ..., -2, -1]
    // Positive freq bins: 1..N/2
    // Negative freq bins: N/2+1..N-1
    let half = n / 2;
    if upper {
        // USB: keep positive frequencies (bins 1..half), zero negative (bins half+1..n-1)
        // DC (bin 0) and Nyquist (bin half) kept at half amplitude
        freq[0] *= 0.5;
        if half < n {
            freq[half] *= 0.5;
        }
        for i in (half + 1)..n {
            freq[i] = Complex32::new(0.0, 0.0);
        }
    } else {
        // LSB: keep negative frequencies (bins half+1..n-1), zero positive (bins 1..half-1)
        freq[0] *= 0.5;
        if half < n {
            freq[half] *= 0.5;
        }
        for i in 1..half {
            freq[i] = Complex32::new(0.0, 0.0);
        }
    }

    // iFFT back to time domain
    let ifft = planner.plan_fft_inverse(n);
    let iscratch_len = ifft.get_inplace_scratch_len();
    if scratch.len() < iscratch_len {
        scratch.resize(iscratch_len, Complex32::new(0.0, 0.0));
    }
    ifft.process_with_scratch(&mut freq, &mut scratch);

    // Normalize (rustfft doesn't normalize) and take Re.
    // Factor 2.0 compensates for zeroing half the spectrum.
    let inv = 2.0 / n as f32;
    freq.iter().map(|s| s.re * inv).collect()
}

/// CW demod: 700 Hz BFO + USB sideband selection.
///
/// The BFO shifts the CW signal from DC to 700 Hz, then USB sideband
/// selection filters out noise from the lower sideband. This matches
/// how OpenWebRX and other SDR programs handle CW.
fn demod_cw(iq: &[Complex32], rate: f32, state: &mut ClientState, planner: &mut FftPlanner<f32>) -> Vec<f32> {
    let bfo = 700.0f32;
    let phase_inc = 2.0 * PI * bfo / rate;
    let mut phase = state.bfo_phase;

    // Mix with BFO (shifts CW signal to 700 Hz)
    let mixed: Vec<Complex32> = iq
        .iter()
        .map(|s| {
            let osc = Complex32::new(phase.cos(), phase.sin());
            phase += phase_inc;
            if phase > 2.0 * PI {
                phase -= 2.0 * PI;
            }
            s * osc
        })
        .collect();

    state.bfo_phase = phase;

    // Apply USB sideband selection to reject noise from lower sideband
    demod_ssb(&mixed, true, planner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fm_demod() {
        let mut demod = Demodulator::new();
        // Generate FM signal: constant frequency offset
        let rate = 25000.0f32;
        let freq = 1000.0f32;
        let iq: Vec<Complex32> = (0..1000)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, 2.0 * PI * freq * t).exp()
            })
            .collect();

        let result = demod.demodulate(&iq, rate, DemodMode::Fm, 1, 100_000_000, 12500, 48000, false, 0.0, AgcMode::Medium, 300);
        assert!(result.is_some());
        let audio = result.unwrap();
        assert!(!audio.is_empty());
    }

    #[test]
    fn test_am_demod() {
        let iq: Vec<Complex32> = (0..100)
            .map(|i| Complex32::new(0.5 + 0.3 * (i as f32 * 0.1).sin(), 0.0))
            .collect();
        let mut state = ClientState {
            last_iq: None,
            agc_level: 0.1,
            freq: 100_000_000,
            mode: DemodMode::Am,
            bandwidth: 6000,
            channel_rate: 12000,
            output_rate: 48000,
            raw_audio: false,
            resampler: Resampler::new(12000, 48000),
            bfo_phase: 0.0,
            audio_lpf1: None,
            audio_lpf2: None,
            deemph: None,
            dc_x1: 0.0,
            dc_y1: 0.0,
            correction_phase: 0.0,
            sam_phase: 0.0,
            sam_freq: 0.0,
            stereo_detected: false,
            pilot_energy: 0.0,
            rds_decoder: None,
            rds_frame_count: 0,
            iq_fir: None,
            agc_hang: 0,
            audio_hpf: None,
            ssb: None,
        };
        let audio = demod_am(&iq, &mut state);
        assert_eq!(audio.len(), 100);
    }

    #[test]
    fn test_fm_frame_continuity() {
        let mut demod = Demodulator::new();
        let rate = 25000.0f32;
        let freq = 1000.0f32;

        // Two frames of the same FM tone
        let iq1: Vec<Complex32> = (0..500)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, 2.0 * PI * freq * t).exp()
            })
            .collect();
        let iq2: Vec<Complex32> = (500..1000)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, 2.0 * PI * freq * t).exp()
            })
            .collect();

        let r1 = demod.demodulate(&iq1, rate, DemodMode::Fm, 1, 100_000_000, 12500, 48000, false, 0.0, AgcMode::Medium, 300).unwrap();
        let r2 = demod.demodulate(&iq2, rate, DemodMode::Fm, 1, 100_000_000, 12500, 48000, false, 0.0, AgcMode::Medium, 300).unwrap();

        // Check continuity at boundary
        if let (Some(&a), Some(&b)) = (r1.last(), r2.first()) {
            let diff = (b as i32 - a as i32).unsigned_abs();
            assert!(diff < 2000, "Audio discontinuity at frame boundary: {} -> {} (diff={})", a, b, diff);
        }
    }

    /// USB-Demodulation mit Seitenbandfilter: Töne im Durchlassbereich (500 Hz, 2 kHz) kommen durch, der Ton im
    /// unteren Seitenband (−1 kHz) und der Ton unter der Bandkante (100 Hz) nicht. Rahmen von 20 ms wie im Betrieb,
    /// gemessen über die letzten Rahmen (Filter eingeschwungen), Ton über die Rahmengrenzen hinweg stetig.
    #[test]
    fn usb_filter_seitenband_und_bass() {
        let mut demod = Demodulator::new();
        let rate = 8000.0f32; let frame = 160usize; let n = frame * 40;
        let iq: Vec<Complex32> = (0..n).map(|i| {
            let t = i as f32 / rate;
            let mut s = Complex32::new(0.0, 0.0);
            for &f in &[500.0f32, 2000.0, -1000.0, 100.0] { s += Complex32::new(0.0, 2.0 * PI * f * t).exp() * 0.2; }
            s
        }).collect();
        let mut audio: Vec<f32> = Vec::new();
        for c in iq.chunks(frame) {
            let out = demod.demodulate(c, rate, DemodMode::Usb, 7, 144_260_000, 2700, 8000, false, 0.0, AgcMode::Off, 300).unwrap();
            audio.extend(out.iter().map(|&v| v as f32 / 32767.0));
        }
        let tail = &audio[audio.len() - frame * 10..];
        let e = |f: f32| goertzel_energy(tail, 8000.0, f);
        let (e500, e2000, e1000, e100) = (e(500.0), e(2000.0), e(1000.0), e(100.0));
        assert!(e1000 < e500 * 0.01, "LSB-Ton nicht unterdrückt: {} vs {}", e1000, e500);
        assert!(e100 < e500 * 0.05, "Bass unter 300 Hz nicht unterdrückt: {} vs {}", e100, e500);
        assert!(e2000 > e500 * 0.3, "2 kHz zu schwach: {} vs {}", e2000, e500);
        // keine Sprünge an den Rahmengrenzen: größte Differenz benachbarter Werte bleibt klein (reine Töne ≤ 2 kHz)
        let maxstep = tail.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(maxstep < 0.9, "Knackser an Rahmengrenzen: {}", maxstep);
    }

    /// Regelung: ein starkes Signal nach Rauschen darf nicht sekundenlang übersteuern – ab dem ersten Rahmen unter Vollpegel
    #[test]
    fn agc_sofortiger_angriff() {
        let mut demod = Demodulator::new();
        let rate = 8000.0f32; let frame = 160usize;
        let tone = |amp: f32, i0: usize| -> Vec<Complex32> { (0..frame).map(|i| Complex32::new(0.0, 2.0 * PI * 1000.0 * (i0 + i) as f32 / rate).exp() * amp).collect() };
        for k in 0..100 { let c = tone(0.001, k * frame); demod.demodulate(&c, rate, DemodMode::Usb, 9, 144_260_000, 2700, 8000, false, 0.0, AgcMode::Medium, 300); }   // leise: Regelung dreht auf
        let mut clipped = 0usize;
        for k in 100..105 {
            let c = tone(1.0, k * frame);
            let out = demod.demodulate(&c, rate, DemodMode::Usb, 9, 144_260_000, 2700, 8000, false, 0.0, AgcMode::Medium, 300).unwrap();
            clipped += out.iter().filter(|&&v| v.abs() >= 31000).count();
        }
        assert_eq!(clipped, 0, "Übersteuerung nach lautem Einsatz");
    }

    #[test]
    fn test_ssb_demod_with_filter() {
        let mut demod = Demodulator::new();
        let rate = 12000.0f32;
        // Generate a 1 kHz USB tone (positive frequency = upper sideband)
        let iq: Vec<Complex32> = (0..1200)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, 2.0 * PI * 1000.0 * t).exp()
            })
            .collect();

        let result = demod.demodulate(&iq, rate, DemodMode::Usb, 1, 100_000_000, 2700, 48000, false, 0.0, AgcMode::Medium, 300);
        assert!(result.is_some());
        let audio = result.unwrap();
        assert!(!audio.is_empty());
    }

    #[test]
    fn test_ssb_sideband_selection() {
        // USB should pass positive frequencies and reject negative
        let mut planner = FftPlanner::new();
        let n = 256;
        let rate = 12000.0f32;
        // Positive frequency tone at +1 kHz (USB signal)
        let pos_tone: Vec<Complex32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, 2.0 * PI * 1000.0 * t).exp()
            })
            .collect();
        // Negative frequency tone at -1 kHz (LSB signal)
        let neg_tone: Vec<Complex32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                Complex32::new(0.0, -2.0 * PI * 1000.0 * t).exp()
            })
            .collect();

        let usb_from_pos = demod_ssb(&pos_tone, true, &mut planner);
        let usb_from_neg = demod_ssb(&neg_tone, true, &mut planner);
        let lsb_from_neg = demod_ssb(&neg_tone, false, &mut planner);
        let lsb_from_pos = demod_ssb(&pos_tone, false, &mut planner);

        // USB should pass positive tone
        let usb_pos_energy: f32 = usb_from_pos.iter().map(|s| s * s).sum::<f32>() / n as f32;
        // USB should reject negative tone
        let usb_neg_energy: f32 = usb_from_neg.iter().map(|s| s * s).sum::<f32>() / n as f32;
        // LSB should pass negative tone
        let lsb_neg_energy: f32 = lsb_from_neg.iter().map(|s| s * s).sum::<f32>() / n as f32;
        // LSB should reject positive tone
        let lsb_pos_energy: f32 = lsb_from_pos.iter().map(|s| s * s).sum::<f32>() / n as f32;

        assert!(usb_pos_energy > usb_neg_energy * 100.0,
            "USB should pass +1kHz but reject -1kHz: pos={:.4} neg={:.4}", usb_pos_energy, usb_neg_energy);
        assert!(lsb_neg_energy > lsb_pos_energy * 100.0,
            "LSB should pass -1kHz but reject +1kHz: neg={:.4} pos={:.4}", lsb_neg_energy, lsb_pos_energy);
    }

    #[test]
    fn test_rds_syndrome() {
        // Encode a known PI code with offset A, then verify syndrome matches
        let pi_code: u16 = 0xD314;
        let block = rds_encode(pi_code, RDS_OFFSET_A);
        let syn = rds_syndrome(block);
        assert_eq!(syn, RDS_OFFSET_A, "Syndrome of valid block A should equal OFFSET_A");

        // Test all 4 offset words
        for (data, offset, name) in [
            (0xD314u16, RDS_OFFSET_A, "A"),
            (0x2400u16, RDS_OFFSET_B, "B"),
            (0x1234u16, RDS_OFFSET_C, "C"),
            (0xABCDu16, RDS_OFFSET_D, "D"),
        ] {
            let block = rds_encode(data, offset);
            let syn = rds_syndrome(block);
            assert_eq!(syn, offset, "Syndrome of valid block {} should equal offset", name);
        }

        // Syndrome of corrupted block should NOT match
        let block = rds_encode(0xD314, RDS_OFFSET_A);
        let corrupted = block ^ 1; // flip LSB
        let syn = rds_syndrome(corrupted);
        assert_ne!(syn, RDS_OFFSET_A, "Corrupted block should not match offset A");
    }

    #[test]
    fn test_frequency_correction() {
        let mut demod = Demodulator::new();
        let rate = 12000.0f32;
        // A 1 kHz USB tone with 200 Hz residual offset
        // Without correction: the tone appears at 1200 Hz instead of 1000 Hz
        // With correction: the mixer shifts it back to 1000 Hz
        let iq: Vec<Complex32> = (0..1200)
            .map(|i| {
                let t = i as f32 / rate;
                // Tone at 1000 Hz + 200 Hz offset = 1200 Hz in channel
                Complex32::new(0.0, 2.0 * PI * 1200.0 * t).exp()
            })
            .collect();

        // With residual_hz = 200 (the channel is 200 Hz too low),
        // the correction mixer shifts by -200 Hz, bringing the tone to 1000 Hz
        let result = demod.demodulate(&iq, rate, DemodMode::Usb, 1, 100_000_000, 2700, 48000, false, 200.0, AgcMode::Medium, 300);
        assert!(result.is_some());
        let audio = result.unwrap();
        assert!(!audio.is_empty());
    }
}
