#!/usr/bin/env python3
"""audiostats — Kennzahlen einer WAV-Datei (mono, 48 kHz): SINAD bei bekanntem Ton, Rauschteppich, stärkste Störlinien
(Spurs) im NF-Band, Klick-Zähler (impulsive Sprünge), Pegel-Statistik in 100-ms-Fenstern (Aussetzer).
   audiostats.py out.wav [--tone 1000] [--skip 1.0]"""
import argparse, numpy as np, wave, json
ap = argparse.ArgumentParser(); ap.add_argument("wav"); ap.add_argument("--tone", type=float, default=None)
ap.add_argument("--skip", type=float, default=1.0, help="Sekunden am Anfang verwerfen"); ap.add_argument("--lo", type=float, default=300); ap.add_argument("--hi", type=float, default=3000)
a = ap.parse_args()
with wave.open(a.wav) as w:
    fs = w.getframerate(); x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768
x = x[int(a.skip * fs):]
res = {"file": a.wav, "sec": round(len(x) / fs, 2), "rms_dbfs": round(20 * np.log10(np.sqrt(np.mean(x**2)) + 1e-12), 1),
       "peak_dbfs": round(20 * np.log10(np.max(np.abs(x)) + 1e-12), 1)}
# Spektrum (Welch-artig, 8192er Fenster)
N = 8192; win = np.hanning(N); nseg = len(x) // N
P = np.zeros(N // 2 + 1)
for i in range(nseg):
    P += np.abs(np.fft.rfft(x[i * N:(i + 1) * N] * win)) ** 2
P /= max(nseg, 1); f = np.fft.rfftfreq(N, 1 / fs); Pdb = 10 * np.log10(P + 1e-20)
band = (f >= a.lo) & (f <= a.hi)
res["noise_floor_db_median"] = round(float(np.median(Pdb[band])), 1)
if a.tone:
    tb = np.abs(f - a.tone) <= 3 * fs / N
    p_tone = P[tb].sum(); p_rest = P[band & ~tb].sum()
    res["sinad_db"] = round(10 * np.log10((p_tone + p_rest) / max(p_rest, 1e-20)), 1)
    res["thd_n_pct"] = round(100 * np.sqrt(p_rest / max(p_tone, 1e-20)), 3)
# Spurs: lokale Maxima ≥ 12 dB über dem Median, ohne Ton und dessen Harmonische
m = Pdb - np.median(Pdb[band]); spurs = []
for i in np.where(band)[0]:
    if m[i] > 12 and m[i] >= m[max(i-4,0):i+5].max():
        if a.tone and any(abs(f[i] - h * a.tone) < 4 * fs / N for h in range(1, 6)): continue
        spurs.append((round(float(f[i])), round(float(m[i]), 1)))
res["spurs_hz_db"] = sorted(spurs, key=lambda t: -t[1])[:8]
# Klicks: Sprünge der 1. Ableitung > 10× lokaler MAD (je 1-s-Fenster, damit ein AGC-Anstieg keine Klicks vortäuscht),
# mindestens 5 ms Abstand; dazu die Kurtosis der Ableitung (3 = gaußisch, >6 = impulsiv)
clicks = 0; kurt = []
for s0 in range(0, len(x) - fs, fs):
    d = np.diff(x[s0:s0 + fs]); mad = np.median(np.abs(d - np.median(d))) + 1e-9
    kurt.append(float(np.mean((d - d.mean()) ** 4) / (np.var(d) ** 2 + 1e-30)))
    idx = np.where(np.abs(d) > 10 * mad)[0]; last = -fs
    for i in idx:
        if i - last > fs * 0.005: clicks += 1; last = i
res["clicks"] = int(clicks); res["clicks_per_s"] = round(clicks / (len(x) / fs), 2)
res["deriv_kurtosis_max"] = round(max(kurt), 1) if kurt else None
# Aussetzer: 100-ms-Fenster mit RMS < −60 dBFS
W = fs // 10; nw = len(x) // W
rmsw = np.sqrt(np.mean(x[:nw * W].reshape(nw, W) ** 2, axis=1))
res["silent_100ms_windows"] = int(np.sum(rmsw < 1e-3)); res["rms_window_min_dbfs"] = round(20 * np.log10(rmsw.min() + 1e-12), 1)
print(json.dumps(res, ensure_ascii=False))
