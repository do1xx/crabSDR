#!/usr/bin/env python3
"""iqscan — Übersicht über eine .cu8-Aufnahme: Rauschboden, Max-Hold- und Mittelwert-Spektrum, stärkste Träger
(Frequenz, dB über Rauschen, Belegung in % der Zeit), Prüfung eines Wunschkanals, Pegelstatistik (Übersteuerung, DC).
   iqscan.py testdata/70cm-o.cu8 --center 438500000 [--check 438925000] [--sec 60]"""
import argparse, numpy as np
ap = argparse.ArgumentParser(); ap.add_argument("file"); ap.add_argument("--center", type=int, required=True)
ap.add_argument("--rate", type=int, default=2048000); ap.add_argument("--sec", type=float, default=60); ap.add_argument("--check", type=int, action="append", default=[])
ap.add_argument("--nfft", type=int, default=4096); ap.add_argument("--min-db", type=float, default=10)
a = ap.parse_args(); FS, N = a.rate, a.nfft
raw = np.fromfile(a.file, dtype=np.uint8, count=int(FS * 2 * a.sec))
n = len(raw) // 2
print(f"{a.file}: {n/FS:.1f} s, u8 min/max {raw.min()}/{raw.max()}, Mittel I/Q {raw[0::2].mean():.1f}/{raw[1::2].mean():.1f}, "
      f"Anteil Vollausschlag {(np.sum((raw==0)|(raw==255))/len(raw))*100:.3f} %")
w = np.hanning(N).astype(np.float32); hold = np.full(N, -999.0); avg = np.zeros(N); occ = np.zeros(N); nb = 0
STEP = N * 8      # jeden 8. Block auswerten (reicht für Max-Hold/Belegung)
for s in range(0, n - N, STEP):
    blk = (raw[2*s:2*(s+N)].astype(np.float32) - 127.5) / 127.5
    iq = blk[0::2] + 1j * blk[1::2]
    p = 10 * np.log10(np.abs(np.fft.fftshift(np.fft.fft(iq * w))) ** 2 / N + 1e-20)
    hold = np.maximum(hold, p); avg += p; nb += 1
    if nb == 1: floor0 = np.median(p)
    occ += (p > floor0 + a.min_db)
avg /= nb; occ = occ / nb * 100
f = a.center + np.fft.fftshift(np.fft.fftfreq(N, 1 / FS))
floor = np.median(avg); print(f"Rauschboden (Median, Mittelwert-Spektrum) {floor:.1f} dB; Blöcke {nb}")
m = hold - floor; idx = np.where(m > a.min_db)[0]; peaks = []
for i in idx:
    if peaks and i - peaks[-1][0] < 6:
        if m[i] > peaks[-1][1]: peaks[-1] = (i, m[i])
    else: peaks.append((i, m[i]))
print("Stärkste Träger (Max-Hold über Rauschen, Belegung):")
for i, v in sorted(peaks, key=lambda t: -t[1])[:15]:
    print(f"  {f[i]/1e6:10.4f} MHz  +{v:5.1f} dB  {occ[max(i-2,0):i+3].max():5.1f} % der Zeit  (Mittel +{avg[i]-floor:5.1f} dB)")
for c in a.check:
    sel = np.abs(f - c) <= 6000
    print(f"Kanal {c/1e6:.4f} MHz ±6 kHz: Max-Hold +{(hold[sel]-floor).max():.1f} dB, Mittel +{(avg[sel]-floor).max():.1f} dB, Belegung {occ[sel].max():.1f} %")
