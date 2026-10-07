#!/usr/bin/env python3
"""synth — synthetisches Testsignal als .cu8 (2,048 MS/s), bekannter Inhalt für messbare Tonqualität (SINAD):
  A  NFM, 1000-Hz-Ton, Hub 2,5 kHz, −20 dBFS bei +400 kHz      (438,900 MHz bei Mitte 438,500)
  B  unmodulierter Träger −30 dBFS bei −300 kHz                  (438,200 MHz) — „ruhiger Träger" (Knistertest)
  C  NFM Nachbarkanal +425 kHz, 400-Hz-Ton, −10 dBFS             (438,925 MHz, 25 kHz neben A: Nachbarkanal-Test)
  D  NFM, 1000-Hz-Ton, −50 dBFS bei −700 kHz                    (437,800 MHz) — schwaches Signal
  Rauschen: komplexes AWGN −65 dBFS/Sample.  Ausgabe u8, gerundet.
   synth.py testdata/synth.cu8 --sec 30"""
import argparse, numpy as np
ap = argparse.ArgumentParser(); ap.add_argument("out"); ap.add_argument("--sec", type=float, default=30); ap.add_argument("--rate", type=int, default=2048000)
a = ap.parse_args(); FS = a.rate
SIGS = [  # (Offset Hz, Pegel dBFS, Ton Hz, Hub Hz)
    (+400_000, -20, 1000.0, 2500.0), (-300_000, -30, None, 0.0), (+425_000, -10, 400.0, 2500.0), (-700_000, -50, 1000.0, 2500.0)]
CH = 1 << 20; total = int(FS * a.sec); rng = np.random.default_rng(1)
ph = [0.0] * len(SIGS); mph = [0.0] * len(SIGS)
with open(a.out, "wb") as f:
    for s in range(0, total, CH):
        e = min(s + CH, total); k = np.arange(e - s); x = np.zeros(e - s, dtype=np.complex64)
        for i, (off, db, tone, dev) in enumerate(SIGS):
            amp = 10 ** (db / 20)
            if tone:
                m = np.sin(mph[i] + 2 * np.pi * tone / FS * k); mph[i] = (mph[i] + 2 * np.pi * tone / FS * (e - s)) % (2 * np.pi)
                inst = 2 * np.pi * (off + dev * m) / FS
            else:
                inst = np.full(e - s, 2 * np.pi * off / FS)
            phase = ph[i] + np.cumsum(inst); ph[i] = phase[-1] % (2 * np.pi)
            x += (amp * np.exp(1j * phase)).astype(np.complex64)
        nz = 10 ** (-65 / 20) / np.sqrt(2)
        x += (rng.standard_normal(e - s) + 1j * rng.standard_normal(e - s)).astype(np.complex64) * nz
        u = np.empty(2 * (e - s), dtype=np.uint8)
        u[0::2] = np.clip(np.round(x.real * 127.5 + 127.5), 0, 255); u[1::2] = np.clip(np.round(x.imag * 127.5 + 127.5), 0, 255)
        f.write(u.tobytes())
print(f"{a.out}: {a.sec} s, {total*2/1e6:.0f} MB")
