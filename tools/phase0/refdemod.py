#!/usr/bin/env python3
"""refdemod — Referenz-NFM-Demodulation einer .cu8-Aufnahme mit numpy/scipy, unabhängig von crabSDR.
   refdemod.py testdata/70cm-o.cu8 --center 438500000 --freq 438925000 --out ref.wav [--sec 20] [--bw 12500]
Kette: u8→complex, Mischen auf 0 Hz, polyphase Dezimation 2,048 MS/s → 48 kHz, ZF-Tiefpass ±bw/2 (FIR),
Diskriminator (Phasendifferenz, normiert auf 2,5 kHz Hub), NF-Tiefpass 3 kHz, Hochpass 300 Hz, WAV 48 kHz i16.
Gibt Kanalleistung (dBFS) und NF-Pegel aus."""
import argparse, numpy as np, scipy.signal as sig, wave

ap = argparse.ArgumentParser()
ap.add_argument("file"); ap.add_argument("--center", type=int, required=True); ap.add_argument("--freq", type=int, required=True)
ap.add_argument("--out", required=True); ap.add_argument("--sec", type=float, default=20); ap.add_argument("--skip", type=float, default=0)
ap.add_argument("--bw", type=int, default=12500); ap.add_argument("--dev", type=float, default=2500); ap.add_argument("--rate", type=int, default=2048000)
a = ap.parse_args()
FS, OUT = a.rate, 48000
raw = np.fromfile(a.file, dtype=np.uint8, count=int(FS * 2 * a.sec), offset=int(FS * 2 * a.skip))
n = len(raw) // 2
iq = np.empty(n, dtype=np.complex64)
off = a.freq - a.center
CH = 1 << 20; phase = 0.0
for s in range(0, n, CH):                      # blockweise mischen (Speicher), Phase fortlaufend
    e = min(s + CH, n); k = np.arange(e - s)
    blk = (raw[2 * s:2 * e].astype(np.float32) - 127.5) / 127.5
    iq[s:e] = (blk[0::2] + 1j * blk[1::2]) * np.exp(-1j * (phase + 2 * np.pi * off / FS * k)).astype(np.complex64)
    phase = (phase + 2 * np.pi * off / FS * (e - s)) % (2 * np.pi)
from math import gcd
g = gcd(FS, OUT); ch = sig.resample_poly(iq, OUT // g, FS // g)            # 3/128 bei 2,048 MS/s
b_if = sig.firwin(255, a.bw / 2, fs=OUT); ch = sig.lfilter(b_if, 1, ch)
p_ch = 10 * np.log10(np.mean(np.abs(ch) ** 2) + 1e-20)
d = np.angle(ch[1:] * np.conj(ch[:-1])) * OUT / (2 * np.pi * a.dev)      # ±1 bei ±dev
b_af = sig.firwin(255, 3000, fs=OUT); d = sig.lfilter(b_af, 1, d)
b_hp = sig.butter(2, 300, "high", fs=OUT); d = sig.lfilter(b_hp[0], b_hp[1], d)
d = d[OUT // 4:]                                                            # Einschwingen weg
pcm = (np.clip(d * 0.5, -1, 1) * 32767).astype(np.int16)
with wave.open(a.out, "wb") as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(OUT); w.writeframes(pcm.tobytes())
print(f"{a.out}: {len(pcm)/OUT:.1f} s, Kanalleistung {p_ch:.1f} dBFS, NF-RMS {np.sqrt(np.mean(d**2)):.4f} (1.0 = voller Hub)")
