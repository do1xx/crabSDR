#!/usr/bin/env python3
"""decoder-signals.py — künstliches 2-m-Band (cu8, 2,048 MS/s, Mitte 145,000 MHz, 60 s) mit bekannten Testsignalen:
  APRS 144,800 MHz FM: 6 AX.25-Pakete (gen_packets aus direwolf), bei t = 3, 13, 23, 33, 43, 53 s
  FT8  144,174 MHz USB: je Zyklus (0/15/30/45 s, Beginn +0,5 s) zwei Nachrichten bei 1200 und 1850 Hz (Töne aus ft8code)
Läuft im vollen crabSDR-Image (braucht gen_packets, ft8code, numpy). Schreibt <out>.cu8 und <out>.json (Erwartung).
   python3 decoder-signals.py /work/test2m"""
import json, os, re, subprocess, sys, tempfile, wave
import numpy as np

out = sys.argv[1]
FS, T, FC = 2_048_000, 60, 145_000_000
rng = np.random.default_rng(1)

# --- APRS: Ton 48 kHz aus gen_packets ---
AR = 48000
aprs = np.zeros(T * AR)
pk = [f"DO1XX-{i}>APRS,WIDE1-1:!5301.{10+i:02d}N/01129.{20+i:02d}E-crabSDR Test {i}" for i in range(1, 7)]
with tempfile.TemporaryDirectory() as td:
    for i, p in enumerate(pk):
        f = os.path.join(td, "m.txt"); open(f, "w").write(p + "\n")
        w = os.path.join(td, "p.wav")
        subprocess.run(["gen_packets", "-r", str(AR), "-a", "70", "-o", w, f], check=True, capture_output=True)
        with wave.open(w) as wf: a = np.frombuffer(wf.readframes(wf.getnframes()), dtype=np.int16) / 32768.0
        t0 = int((3 + 10 * i) * AR); aprs[t0:t0 + len(a)] = a[: len(aprs) - t0]

# --- FT8: 79 Symbole je Nachricht, 8-FSK, 6,25 Hz Abstand, 0,16 s je Symbol, 12 kHz ---
FR = 12000
def ft8_tones(msg):
    txt = subprocess.run(["ft8code", msg], capture_output=True, text=True, check=True).stdout
    tail = txt.split("Channel symbols")[1]
    digits = "".join(re.findall(r"\b[0-7]{7,}\b", tail))
    assert len(digits) == 79, (msg, len(digits))
    return [int(c) for c in digits]
msgs = [("CQ DO1XX JO53", 1200.0), ("DL0ABC DO1XX JO53", 1850.0)]
ft8 = np.zeros(T * FR)
for cyc in range(0, T, 15):
    for msg, f0 in msgs:
        tones = ft8_tones(msg); ph = 0.0; sym = []
        for k in tones:
            f = f0 + 6.25 * k
            n = np.arange(1920); sym.append(np.sin(ph + 2 * np.pi * f * n / FR)); ph += 2 * np.pi * f * 1920 / FR
        s = np.concatenate(sym) * 0.35
        t0 = int((cyc + 0.5) * FR); ft8[t0:t0 + len(s)] += s

# --- komplexe Basisbänder ---
aprs_bb = np.exp(1j * 2 * np.pi * 3000 * np.cumsum(aprs) / AR)            # FM, 3 kHz Hub, Träger immer an
spec = np.fft.fft(ft8); spec[len(spec) // 2:] = 0; spec[1:len(spec) // 2] *= 2
ft8_bb = np.fft.ifft(spec)                                                  # analytisch: Töne oberhalb der Trägerfrequenz = USB
ta, tf = np.arange(len(aprs_bb)) / AR, np.arange(len(ft8_bb)) / FR
F_APRS, F_FT8 = 144_800_000 - FC, 144_174_000 - FC

with open(out + ".cu8", "wb") as fo:
    B = FS // 2
    for b0 in range(0, T * FS, B):
        t = (b0 + np.arange(B)) / FS
        xa = np.interp(t, ta, aprs_bb.real) + 1j * np.interp(t, ta, aprs_bb.imag)
        xf = np.interp(t, tf, ft8_bb.real) + 1j * np.interp(t, tf, ft8_bb.imag)
        x = 0.05 * xa * np.exp(2j * np.pi * F_APRS * t) + 0.004 * xf * np.exp(2j * np.pi * F_FT8 * t)
        x += (rng.standard_normal(B) + 1j * rng.standard_normal(B)) * 0.01
        iq = np.empty(2 * B, dtype=np.uint8)
        iq[0::2] = np.clip(x.real * 127 + 127.5, 0, 255); iq[1::2] = np.clip(x.imag * 127 + 127.5, 0, 255)
        fo.write(iq.tobytes())
json.dump({"aprs": pk, "ft8": [m for m, _ in msgs], "cycles_per_min": 4}, open(out + ".json", "w"), indent=1)
print("fertig:", out + ".cu8", os.path.getsize(out + ".cu8") // 1_000_000, "MB")
