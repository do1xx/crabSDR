pub mod channelizer;
pub mod demod;
pub mod resample;

use std::f32::consts::PI;

/// Modified Bessel function of the first kind, order 0.
/// Used by Kaiser window in both channelizer and resampler.
/// Series expansion with 25 terms — accurate to ~1e-12 for x < 20.
pub(crate) fn bessel_i0(x: f32) -> f32 {
    let mut sum = 1.0f64;
    let mut term = 1.0f64;
    let xd = x as f64;
    let half_x = xd * 0.5;
    for k in 1..=25 {
        term *= (half_x / k as f64) * (half_x / k as f64);
        sum += term;
    }
    sum as f32
}

/// Kaiser window of length `n` with shape parameter `beta`.
pub(crate) fn kaiser_window(n: usize, beta: f32) -> Vec<f32> {
    if n <= 1 {
        return vec![1.0; n];
    }
    let denom = bessel_i0(beta);
    let half = (n - 1) as f32 * 0.5;
    (0..n)
        .map(|i| {
            let r = (i as f32 - half) / half;
            bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom
        })
        .collect()
}

/// Design a lowpass FIR filter using windowed-sinc method with Kaiser window.
///
/// `num_taps`: filter length (odd recommended)
/// `cutoff_normalized`: cutoff frequency as fraction of sample rate (0..0.5)
/// `beta`: Kaiser window beta (higher = more stopband attenuation)
pub(crate) fn design_lowpass_fir(num_taps: usize, cutoff_normalized: f32, beta: f32) -> Vec<f32> {
    let window = kaiser_window(num_taps, beta);
    let mid = (num_taps - 1) as f32 * 0.5;
    let omega_c = 2.0 * PI * cutoff_normalized;

    let mut coeffs: Vec<f32> = (0..num_taps)
        .map(|i| {
            let n = i as f32 - mid;
            let sinc = if n.abs() < 1e-6 {
                omega_c / PI
            } else {
                (omega_c * n).sin() / (PI * n)
            };
            sinc * window[i]
        })
        .collect();

    // Normalize to unity DC gain
    let sum: f32 = coeffs.iter().sum();
    if sum.abs() > 1e-10 {
        for c in &mut coeffs {
            *c /= sum;
        }
    }

    coeffs
}
